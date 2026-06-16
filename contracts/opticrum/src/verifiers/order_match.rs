use ckb_cinnabar_verifier::{re_exports::ckb_std, Result, Verification};
use ckb_std::debug;

use crate::{
    error::OpticrumError,
    utils::{find_channel_in_celldeps, has_lock_in_inputs},
    Branch, Context,
};

/// Verifies that a seller has properly matched an Order Cell.
///
/// The two-step flow:
///   1. Seller already created a channel via Fiber API → Channel Cell exists on-chain
///   2. Seller submits this tx: consumes Order Cell, references Channel Cell as CellDep,
///      produces a Match Cell.
///
/// Checks:
/// 1. The Channel Cell identified by channel_outpoint exists in CellDeps
///    and has capacity >= order_data.channel_capacity
/// 2. The produced Match Cell args correctly extend the Order args
/// 3. Match Cell data is properly initialized (rent_per_block > 0,
///    escrow_blocks > 0, last_extraction_block == 0)
/// 4. Match Cell capacity == Order Cell capacity (rent transferred intact)
/// 5. Seller authorizes the transaction (lock hash in inputs)
#[derive(Default)]
pub struct OrderMatch;

impl Verification<Context> for OrderMatch {
    fn verify(&mut self, name: &str, ctx: &mut Context) -> Result<Option<&str>> {
        debug!("Entering [{name}]");

        let Branch::Order(_, order_data) = &ctx.old_state.branch else {
            return Err(OpticrumError::BadArgsLength.into());
        };

        let Branch::Match(match_args, match_data) = &ctx.new_state.as_ref().unwrap().branch else {
            return Err(OpticrumError::BadArgsLength.into());
        };

        // 1. Verify a CellDep exists with at least the required channel capacity.
        let channel_obligated = find_channel_in_celldeps(
            &match_args.channel_outpoint,
            order_data.channel_capacity,
            order_data.xudt_amount,
            Some(
                ctx.old_state
                    .xudt
                    .as_ref()
                    .map(|(_, type_script)| type_script),
            ),
        );
        if !channel_obligated {
            return Err(OpticrumError::ChannelCellNotInDep.into());
        }

        // 2. Seller must participate
        let seller_present = has_lock_in_inputs(&match_args.seller_lock_hash)?;
        if !seller_present {
            debug!("Seller lock hash not found in inputs");
            return Err(OpticrumError::SellerAuthMissing.into());
        }

        // 3. Validate Match Cell data initialization
        if match_data.rent_per_block == 0.0
            || match_data.escrow_blocks == 0
            || match_data.last_extraction_block != 0
        {
            return Err(OpticrumError::MatchDataNotSet.into());
        }

        // 4. Capacity check: rent must transfer fully from Order to Match
        if ctx.old_state.unoccupied_capacity != ctx.new_state.as_ref().unwrap().unoccupied_capacity
            || order_data.xudt_amount != match_data.xudt_amount
        {
            debug!(
                "Capacity mismatch: order={} vs match={}, xudt_amount mismatch: {} vs {}",
                ctx.old_state.unoccupied_capacity,
                ctx.new_state.as_ref().unwrap().unoccupied_capacity,
                order_data.xudt_amount,
                match_data.xudt_amount
            );
            return Err(OpticrumError::BadOrderMatch.into());
        }

        debug!("[{name}] Order matched successfully");
        Ok(None)
    }
}
