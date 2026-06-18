#![no_main]
#![no_std]

use ckb_cinnabar_verifier::{
    cinnabar_main,
    re_exports::ckb_std::{self, ckb_types::packed::Script},
    Result, TREE_ROOT,
};
use opticrum_protocol::{
    MatchArgs, MatchData, OrderArgs, OrderData, MATCH_ARGS_LEN, ORDER_ARGS_LEN,
};

mod error;
mod utils;
mod verifiers;

use error::OpticrumError;
use verifiers::*;

use crate::utils::load_header_block_number;

pub const FIBER_FUNDING_TYPE_ID_TESTNET: [u8; 32] = [
    0x6c, 0x67, 0x88, 0x7f, 0xe2, 0x01, 0xee, 0x0c, 0x78, 0x53, 0xf1, 0x68, 0x2c, 0x0b, 0x77, 0xc0,
    0xe6, 0x21, 0x40, 0x44, 0xc1, 0x56, 0xc7, 0x55, 0x82, 0x69, 0x39, 0x0a, 0x8a, 0xfa, 0x6d, 0x7c,
];

pub const FIBER_FUNDING_TYPE_ID_MAINNET: [u8; 32] = [
    0xe4, 0x5b, 0x1f, 0x8f, 0x21, 0xbf, 0xf2, 0x31, 0x37, 0x03, 0x5a, 0x3a, 0xb7, 0x51, 0xd7, 0x5b,
    0x36, 0xa9, 0x81, 0xde, 0xec, 0x3e, 0x78, 0x20, 0x19, 0x4b, 0x9c, 0x04, 0x29, 0x67, 0xf4, 0xf1,
];

/// Mock Fiber funding type hash for testing.
/// When set to all-zeros, acts as a wildcard (accepts any channel).
/// Otherwise, must match exactly.
pub const MOCK_FIBER_FUNDING_TYPE_HASH: [u8; 32] = [0u8; 32];

#[derive(Default, PartialEq, Eq)]
enum Branch {
    Order(OrderArgs, OrderData),
    Match(MatchArgs, MatchData),
    #[default]
    Unknown,
}

impl Branch {
    pub fn parse(args: &[u8], data: &[u8]) -> Result<Self> {
        match args.len() {
            ORDER_ARGS_LEN => Ok(Branch::Order(
                ERR!(OrderArgs::from_slice(&args), BadArgsLength)?,
                ERR!(OrderData::from_slice(&data), OrderDataNotSet)?,
            )),
            MATCH_ARGS_LEN => Ok(Branch::Match(
                ERR!(MatchArgs::from_slice(&args), BadArgsLength)?,
                ERR!(MatchData::from_slice(&data), MatchDataNotSet)?,
            )),
            _ => Err(OpticrumError::BadArgsLength.into()),
        }
    }
}

enum OpticrumPattern {
    OrderCancel,
    OrderMatch,
    MatchExtract,
    MatchDestroy,
    Unknown,
}

#[derive(Default)]
struct OpticrumState {
    branch: Branch,
    unoccupied_capacity: u64,
    xudt: Option<(u128, Script)>,
}

impl OpticrumState {
    pub fn compare(&self, another: Option<&OpticrumState>) -> OpticrumPattern {
        match &self.branch {
            Branch::Order(order_args, _) => {
                let Some(another) = another else {
                    return OpticrumPattern::OrderCancel;
                };
                match &another.branch {
                    Branch::Order(_, _) => OpticrumPattern::Unknown,
                    Branch::Match(match_args, _) => {
                        if order_args == &match_args.order_args && self.xudt == another.xudt {
                            OpticrumPattern::OrderMatch
                        } else {
                            OpticrumPattern::Unknown
                        }
                    }
                    _ => OpticrumPattern::Unknown,
                }
            }
            Branch::Match(match_args, _) => {
                let Some(another) = another else {
                    return OpticrumPattern::MatchDestroy;
                };
                match &another.branch {
                    Branch::Order(_, _) => OpticrumPattern::Unknown,
                    Branch::Match(another_match_args, _) => {
                        if match_args == another_match_args && self.xudt == another.xudt {
                            OpticrumPattern::MatchExtract
                        } else {
                            OpticrumPattern::Unknown
                        }
                    }
                    _ => OpticrumPattern::Unknown,
                }
            }
            _ => OpticrumPattern::Unknown,
        }
    }

    pub fn liquidity_rent(&self) -> u64 {
        let Branch::Match(_, match_data) = &self.branch else {
            return 0;
        };
        let base_block = if match_data.last_extraction_block == 0 {
            load_header_block_number(1).unwrap_or_default()
        } else {
            match_data.last_extraction_block
        };
        let tip_block = load_header_block_number(0).unwrap_or_default();
        let elapsed = tip_block.saturating_sub(base_block);
        (match_data.rent_per_block * elapsed as f64) as u64
    }

    pub fn is_exhausted(&self) -> bool {
        let liquidity_rent = self.liquidity_rent();
        if let Some((xudt_amount, _)) = &self.xudt {
            *xudt_amount <= liquidity_rent as u128
        } else {
            self.unoccupied_capacity <= liquidity_rent
        }
    }

    pub fn good_extraction(&self, another: &OpticrumState) -> bool {
        let Branch::Match(_, match_data) = &self.branch else {
            return false;
        };
        let Branch::Match(_, another_match_data) = &another.branch else {
            return false;
        };
        let liquidity_rent = self.liquidity_rent();
        let tip_block = load_header_block_number(0).unwrap_or_default();
        if self.xudt.is_some() {
            match_data.good_extraction(another_match_data, tip_block, liquidity_rent as u128)
        } else {
            match_data.good_extraction(another_match_data, tip_block, 0)
        }
    }
}

/// Context passed through the verification tree.
/// Populated by Root, consumed by branch verifiers.
#[derive(Default)]
struct Context {
    old_state: OpticrumState,
    new_state: Option<OpticrumState>,
}

cinnabar_main!(
    Context,
    (TREE_ROOT, Root),
    ("order_cancel", OrderCancel),
    ("order_match", OrderMatch),
    ("match_extract", MatchExtract),
    ("match_destroy", MatchDestroy),
);
