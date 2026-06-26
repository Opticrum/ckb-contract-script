use ckb_cinnabar_verifier::{re_exports::ckb_std, Result, Verification};
use ckb_std::debug;

use crate::{error::OpticrumError, state::Context, utils::require_input_lock};

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

        let (match_args, _) = ctx.expect_old_match()?;

        // Only seller can destroy
        require_input_lock(
            name,
            &match_args.seller_lock_hash,
            OpticrumError::SellerAuthMissing,
        )?;

        // Must be exhausted
        if !ctx.old_state.is_exhausted()? {
            debug!("[{name}] Match not exhausted, cannot destroy");
            return Err(OpticrumError::MatchNotExhausted.into());
        }

        debug!("[{name}] Match destroyed successfully");
        Ok(None)
    }
}
