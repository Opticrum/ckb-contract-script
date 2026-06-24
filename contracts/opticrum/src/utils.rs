//! Contract-specific utility functions.
//!
//! Protocol types (`OrderArgs`, `OrderData`, `MatchArgs`, `MatchData`,
//! and all length constants) are defined canonically in `opticrum-protocol`
//! and re-exported here. This module adds helpers that access on-chain
//! state via CKB syscalls (only available inside the CKB-VM).

use ckb_cinnabar_verifier::{re_exports::ckb_std, Result};
use ckb_std::{
    ckb_constants::Source,
    ckb_types::{core::ScriptHashType, packed::Script, prelude::Unpack},
    debug,
    high_level::{
        load_cell_capacity, load_cell_data, load_cell_lock, load_cell_lock_hash,
        load_cell_occupied_capacity, load_cell_type, load_cell_type_hash, load_header,
        load_transaction, QueryIter,
    },
};

use crate::{
    error::OpticrumError, FIBER_FUNDING_TYPE_ID_MAINNET, FIBER_FUNDING_TYPE_ID_MOCK,
    FIBER_FUNDING_TYPE_ID_TESTNET,
};
use opticrum_protocol::keyagg;

// Re-export all protocol types from the canonical crate
pub use opticrum_protocol::*;

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

#[macro_export]
macro_rules! ERR {
    ($result:expr, $err:ident) => {
        $crate::utils::map_proto_err($result, $crate::OpticrumError::$err)
    };
}

/// Map a protocol `from_slice` error (`&'static str`) into a verifier error.
pub fn map_proto_err<T>(
    result: core::result::Result<T, &'static str>,
    err: OpticrumError,
) -> Result<T> {
    result.map_err(|_| err.into())
}

// ---------------------------------------------------------------------------
// Header helpers
// ---------------------------------------------------------------------------

/// Load a header block number from HeaderDep by index.
pub fn load_header_block_number(index: usize) -> Result<u64> {
    let header = load_header(index, Source::HeaderDep).map_err(|_| OpticrumError::HeaderNotSet)?;
    let block_number: u64 = header.raw().number().unpack();
    Ok(block_number)
}

// ---------------------------------------------------------------------------
// Input authorization
// ---------------------------------------------------------------------------

/// Check if a lock hash (raw bytes) appears in any tx input's lock script.
pub fn has_lock_in_inputs(lock_hash: &[u8]) -> Result<bool> {
    let found = QueryIter::new(load_cell_lock_hash, Source::Input).any(|hash| hash == lock_hash);
    Ok(found)
}

// ---------------------------------------------------------------------------
// Channel cell lookup
// ---------------------------------------------------------------------------

/// Returns true if the type hash is a recognized Fiber funding type.
fn is_fiber_funding_contract(hash: &[u8; 32]) -> bool {
    *hash == FIBER_FUNDING_TYPE_ID_TESTNET
        || *hash == FIBER_FUNDING_TYPE_ID_MAINNET
        || *hash == FIBER_FUNDING_TYPE_ID_MOCK
}

/// Find the CellDep index of a channel cell matching `channel_outpoint`.
///
/// Outpoints are read from the transaction `cell_deps` table via `load_transaction`.
/// `load_input_out_point` only works for inputs, not cell deps.
///
/// When the mock type hash is not the all-zero wildcard, also requires a Fiber
/// funding type script on the cell.
pub fn find_channel_celldep_index(channel_outpoint: &OutPoint) -> Option<usize> {
    let tx = load_transaction().ok()?;
    for (i, dep) in tx.raw().cell_deps().into_iter().enumerate() {
        if !channel_outpoint.matches(&dep.out_point()) {
            continue;
        }
        if FIBER_FUNDING_TYPE_ID_MOCK != [0u8; 32] {
            let hash = load_cell_type_hash(i, Source::CellDep).ok()??;
            if !is_fiber_funding_contract(&hash) {
                continue;
            }
        }
        return Some(i);
    }
    None
}

/// Verify that the channel CellDep lock args match the MuSig2-aggregated funding key.
///
/// Fiber funding cells store `blake160(x_only_aggregated_pubkey)` in lock args.
pub fn verify_channel_funding_pubkey(
    channel_outpoint: &OutPoint,
    buyer_pk: &CompressedPubkey,
    seller_pk: &CompressedPubkey,
) -> bool {
    let Some(i) = find_channel_celldep_index(channel_outpoint) else {
        return false;
    };
    let Ok(lock) = load_cell_lock(i, Source::CellDep) else {
        return false;
    };
    let channel_args = lock.args().raw_data().to_vec();
    let Ok(xonly) = keyagg::aggregate_funding_keys_xonly(buyer_pk, seller_pk) else {
        return false;
    };
    let hash = ckb_hash::blake2b_256(xonly);
    channel_args.len() == FIBER_FUNDING_LOCK_ARGS_LEN
        && channel_args.as_slice() == &hash[..FIBER_FUNDING_LOCK_ARGS_LEN]
}

/// Find a cell in CellDeps matching the given channel outpoint and type.
///
/// Verifies the channel cell exists in CellDeps with the correct outpoint
/// and Fiber funding type ID. Optional amount checks are performed when
/// `min_capacity` or `min_xudt_amount` is `Some`.
///
/// Used by `order_match` (requires capacity/xUDT verification) and
/// `match_extract` (existence-only — amounts already verified at match time).
///
/// The channel cell's specific identity (outpoint) is attested off-chain
/// via the seller's signature on the match transaction. On-chain we verify
/// that SOME CellDep has enough capacity to satisfy the order.
pub fn find_channel_in_celldeps(
    channel_outpoint: &OutPoint,
    min_capacity: Option<u64>,
    min_xudt_amount: Option<u128>,
    xudt_type_script: Option<Option<&Script>>,
) -> bool {
    let Ok(tx) = load_transaction() else {
        return false;
    };
    if !tx
        .raw()
        .cell_deps()
        .into_iter()
        .any(|dep| channel_outpoint.matches(&dep.out_point()))
    {
        return false;
    }
    for (i, lock) in QueryIter::new(load_cell_lock, Source::CellDep).enumerate() {
        if !is_fiber_funding_contract(&lock.code_hash().unpack())
            || lock.hash_type() != ScriptHashType::Type.into()
        {
            continue;
        }
        if let Some(xudt_type_script) = xudt_type_script {
            let type_script = load_cell_type(i, Source::CellDep).unwrap();
            if type_script.as_ref() != xudt_type_script {
                continue;
            }
        }
        if let Some(amount) = min_xudt_amount {
            let cell_data = load_cell_data(i, Source::CellDep).unwrap_or_default();
            if cell_data.len() < XUDT_AMOUNT_LEN {
                continue;
            }
            let xudt_amount =
                u128::from_le_bytes(cell_data[0..XUDT_AMOUNT_LEN].try_into().unwrap());
            if xudt_amount < amount {
                continue;
            }
        }
        if let Some(cap) = min_capacity {
            let capacity = load_cell_capacity(i, Source::CellDep).unwrap_or(0);
            debug!("min: {cap}, real: {capacity}");
            if capacity < cap {
                continue;
            }
        }
        return true;
    }
    false
}

/// Parse xudt amount and type from cell data.
pub fn parse_xudt(index: usize, source: Source) -> Result<Option<(u128, Script)>> {
    let xudt_amount = {
        let data = load_cell_data(index, source)?;
        if data.len() < XUDT_AMOUNT_LEN {
            return Ok(None);
        }
        u128::from_le_bytes(data[0..XUDT_AMOUNT_LEN].try_into().unwrap())
    };
    let Some(type_script) = load_cell_type(index, source)? else {
        return Ok(None);
    };
    Ok(Some((xudt_amount, type_script)))
}

/// Get the unoccupied capacity of a cell.
pub fn get_unoccupied_capacity(index: usize, source: Source) -> Result<u64> {
    let capacity = load_cell_capacity(index, source)?;
    let occupied_capacity = load_cell_occupied_capacity(index, source)?;
    Ok(capacity.saturating_sub(occupied_capacity))
}

/// Find the index of the Opticrum script in the given source.
///
/// Loads the current cell's lock code_hash and compares against output/input
/// cells' lock code_hashes. Uses code_hash (not script hash) because Type-based
/// scripts resolve to different script hashes even when sharing the same
/// deployed contract.
pub fn find_opticrum_script(source: Source) -> Option<usize> {
    let this_lock = load_cell_lock(0, Source::GroupInput).ok()?;
    let this_code_hash = this_lock.code_hash();
    QueryIter::new(load_cell_lock, source)
        .enumerate()
        .find_map(|(i, lock)| {
            if lock.code_hash().raw_data() == this_code_hash.raw_data()
                && lock.hash_type() == ScriptHashType::Type.into()
            {
                Some(i)
            } else {
                None
            }
        })
}
