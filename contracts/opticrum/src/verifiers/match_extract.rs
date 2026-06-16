use ckb_cinnabar_verifier::{re_exports::ckb_std, Result, Verification};
use ckb_std::debug;

use crate::{
    error::OpticrumError,
    utils::{find_channel_in_celldeps, has_lock_in_inputs},
    Branch, Context,
};

/// Verifies that the seller correctly extracts rent from a Match Cell.
///
/// The seller submits a transaction with:
///   - HeaderDeps: [0] match creation block, [1] tip block
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
            return Err(OpticrumError::BadArgsLength.into());
        };

        // 1. Verify a CellDep exists with at least the required channel capacity.
        if !find_channel_in_celldeps(
            &match_args.channel_outpoint,
            u64::MAX,
            u128::MAX,
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

        // 3. Calculate linear rent and check if match is exhausted
        if ctx.old_state.is_exhausted() {
            return Err(OpticrumError::MatchAlreadyExpired.into());
        }

        // 4. Handle exhausted vs normal extraction
        if ctx
            .old_state
            .good_extraction(ctx.new_state.as_ref().unwrap())
        {
            return Err(OpticrumError::MatchAlreadyExpired.into());
        }

        debug!("[{name}] Rent extracted successfully");
        Ok(None)
    }
}
