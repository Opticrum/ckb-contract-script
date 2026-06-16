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
    high_level::{
        load_cell_capacity, load_cell_data, load_cell_lock, load_cell_lock_hash,
        load_cell_occupied_capacity, load_cell_type, load_cell_type_hash, load_header,
        load_input_out_point, load_script_hash, QueryIter,
    },
};

use crate::{error::OpticrumError, FIBER_FUNDING_TYPE_ID_MAINNET, FIBER_FUNDING_TYPE_ID_TESTNET};

// Re-export all protocol types from the canonical crate
pub use opticrum_protocol::*;

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

#[macro_export]
macro_rules! ERR {
    ($result:expr, $err:ident) => {
        crate::utils::map_proto_err($result, OpticrumError::$err)
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

/// Find a cell in CellDeps with at least the given capacity.
///
/// Returns the CellDep index on success, or `ChannelCellNotInDep` if no
/// cell in CellDeps meets the capacity requirement.
///
/// The channel cell's specific identity (outpoint) is attested off-chain
/// via the seller's signature on the match transaction. On-chain we verify
/// that SOME CellDep has enough capacity to satisfy the order.
pub fn find_channel_in_celldeps(
    channel_outpoint: &OutPoint,
    min_capacity: u64,
    min_xudt_amount: u128,
    xudt_type_script: Option<Option<&Script>>,
) -> bool {
    QueryIter::new(load_cell_type_hash, Source::CellDep)
        .enumerate()
        .any(|(i, hash)| {
            let Some(hash) = hash else {
                return false;
            };
            if hash != FIBER_FUNDING_TYPE_ID_TESTNET && hash != FIBER_FUNDING_TYPE_ID_MAINNET {
                return false;
            }
            let out_point = load_input_out_point(i, Source::CellDep).unwrap_or_default();
            if !channel_outpoint.eq(&out_point) {
                return false;
            }
            if let Some(xudt_type_script) = xudt_type_script {
                let type_script = load_cell_type(i, Source::CellDep).unwrap();
                if type_script.as_ref() != xudt_type_script {
                    return false;
                }
            }
            if min_xudt_amount > 0 {
                let cell_data = load_cell_data(i, Source::CellDep).unwrap_or_default();
                if cell_data.len() < XUDT_AMOUNT_LEN {
                    return false;
                }
                let xudt_amount =
                    u128::from_le_bytes(cell_data[0..XUDT_AMOUNT_LEN].try_into().unwrap());
                if xudt_amount < min_xudt_amount {
                    return false;
                }
            } else {
                let capacity = load_cell_capacity(i, Source::CellDep).unwrap_or_default();
                if capacity < min_capacity {
                    return false;
                }
            }
            true
        })
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
pub fn find_opticrum_script(source: Source) -> Option<usize> {
    let this_code_hash = load_script_hash().unwrap_or_default();
    QueryIter::new(load_cell_lock, source)
        .enumerate()
        .find_map(|(i, lock)| {
            if lock.code_hash().raw_data() == this_code_hash.as_slice()
                && lock.hash_type() == ScriptHashType::Type.into()
            {
                Some(i)
            } else {
                None
            }
        })
}
