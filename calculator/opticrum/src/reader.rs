//! On-chain cell readers for Opticrum Order and Match cells.
//!
//! These query the CKB indexer and RPC, parse raw cell data, and return
//! the typed structs (`OrderInfo`, `MatchInfo`) that the calculator
//! instructions consume.

use ckb_cinnabar_calculator::{
    indexer::{CellQueryOptions, LiveCell, SearchMode},
    re_exports::{
        ckb_types::{
            core::{Capacity, ScriptHashType},
            packed::{CellOutput, Script},
            prelude::*,
        },
        eyre::{self, eyre},
    },
    rpc::{GetCellsIter, RPC},
    skeleton::{ScriptEx, TYPE_ID_CODE_HASH},
};

use crate::{
    config::opticrum_contract_type_id,
    types::{MatchArgs, MatchData, MatchInfo, OrderArgs, OrderData, OrderInfo, OutPoint, Xudt},
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
/// script, optionally narrowed to a specific args length.
async fn opticrum_query<T: RPC>(rpc: &T) -> eyre::Result<CellQueryOptions> {
    let code_hash = resolve_opticrum_code_hash(rpc);
    let lock_script = ScriptEx::new_type(code_hash.into(), vec![]);

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
async fn scan_cells<T: RPC, U>(
    rpc: &T,
    parse_fn: impl Fn(&LiveCell) -> eyre::Result<U>,
) -> eyre::Result<Vec<U>> {
    let query = opticrum_query(rpc).await?;
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
// Public API
// ---------------------------------------------------------------------------

/// Scan all live Order cells on-chain.
pub async fn scan_orders<T: RPC>(rpc: &T) -> eyre::Result<Vec<OrderInfo>> {
    scan_cells(rpc, parse_order_cell).await
}

/// Scan all live Match cells on-chain.
pub async fn scan_matches<T: RPC>(rpc: &T) -> eyre::Result<Vec<MatchInfo>> {
    scan_cells(rpc, parse_match_cell).await
}
