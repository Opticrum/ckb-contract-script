use ckb_cinnabar_verifier::{re_exports::ckb_std, Result, Verification};
use ckb_std::{ckb_constants::Source, debug, high_level::{load_cell_capacity, load_cell_occupied_capacity}};

use crate::{
    error::OpticrumError,
    utils::{find_channel_in_celldeps, has_lock_in_inputs},
    Branch, Context,
};

/// Verifies that the seller correctly extracts rent from a Match Cell.
///
/// The seller submits a transaction with:
///   - HeaderDeps: [0] tip block, [1] match creation block
///   - Input: Match Cell (Opticrum lock)
///   - Output: Updated Match Cell (reduced capacity, updated last_extraction_block)
///
/// Checks:
/// 1. Transaction includes seller's lock (seller authorized)
/// 2. Extraction amount == rent_per_block × elapsed (linear)
/// 3. If accumulated rent >= remaining capacity: match is exhausted,
///    seller gets everything (no Match output produced)
/// 4. Updated Match Cell data is correct (only last_extraction_block changes)
/// 5. Match args unchanged
#[derive(Default)]
pub struct MatchExtract;

impl Verification<Context> for MatchExtract {
    fn verify(&mut self, name: &str, ctx: &mut Context) -> Result<Option<&str>> {
        debug!("Entering [{name}]");

        let Branch::Match(match_args, _) = &ctx.old_state.branch else {
            return Err(OpticrumError::UnexpectedBranch.into());
        };

        // 1. Verify channel cell still exists in CellDeps (existence only —
        //    capacity/xUDT amount was already verified at match time).
        if !find_channel_in_celldeps(
            &match_args.channel_outpoint,
            None,
            None,
            ctx.old_state
                .xudt
                .as_ref()
                .map(|(_, type_script)| Some(type_script)),
        ) {
            return Err(OpticrumError::ChannelCellNotInDep.into());
        }

        // 2. Seller must participate
        let seller_present = has_lock_in_inputs(&match_args.seller_lock_hash)?;
        if !seller_present {
            debug!("Seller lock hash not found in inputs");
            return Err(OpticrumError::SellerAuthMissing.into());
        }

        // 3. If match is already exhausted, reject extraction — use destroy instead
        if ctx.old_state.is_exhausted() {
            return Err(OpticrumError::MatchAlreadyExhausted.into());
        }

        // 4. Validate extraction amount matches linear rent
        let expected_rent = ctx.old_state.liquidity_rent();
        let new_state = ctx.new_state.as_ref().unwrap();
        if let Some((old_xudt, _)) = &ctx.old_state.xudt {
            let Branch::Match(_, new_match_data) = &new_state.branch else {
                return Err(OpticrumError::UnexpectedBranch.into());
            };
            let extracted = old_xudt.saturating_sub(new_match_data.xudt_amount);
            if extracted != expected_rent as u128 {
                return Err(OpticrumError::BadExtractionAmount.into());
            }
        } else {
            let extracted = ctx
                .old_state
                .unoccupied_capacity
                .saturating_sub(new_state.unoccupied_capacity);
            if extracted != expected_rent {
                return Err(OpticrumError::BadExtractionAmount.into());
            }
        }

        // 5. Validate MatchData fields are correctly updated
        if !ctx.old_state.good_extraction(new_state) {
            return Err(OpticrumError::BadMatchDataUpdate.into());
        }

        // 6. Guard: output cell must remain viable (capacity >= occupied).
        //    Full extraction should use destroy_match instead.
        let out_cap = load_cell_capacity(0, Source::Output)
            .map_err(|_| OpticrumError::BadExtractionAmount)?;
        let out_occ = load_cell_occupied_capacity(0, Source::Output)
            .map_err(|_| OpticrumError::BadExtractionAmount)?;
        if out_cap < out_occ {
            debug!("Extraction would leave cell underfunded");
            return Err(OpticrumError::BadExtractionAmount.into());
        }

        debug!("[{name}] Rent extracted successfully");
        Ok(None)
    }
}
