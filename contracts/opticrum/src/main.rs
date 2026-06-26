#![no_main]
#![no_std]

use ckb_cinnabar_verifier::{
    cinnabar_main,
    re_exports::ckb_std::{
        self,
        ckb_constants::Source,
        ckb_types::{packed::Script, prelude::Unpack},
        high_level::load_header,
    },
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

/// Mock Fiber funding type hash for integration tests.
/// Matches the channel type script seeded in `tests/tests/opticrum_tests.rs`
/// (`code_hash = [0xCC; 32]`, `hash_type = Data1`, empty args).
pub const FIBER_FUNDING_TYPE_ID_MOCK: [u8; 32] = [
    0x77, 0xc9, 0x16, 0x3a, 0xdd, 0xbf, 0x87, 0xc8, 0x05, 0xbe, 0x3b, 0x6c, 0x85, 0x69, 0xb8, 0xe0,
    0x15, 0xa4, 0xca, 0x0e, 0xf3, 0xc6, 0x89, 0x15, 0x02, 0x34, 0xf0, 0xc8, 0x02, 0xa7, 0x69, 0x00,
];

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
                ERR!(OrderArgs::from_slice(args), BadArgsLength)?,
                ERR!(OrderData::from_slice(data), OrderDataNotSet)?,
            )),
            MATCH_ARGS_LEN => Ok(Branch::Match(
                ERR!(MatchArgs::from_slice(args), BadArgsLength)?,
                ERR!(MatchData::from_slice(data), MatchDataNotSet)?,
            )),
            _ => Err(OpticrumError::BadArgsLength.into()),
        }
    }
}

/// Routes the Root verifier to the correct branch verifier.
/// No status-based discrimination — Match→Match always routes to MatchUpdate
/// (which internally branches on auth: seller→extract, buyer→inject/withdraw).
enum OpticrumPattern {
    OrderCancel,
    OrderMatch,
    MatchUpdate,
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
                            OpticrumPattern::MatchUpdate
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

    /// Compute accumulated linear rent
    pub fn liquidity_rent(&self) -> Result<u64> {
        let Branch::Match(_, match_data) = &self.branch else {
            return Err(OpticrumError::UnexpectedBranch.into());
        };
        let base_block = if match_data.last_extraction_block == 0 {
            load_header(0, Source::GroupInput)
                .map_err(|_| OpticrumError::HeaderNotSet)?
                .raw()
                .number()
                .unpack()
        } else {
            match_data.last_extraction_block
        };
        let tip_block = load_header_block_number(0).map_err(|_| OpticrumError::HeaderNotSet)?;
        let elapsed = tip_block.saturating_sub(base_block);
        Ok(match_data.shannons_per_block * elapsed)
    }

    pub fn is_exhausted(&self) -> Result<bool> {
        let liquidity_rent = self.liquidity_rent()?;
        if let Some((xudt_amount, _)) = &self.xudt {
            Ok(*xudt_amount <= liquidity_rent as u128)
        } else {
            Ok(self.unoccupied_capacity <= liquidity_rent)
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
    ("match_update", MatchUpdate),
    ("match_destroy", MatchDestroy),
);
