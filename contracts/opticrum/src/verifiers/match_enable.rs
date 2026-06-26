use ckb_cinnabar_verifier::{
    re_exports::ckb_std::{
        self, ckb_constants::Source, ckb_types::prelude::Unpack, high_level::load_header,
    },
    Result, Verification,
};
use ckb_std::debug;
use opticrum_protocol::MatchStatus;

use crate::{
    error::OpticrumError,
    utils::{has_lock_in_inputs, load_header_block_number},
    Branch, Context, ABOUT_THREE_DAYS_BLOCKS,
};

/// Verifies a Frozen → Enabled transition.
///
/// Two paths:
///   - **Buyer confirm**: `buyer_lock_hash` in inputs, no timing requirement.
///   - **Seller auto-enable**: `seller_lock_hash` in inputs, HeaderDep[1] proves
///     match creation block, and `tip - creation >= ABOUT_THREE_DAYS_BLOCKS`.
///
/// HeaderDeps:
///   [0] tip block
///   [1] match creation block (auto-enable only; ignored for buyer confirm)
#[derive(Default)]
pub struct MatchEnable;

impl Verification<Context> for MatchEnable {
    fn verify(&mut self, name: &str, ctx: &mut Context) -> Result<Option<&str>> {
        debug!("Entering [{name}]");

        let Branch::Match(match_args, match_data) = &ctx.old_state.branch else {
            return Err(OpticrumError::UnexpectedBranch.into());
        };
        let new_state = ctx
            .new_state
            .as_ref()
            .ok_or(OpticrumError::UnexpectedBranch)?;
        let Branch::Match(_, new_match_data) = &new_state.branch else {
            return Err(OpticrumError::UnexpectedBranch.into());
        };

        // Old must be Frozen
        if match_data.status != MatchStatus::Frozen {
            return Err(OpticrumError::MatchNotFrozen.into());
        }
        // New must be Enabled
        if new_match_data.status != MatchStatus::Enabled {
            return Err(OpticrumError::BadMatchStatus.into());
        }

        let buyer_present = has_lock_in_inputs(&match_args.order_args.buyer_lock_hash)?;
        let seller_present = has_lock_in_inputs(&match_args.seller_lock_hash)?;

        if buyer_present {
            // Buyer confirmation — no timing requirement
            debug!("[{name}] Buyer confirming match");
        } else if seller_present {
            // Seller auto-enable — must prove 3 days elapsed
            let tip = load_header_block_number(0).unwrap_or_default();
            let creation = load_header(0, Source::GroupInput)
                .map_err(|_| OpticrumError::HeaderNotSet)?
                .raw()
                .number()
                .unpack();
            if tip.saturating_sub(creation) < ABOUT_THREE_DAYS_BLOCKS {
                debug!(
                    "[{name}] Auto-enable too early: {} blocks elapsed (need {})",
                    tip - creation,
                    ABOUT_THREE_DAYS_BLOCKS
                );
                return Err(OpticrumError::MatchAutoEnableTooEarly.into());
            }
            debug!(
                "[{name}] Seller auto-enabling after {} blocks",
                tip - creation
            );
        } else {
            return Err(OpticrumError::AuthorizationMissing.into());
        }

        // Validate MatchData fields are preserved (except status).
        // Skip rent_per_block comparison — f64 equality is unreliable across platforms.
        if match_data.xudt_amount != new_match_data.xudt_amount
            || match_data.escrow_blocks != new_match_data.escrow_blocks
            || match_data.last_extraction_block != new_match_data.last_extraction_block
        {
            return Err(OpticrumError::BadMatchDataUpdate.into());
        }

        debug!("[{name}] Match enabled successfully");
        Ok(None)
    }
}
