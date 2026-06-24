//! Off-chain aggregation types and re-exports from the protocol crate.
//!
//! Protocol types (`OrderArgs`, `OrderData`, `MatchArgs`, `MatchData`,
//! and all length constants) are defined canonically in `opticrum-protocol`
//! and re-exported here. This module adds only the types that live purely
//! off-chain: `AnnualYield`, `OrderInfo`, `MatchInfo`, and `Xudt`.

use ckb_cinnabar_calculator::re_exports::ckb_types::packed::Script;

// Re-export all protocol types and constants
pub use opticrum_protocol::*;

use crate::config::ABOUT_ONE_DAY_BLOCKS;

// ---------------------------------------------------------------------------
// Xudt — token specification (off-chain aggregation, not on-chain layout)
// ---------------------------------------------------------------------------

/// Specifies the xUDT token attached to an Order or Match Cell.
///
/// The `amount` is stored in the cell data (first 16 bytes, u128 LE).
/// The `type_script` is set as the cell's type script
#[derive(Clone, Debug)]
pub struct Xudt {
    pub amount: u128,
    pub type_script: Script,
}

// ---------------------------------------------------------------------------
// AnnualYield — annual yield in percentage
// ---------------------------------------------------------------------------

/// Represents the annual yield in percentage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnualYield(pub u8);

impl AnnualYield {
    pub fn to_ckb(&self, order: &OrderData) -> u64 {
        let one_year_profit = order.channel_capacity as f64 * self.0 as f64 / 100.0;
        let one_day_profit = one_year_profit / 365.0;
        let days = order.escrow_blocks as f64 / ABOUT_ONE_DAY_BLOCKS as f64;
        (one_day_profit * days) as u64
    }

    pub fn to_xudt(&self, order: &OrderData) -> u128 {
        self.to_ckb(order) as u128
    }
}

// ---------------------------------------------------------------------------
// OrderInfo — aggregated on-chain Order cell information
// ---------------------------------------------------------------------------

/// Parsed representation of a live Order cell, including indexer-provided
/// context (outpoint, capacity).
#[derive(Clone, Debug)]
pub struct OrderInfo {
    pub order_args: OrderArgs,
    pub order_data: OrderData,
    pub xudt: Option<Xudt>,
    pub ckb_capacity: u64,
    pub order_outpoint: OutPoint,
}

// ---------------------------------------------------------------------------
// MatchInfo — aggregated on-chain Match cell information
// ---------------------------------------------------------------------------

/// Parsed representation of a live Match cell, including indexer-provided
/// context (outpoint, capacity, block number).
#[derive(Clone, Debug)]
pub struct MatchInfo {
    pub match_args: MatchArgs,
    pub match_data: MatchData,
    pub xudt: Option<Xudt>,
    pub ckb_capacity: u64,
    pub match_outpoint: OutPoint,
    pub match_current_block: u64,
}

impl MatchInfo {
    pub fn extraction_amount(&self, tip_block: u64) -> u64 {
        let start_block = if self.match_data.last_extraction_block == 0 {
            // Never extracted — count from match creation
            self.match_current_block
        } else {
            self.match_data.last_extraction_block
        };

        let elapsed = tip_block.saturating_sub(start_block);
        (self.match_data.rent_per_block * elapsed as f64) as u64
    }

    pub fn is_exhausted(&self, tip_block: u64) -> bool {
        let accumulated_rent = self.extraction_amount(tip_block);
        match &self.xudt {
            Some(x) => x.amount <= accumulated_rent as u128,
            None => self.ckb_capacity <= accumulated_rent,
        }
    }
}
