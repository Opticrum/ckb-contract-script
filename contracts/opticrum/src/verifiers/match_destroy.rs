use ckb_cinnabar_verifier::{re_exports::ckb_std, Result, Verification};
use ckb_std::debug;
use opticrum_protocol::MatchStatus;

use crate::{error::OpticrumError, utils::has_lock_in_inputs, Branch, Context};

/// Verifies that a Match Cell can be destroyed.
///
/// Authorization depends on match status:
///   - **Frozen**: cannot be destroyed at all.
///   - **Discarded**: only the seller can destroy.
///   - **Enabled**: must be exhausted (accumulated rent >= remaining value);
///     seller or buyer can sweep.
///
/// HeaderDeps: [0] tip block, [1] match creation block (if never extracted)
#[derive(Default)]
pub struct MatchDestroy;

impl Verification<Context> for MatchDestroy {
    fn verify(&mut self, name: &str, ctx: &mut Context) -> Result<Option<&str>> {
        debug!("Entering [{name}]");

        let Branch::Match(match_args, match_data) = &ctx.old_state.branch else {
            return Err(OpticrumError::UnexpectedBranch.into());
        };

        match match_data.status {
            s if s == MatchStatus::Frozen => {
                // Frozen matches cannot be destroyed
                debug!("[{name}] Cannot destroy a Frozen match");
                return Err(OpticrumError::MatchNotExhausted.into());
            }
            s if s == MatchStatus::Discarded => {
                // Only seller can destroy Discarded matches
                let seller_present = has_lock_in_inputs(&match_args.seller_lock_hash)?;
                if !seller_present {
                    debug!("[{name}] Only seller can destroy Discarded match");
                    return Err(OpticrumError::SellerAuthMissing.into());
                }
            }
            s if s == MatchStatus::Enabled => {
                // Must be exhausted for Enabled matches
                if !ctx.old_state.is_exhausted() {
                    return Err(OpticrumError::MatchNotExhausted.into());
                }
                // Seller or Buyer can destroy exhausted Enabled matches
                let seller_present = has_lock_in_inputs(&match_args.seller_lock_hash)?;
                let buyer_present = has_lock_in_inputs(&match_args.order_args.buyer_lock_hash)?;
                if !seller_present && !buyer_present {
                    debug!("[{name}] Neither seller nor buyer authorized");
                    return Err(OpticrumError::AuthorizationMissing.into());
                }
            }
            _ => {
                return Err(OpticrumError::BadMatchStatus.into());
            }
        }

        debug!("[{name}] Match destroyed successfully");
        Ok(None)
    }
}
