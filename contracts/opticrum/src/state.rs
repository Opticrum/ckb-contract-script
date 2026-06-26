//! State types and transition routing for Opticrum cells.
//!
//! Defines the branch-specific data (Order vs Match), the state comparison
//! engine that routes transitions, and the `Context` that flows through
//! the verification tree.

use ckb_cinnabar_verifier::{
    re_exports::ckb_std::{
        ckb_constants::Source,
        ckb_types::{packed::Script, prelude::Unpack},
        high_level::load_header,
    },
    Result,
};
use opticrum_protocol::{
    MatchArgs, MatchData, OrderArgs, OrderData, MATCH_ARGS_LEN, ORDER_ARGS_LEN,
};

use crate::{error::OpticrumError, utils::load_header_block_number, ERR};

// ---------------------------------------------------------------------------
// Branch — discriminates Order vs Match from raw lock args length
// ---------------------------------------------------------------------------

#[derive(Default, PartialEq, Eq)]
pub enum Branch {
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

// ---------------------------------------------------------------------------
// OpticrumPattern — routes the root verifier to the correct branch verifier
// ---------------------------------------------------------------------------

/// Routes the Root verifier to the correct branch verifier.
/// No status-based discrimination — Match→Match always routes to MatchUpdate
/// (which internally branches on auth: seller→extract, buyer→inject/withdraw).
pub enum OpticrumPattern {
    OrderCancel,
    OrderMatch,
    MatchUpdate,
    MatchDestroy,
    Unknown,
}

// ---------------------------------------------------------------------------
// OpticrumState — parsed cell state extracted by the root verifier
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct OpticrumState {
    pub branch: Branch,
    pub unoccupied_capacity: u64,
    pub xudt: Option<(u128, Script)>,
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

    /// Compute accumulated linear rent.
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

// ---------------------------------------------------------------------------
// Context — flows through the verification tree
// ---------------------------------------------------------------------------

/// Context passed through the verification tree.
/// Populated by Root, consumed by branch verifiers.
#[derive(Default)]
pub struct Context {
    pub old_state: OpticrumState,
    pub new_state: Option<OpticrumState>,
}

impl Context {
    /// Destructure the old state as an Order branch, or return `UnexpectedBranch`.
    pub fn expect_old_order(&self) -> Result<(&OrderArgs, &OrderData)> {
        match &self.old_state.branch {
            Branch::Order(args, data) => Ok((args, data)),
            _ => Err(OpticrumError::UnexpectedBranch.into()),
        }
    }

    /// Destructure the old state as a Match branch, or return `UnexpectedBranch`.
    pub fn expect_old_match(&self) -> Result<(&MatchArgs, &MatchData)> {
        match &self.old_state.branch {
            Branch::Match(args, data) => Ok((args, data)),
            _ => Err(OpticrumError::UnexpectedBranch.into()),
        }
    }

    /// Destructure the new state as a Match branch.
    ///
    /// Returns `BadMatchUpdate` if `new_state` is `None` (should not happen for
    /// Match→Match transitions), or `UnexpectedBranch` if it's the wrong branch.
    pub fn expect_new_match(&self) -> Result<(&MatchArgs, &MatchData)> {
        let state = self
            .new_state
            .as_ref()
            .ok_or(OpticrumError::BadMatchUpdate)?;
        match &state.branch {
            Branch::Match(args, data) => Ok((args, data)),
            _ => Err(OpticrumError::UnexpectedBranch.into()),
        }
    }
}
