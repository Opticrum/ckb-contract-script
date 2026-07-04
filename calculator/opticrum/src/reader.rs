//! On-chain cell readers for Opticrum Order and Match cells.
//!
//! These query the CKB indexer and RPC, parse raw cell data, and return
//! the typed structs (`OrderInfo`, `MatchInfo`) that the calculator
//! instructions consume.

use ckb_cinnabar_calculator::{
    indexer::{CellQueryOptions, LiveCell, SearchMode},
    re_exports::{
        ckb_jsonrpc_types::Either,
        ckb_types::{
            core::{Capacity, ScriptHashType},
            packed::{CellOutput, Script, WitnessArgs},
            prelude::*,
            H256,
        },
        eyre::{self, eyre},
    },
    rpc::{GetCellsIter, RPC},
    skeleton::{ScriptEx, TYPE_ID_CODE_HASH},
};

use crate::{
    config::opticrum_contract_type_id,
    types::{
        CompressedPubkey, MatchArgs, MatchData, MatchInfo, OrderArgs, OrderData, OrderInfo,
        OutPoint, Xudt,
    },
};

// ---------------------------------------------------------------------------
// Helpers — Opticrum lock script resolution
// ---------------------------------------------------------------------------

/// Resolve the Opticrum lock script's `code_hash` by finding the deployed
/// contract cell on-chain.
///
/// The contract cell is identified by its type script:
/// `(TYPE_ID_CODE_HASH, Type, type_id)`. Its type hash becomes the
/// `code_hash` for the Opticrum lock script — hash type is always `Type`.
///
/// This mirrors the resolution path that `AddOpticrumContractCelldep`
/// performs during transaction building.
fn resolve_opticrum_code_hash<T: RPC>(rpc: &T) -> [u8; 32] {
    let type_id = opticrum_contract_type_id(rpc.network());

    let type_script = Script::new_builder()
        .code_hash(TYPE_ID_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(type_id.as_bytes().pack())
        .build();

    type_script.calc_script_hash().unpack()
}

/// Build a `CellQueryOptions` for scanning cells locked by the Opticrum
/// script.
///
/// When `args_prefix` is `Some`, it is set as the lock script args so the
/// indexer's prefix search only returns cells whose lock args start with
/// those bytes. For both Order and Match cells, the first 33 bytes are the
/// buyer's Fiber pubkey, so passing a pubkey here filters to a single buyer.
async fn opticrum_query<T: RPC>(
    rpc: &T,
    args_prefix: Option<Vec<u8>>,
) -> eyre::Result<CellQueryOptions> {
    let code_hash = resolve_opticrum_code_hash(rpc);
    let lock_args = args_prefix.unwrap_or_default();
    let lock_script = ScriptEx::new_type(code_hash.into(), lock_args);

    let mut query = CellQueryOptions::new_lock(lock_script.to_script_unchecked());
    query.script_search_mode = Some(SearchMode::Prefix);
    query.with_data = Some(true);
    Ok(query)
}

// ---------------------------------------------------------------------------
// Generic cell scanner
// ---------------------------------------------------------------------------

/// Iterate all cells locked by the Opticrum script and parse them with
/// the provided function. Silently skips cells that fail to parse.
///
/// `args_prefix` is forwarded to [`opticrum_query`] to narrow results
/// at the indexer level (e.g. by buyer Fiber pubkey).
async fn scan_cells<T: RPC, U>(
    rpc: &T,
    args_prefix: Option<Vec<u8>>,
    parse_fn: impl Fn(&LiveCell) -> eyre::Result<U>,
) -> eyre::Result<Vec<U>> {
    let query = opticrum_query(rpc, args_prefix).await?;
    let search_key = query.into();
    let mut results = Vec::new();
    let mut iter = GetCellsIter::new(rpc, search_key);

    while let Some(batch) = iter.next_batch(50).await? {
        for cell in batch {
            let live: LiveCell = cell.into();
            if let Ok(value) = parse_fn(&live) {
                results.push(value);
            }
        }
    }

    Ok(results)
}

// ---------------------------------------------------------------------------
// Cell → typed struct parsers
// ---------------------------------------------------------------------------

/// Compute the real rent capacity by subtracting the cell's occupied
/// capacity from its total capacity.
///
/// Occupied bytes = CellOutput molecule bytes + data bytes.
/// Each byte costs `CKB_DECIMAL` shannons.
fn real_rent_capacity(output: &CellOutput, data: &[u8]) -> eyre::Result<u64> {
    let total: u64 = output.capacity().unpack();
    let occupied_capacity = output.occupied_capacity(Capacity::bytes(data.len())?)?;
    Ok(total.saturating_sub(occupied_capacity.as_u64()))
}

/// Fields common to every Opticrum cell, extracted once by [`parse_cell_prologue`].
struct ParsedCellMeta {
    lock_args: Vec<u8>,
    raw_data: Vec<u8>,
    outpoint: OutPoint,
    block_number: u64,
    output: CellOutput,
}

/// Parse the shared envelope of an Opticrum cell.
///
/// Extracts the fields that are identical between Order and Match parsing:
/// raw lock args, output data, outpoint, block number, and the cell output.
fn parse_cell_prologue(cell: &LiveCell) -> eyre::Result<ParsedCellMeta> {
    Ok(ParsedCellMeta {
        lock_args: cell.output.lock().args().raw_data().to_vec(),
        raw_data: cell.output_data.to_vec(),
        outpoint: OutPoint::from_slice(cell.out_point.as_slice())
            .map_err(|e| eyre!("Bad outpoint: {e}"))?,
        block_number: cell.block_number,
        output: cell.output.clone(),
    })
}

/// Parse a `LiveCell` (with Opticrum lock) into an `OrderInfo`.
fn parse_order_cell(cell: &LiveCell) -> eyre::Result<OrderInfo> {
    let base = parse_cell_prologue(cell)?;

    let order_args =
        OrderArgs::from_slice(&base.lock_args).map_err(|e| eyre!("Bad Order args: {}", e))?;

    let order_data =
        OrderData::from_slice(&base.raw_data).map_err(|e| eyre!("Bad Order data: {}", e))?;

    let ckb_capacity = real_rent_capacity(&base.output, &base.raw_data)?;

    let xudt = base.output.type_().to_opt().map(|type_script| Xudt {
        amount: order_data.xudt_amount,
        type_script,
    });

    Ok(OrderInfo {
        order_args,
        order_data,
        xudt,
        ckb_capacity,
        order_outpoint: base.outpoint,
        fiber_address: None, // enriched later in scan_orders via fetch_fiber_address
    })
}

/// Parse a `LiveCell` (with Opticrum lock) into a `MatchInfo`.
fn parse_match_cell(cell: &LiveCell) -> eyre::Result<MatchInfo> {
    let base = parse_cell_prologue(cell)?;

    let match_args =
        MatchArgs::from_slice(&base.lock_args).map_err(|e| eyre!("Bad Match args: {}", e))?;

    let match_data =
        MatchData::from_slice(&base.raw_data).map_err(|e| eyre!("Bad Match data: {}", e))?;

    let ckb_capacity = real_rent_capacity(&base.output, &base.raw_data)?;

    let xudt = base.output.type_().to_opt().map(|type_script| Xudt {
        amount: match_data.xudt_amount,
        type_script,
    });

    Ok(MatchInfo {
        match_args,
        match_data,
        xudt,
        ckb_capacity,
        match_outpoint: base.outpoint,
        match_current_block: base.block_number,
    })
}

// ---------------------------------------------------------------------------
// Fiber address extraction from creation transaction witnesses
// ---------------------------------------------------------------------------

/// Fetch the buyer's Fiber node address from the creation transaction witness.
///
/// The address is stored in the `output_type` field of the `WitnessArgs` at
/// position `witnesses[input_count + output_index]` in the order's creation
/// transaction. Returns `None` if the transaction is unavailable, the witness
/// is missing, or the data is not valid UTF-8.
async fn fetch_fiber_address<T: RPC>(
    rpc: &T,
    tx_hash: &[u8; 32],
    output_index: u32,
) -> Option<String> {
    // Resolve the transaction hash
    let hash = H256::from_slice(tx_hash).ok()?;

    // Fetch the creation transaction
    let tx_response = rpc.get_transaction(&hash).await.ok()??;
    let tx_format = tx_response.transaction?;

    // ResponseFormat.inner is Either<TransactionView, JsonBytes>
    // Extract the TransactionView (JSON format — the common case)
    let tx_view = match tx_format.inner {
        Either::Left(view) => view,
        Either::Right(_hex) => return None, // hex format not supported here
    };

    // The witness index follows the output_type convention:
    // witnesses[input_count + output_index]
    // tx_view.inner is ckb_jsonrpc_types::Transaction
    let input_count = tx_view.inner.inputs.len();
    let witness_index = input_count + output_index as usize;

    let raw_bytes = tx_view
        .inner
        .witnesses
        .get(witness_index)?
        .as_bytes()
        .to_vec();

    if raw_bytes.is_empty() {
        return None;
    }

    // Parse the WitnessArgs molecule to extract the output_type field
    let witness_args = WitnessArgs::from_slice(&raw_bytes).ok()?;
    let output_type_opt = witness_args.output_type().to_opt()?;
    let output_type_bytes = output_type_opt.raw_data().to_vec();

    if output_type_bytes.is_empty() {
        return None;
    }

    String::from_utf8(output_type_bytes.to_vec()).ok()
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Scan all live Order cells on-chain.
///
/// When `fiber_pubkey` is `Some`, the indexer query is narrowed to cells
/// whose lock args start with the given pubkey (the first 33 bytes of both
/// Order and Match args). Pass `None` to return all orders.
///
/// Each order is enriched with the buyer's Fiber node address (multiaddr)
/// extracted from the creation transaction's output_type witness when
/// available.
pub async fn scan_orders<T: RPC>(
    rpc: &T,
    fiber_pubkey: Option<CompressedPubkey>,
) -> eyre::Result<Vec<OrderInfo>> {
    let args_prefix = fiber_pubkey.map(|pk| pk.to_bytes().to_vec());
    let mut orders = scan_cells(rpc, args_prefix, parse_order_cell).await?;

    // Enrich with fiber addresses from creation transaction witnesses
    for order in &mut orders {
        let address = fetch_fiber_address(
            rpc,
            &order.order_outpoint.tx_hash,
            order.order_outpoint.index,
        )
        .await;
        order.fiber_address = address;
    }

    Ok(orders)
}

/// Scan all live Match cells on-chain.
///
/// When `fiber_pubkey` is `Some`, the indexer query is narrowed to cells
/// whose lock args start with the given pubkey (the first 33 bytes of both
/// Order and Match args). Pass `None` to return all matches.
pub async fn scan_matches<T: RPC>(
    rpc: &T,
    fiber_pubkey: Option<CompressedPubkey>,
) -> eyre::Result<Vec<MatchInfo>> {
    let args_prefix = fiber_pubkey.map(|pk| pk.to_bytes().to_vec());
    scan_cells(rpc, args_prefix, parse_match_cell).await
}
