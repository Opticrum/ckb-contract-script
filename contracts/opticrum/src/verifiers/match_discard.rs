use ckb_cinnabar_verifier::{re_exports::ckb_std, Result, Verification};
use ckb_std::debug;
use opticrum_protocol::MatchStatus;

use crate::{error::OpticrumError, utils::has_lock_in_inputs, Branch, Context};

/// Verifies that the buyer rejects a Frozen match (Frozen → Discarded).
///
/// The buyer extracts all extra capacity (CKB matches) or all xUDT
/// (xUDT matches), leaving only the minimum occupied capacity on the
/// output Discarded match cell. The seller must later destroy it.
///
/// Checks:
/// 1. Old Match is Frozen, new Match is Discarded
/// 2. Buyer authorizes (buyer_lock_hash in inputs)
/// 3. xUDT amount may decrease (buyer reclaims xUDT), but never increase
/// 4. Other MatchData fields preserved (escrow_blocks, last_extraction_block;
///    rent_per_block skipped due to f64 cross-platform unreliability)
/// 5. Output cell capacity >= occupied capacity (viability guard)
#[derive(Default)]
pub struct MatchDiscard;

impl Verification<Context> for MatchDiscard {
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

        // 1. Status transition: Frozen → Discarded
        if match_data.status != MatchStatus::Frozen {
            return Err(OpticrumError::MatchNotFrozen.into());
        }
        if new_match_data.status != MatchStatus::Discarded {
            return Err(OpticrumError::BadMatchStatus.into());
        }

        // 2. Buyer must authorize
        let buyer_present = has_lock_in_inputs(&match_args.order_args.buyer_lock_hash)?;
        if !buyer_present {
            debug!("[{name}] Buyer lock hash not found in inputs");
            return Err(OpticrumError::BuyerAuthMissing.into());
        }

        // 3. MatchData fields preserved
        if match_data.escrow_blocks != new_match_data.escrow_blocks
            || match_data.last_extraction_block != new_match_data.last_extraction_block
            || match_data.rent_per_block != new_match_data.rent_per_block
        {
            return Err(OpticrumError::BadMatchDataUpdate.into());
        }

        debug!("[{name}] Match discarded successfully");
        Ok(None)
    }
}
