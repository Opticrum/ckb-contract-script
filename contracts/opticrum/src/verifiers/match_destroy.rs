use ckb_cinnabar_verifier::{re_exports::ckb_std, Result, Verification};
use ckb_std::debug;

use crate::{error::OpticrumError, utils::has_lock_in_inputs, Branch, Context};

/// Verifies that a Match Cell can be destroyed.
///
/// Only the seller can destroy, and only when the match is exhausted
/// (accumulated rent >= remaining value).
///
/// HeaderDeps: [0] tip block, [1] match creation block (if never extracted)
#[derive(Default)]
pub struct MatchDestroy;

impl Verification<Context> for MatchDestroy {
    fn verify(&mut self, name: &str, ctx: &mut Context) -> Result<Option<&str>> {
        debug!("Entering [{name}]");

        let Branch::Match(match_args, _) = &ctx.old_state.branch else {
            return Err(OpticrumError::UnexpectedBranch.into());
        };

        // Only seller can destroy
        let seller_present = has_lock_in_inputs(&match_args.seller_lock_hash)?;
        if !seller_present {
            debug!("[{name}] Only seller can destroy a match");
            return Err(OpticrumError::SellerAuthMissing.into());
        }

        // Must be exhausted
        if !ctx.old_state.is_exhausted()? {
            debug!("[{name}] Match not exhausted, cannot destroy");
            return Err(OpticrumError::MatchNotExhausted.into());
        }

        debug!("[{name}] Match destroyed successfully");
        Ok(None)
    }
}
