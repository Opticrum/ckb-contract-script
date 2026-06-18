use ckb_cinnabar_verifier::{re_exports::ckb_std, Result, Verification};
use ckb_std::debug;

use crate::{error::OpticrumError, utils::has_lock_in_inputs, Branch, Context};

/// Verifies that the buyer can cancel their unmatched Order Cell.
///
/// Checks:
/// 1. The transaction includes an input cell with the buyer's lock
///    (proving buyer authorized it)
#[derive(Default)]
pub struct OrderCancel;

impl Verification<Context> for OrderCancel {
    fn verify(&mut self, name: &str, ctx: &mut Context) -> Result<Option<&str>> {
        debug!("Entered [{name}]");

        let Branch::Order(order_args, _) = &ctx.old_state.branch else {
            return Err(OpticrumError::UnexpectedBranch.into());
        };

        // Verify that the buyer participates in this transaction —
        // the buyer's own lock script on their input cell handles
        // signature verification.
        let buyer_present = has_lock_in_inputs(&order_args.buyer_lock_hash)?;
        if !buyer_present {
            debug!("[{name}] Buyer lock hash not found in inputs");
            return Err(OpticrumError::BuyerAuthMissing.into());
        }

        debug!("[{name}] Order cancelled successfully");
        Ok(None)
    }
}
