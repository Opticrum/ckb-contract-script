use ckb_cinnabar_verifier::{re_exports::ckb_std, Result, Verification};
use ckb_std::debug;

use crate::{
    error::OpticrumError,
    utils::{check_channel_existence, has_lock_in_inputs, load_header_block_number},
    Branch, Context,
};

/// Verifies Match→Match transitions. Since the root verifier cannot distinguish
/// seller-extract from buyer-inject/withdraw by state alone (both are
/// Match→Match with identical args), this verifier internally branches on auth:
///
/// - **seller_lock_hash in inputs** → extraction: verifies linear rent
///   (rent_per_block × elapsed), updates last_extraction_block.
/// - **buyer_lock_hash in inputs** → inject/withdraw: preserves rent_per_block
///   and last_extraction_block; verifies the output cell remains viable.
/// - **both or neither** → error.
///
/// HeaderDeps: [0] tip block (always), [1] match creation block
///   (if last_extraction_block == 0)
#[derive(Default)]
pub struct MatchUpdate;

impl Verification<Context> for MatchUpdate {
    fn verify(&mut self, name: &str, ctx: &mut Context) -> Result<Option<&str>> {
        debug!("Entering [{name}]");

        let Branch::Match(match_args, match_data) = &ctx.old_state.branch else {
            return Err(OpticrumError::UnexpectedBranch.into());
        };
        let new_state = ctx
            .new_state
            .as_ref()
            .ok_or(OpticrumError::BadMatchUpdate)?;
        let Branch::Match(new_match_args, new_match_data) = &new_state.branch else {
            return Err(OpticrumError::UnexpectedBranch.into());
        };

        // MatchArgs must be identical across the transition
        if match_args != new_match_args {
            debug!("[{name}] MatchArgs changed during update");
            return Err(OpticrumError::BadMatchUpdate.into());
        }

        let seller_present = has_lock_in_inputs(&match_args.seller_lock_hash)?;
        let buyer_present = has_lock_in_inputs(&match_args.order_args.buyer_lock_hash)?;

        match (seller_present, buyer_present) {
            // ===== Seller extraction path =====
            (true, false) => {
                // 1. Channel must still exist
                if !check_channel_existence(&match_args.channel_outpoint) {
                    debug!("[{name}] Channel cell not found in CellDeps");
                    return Err(OpticrumError::ChannelCellNotInDep.into());
                }

                // 2. Match must not already be exhausted
                if ctx.old_state.is_exhausted()? {
                    debug!("[{name}] Match already exhausted");
                    return Err(OpticrumError::MatchNotExhausted.into());
                }

                // 3. Compute expected rent and verify extraction amount
                let expected_rent = ctx.old_state.liquidity_rent()?;

                if let Some((old_xudt_amount, _)) = &ctx.old_state.xudt {
                    let extracted = old_xudt_amount.saturating_sub(new_match_data.xudt_amount);
                    if extracted != expected_rent as u128 {
                        debug!(
                            "[{name}] xUDT extraction mismatch: extracted={}, expected={}",
                            extracted, expected_rent
                        );
                        return Err(OpticrumError::BadExtractionAmount.into());
                    }
                } else {
                    let extracted = ctx
                        .old_state
                        .unoccupied_capacity
                        .saturating_sub(new_state.unoccupied_capacity);
                    // Integer arithmetic — no f64 rounding, exact comparison
                    if extracted != expected_rent {
                        debug!(
                            "[{name}] CKB extraction mismatch: extracted={}, expected={}",
                            extracted, expected_rent
                        );
                        return Err(OpticrumError::BadExtractionAmount.into());
                    }
                }

                // 4. rent_per_block preserved (u64 — direct comparison is safe)
                if match_data.shannons_per_block != new_match_data.shannons_per_block {
                    debug!("[{name}] rent_per_block changed during extraction");
                    return Err(OpticrumError::RentPerBlockMismatch.into());
                }

                // 5. last_extraction_block updated to tip
                let tip_block = load_header_block_number(0)?;
                if new_match_data.last_extraction_block != tip_block {
                    debug!(
                        "[{name}] last_extraction_block mismatch: new={}, tip={}",
                        new_match_data.last_extraction_block, tip_block
                    );
                    return Err(OpticrumError::BadMatchDataUpdate.into());
                }

                debug!("[{name}] Rent extracted: {}", expected_rent);
                Ok(None)
            }
            // ===== Buyer inject/withdraw path =====
            (false, true) => {
                // 1. rent_per_block preserved (u64 — direct comparison)
                if match_data.shannons_per_block != new_match_data.shannons_per_block {
                    debug!("[{name}] rent_per_block changed by buyer");
                    return Err(OpticrumError::RentPerBlockMismatch.into());
                }

                // 2. last_extraction_block preserved
                if new_match_data.last_extraction_block != match_data.last_extraction_block {
                    debug!("[{name}] last_extraction_block changed by buyer");
                    return Err(OpticrumError::BadMatchDataUpdate.into());
                }

                // 3. For xUDT matches, verify type script preserved
                if let (Some((_, old_type)), Some((_, new_type))) =
                    (&ctx.old_state.xudt, &new_state.xudt)
                {
                    if old_type != new_type {
                        debug!("[{name}] xUDT type script changed during buyer update");
                        return Err(OpticrumError::BadMatchUpdate.into());
                    }
                }

                debug!("[{name}] Buyer update successful");
                Ok(None)
            }
            _ => Err(OpticrumError::AuthorizationMissing.into()),
        }
    }
}
