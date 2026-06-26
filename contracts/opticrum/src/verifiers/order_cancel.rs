use ckb_cinnabar_verifier::{re_exports::ckb_std, Result, Verification};
use ckb_std::debug;

use crate::{error::OpticrumError, state::Context, utils::require_input_lock};

/// Verifies that the buyer can cancel their unmatched Order Cell.
///
/// Checks:
/// 1. The transaction includes an input cell with the buyer's lock
///    (proving buyer authorized it)
#[derive(Default)]
pub struct OrderCancel;

impl Verification<Context> for OrderCancel {
    fn verify(&mut self, name: &str, ctx: &mut Context) -> Result<Option<&str>> {
        debug!("Entering [{name}]");

        let (order_args, _) = ctx.expect_old_order()?;

        // Verify that the buyer participates in this transaction —
        // the buyer's own lock script on their input cell handles
        // signature verification.
        require_input_lock(
            name,
            &order_args.buyer_lock_hash,
            OpticrumError::BuyerAuthMissing,
        )?;

        debug!("[{name}] Order cancelled successfully");
        Ok(None)
    }
}
