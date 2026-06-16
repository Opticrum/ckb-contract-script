use ckb_cinnabar_verifier::{re_exports::ckb_std, Result, Verification};
use ckb_std::debug;

use crate::{error::OpticrumError, utils::has_lock_in_inputs, Branch, Context};

/// Verifies that a Match Cell can be destroyed after exhaustion.
///
/// Anyone can destroy a Match Cell once the accumulated linear rent
/// meets or exceeds the remaining capacity. This is the safety valve:
/// if the seller abandons the match, the liquidity buyer (or any third
/// party) can sweep remaining funds back.
///
/// The destroy transaction provides:
///   - HeaderDeps: [0] match creation block, [1] tip block
///   - Input: Match Cell (Opticrum lock, Burn pattern)
///
/// Checks:
/// 1. Accumulated rent >= remaining capacity (exhausted)
#[derive(Default)]
pub struct MatchDestroy;

impl Verification<Context> for MatchDestroy {
    fn verify(&mut self, name: &str, ctx: &mut Context) -> Result<Option<&str>> {
        debug!("Entering [{name}]");

        let Branch::Match(match_args, _) = &ctx.old_state.branch else {
            return Err(OpticrumError::BadArgsLength.into());
        };

        // 1. Check if match is exhausted
        if ctx.old_state.is_exhausted() {
            return Err(OpticrumError::MatchAlreadyExpired.into());
        }

        // 2. Seller or Buyer must participate
        let seller_present = has_lock_in_inputs(&match_args.seller_lock_hash)?;
        let buyer_present = has_lock_in_inputs(&match_args.order_args.buyer_lock_hash)?;
        if !seller_present && !buyer_present {
            debug!("Seller or Buyer lock hash not found in inputs");
            return Err(OpticrumError::SellerAuthMissing.into());
        }

        debug!("[{name}] Match destroyed successfully");
        Ok(None)
    }
}
