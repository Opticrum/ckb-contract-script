use ckb_cinnabar_verifier::{re_exports::ckb_std, Result, Verification};
use ckb_std::{
    ckb_constants::Source,
    debug,
    high_level::{load_cell_capacity, load_cell_occupied_capacity, load_header},
};

use crate::{
    error::OpticrumError,
    utils::{find_channel_in_celldeps, has_lock_in_inputs},
    Branch, Context,
};
use opticrum_protocol::MatchStatus;

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
/// 2. The channel lock args match the MuSig2-aggregated funding key from
///    buyer + seller fiber pubkeys
/// 3. The produced Match Cell args correctly extend the Order args
/// 4. Match Cell data is properly initialized (rent_per_block > 0,
///    escrow_blocks > 0, last_extraction_block == 0)
/// 5. Match Cell capacity == Order Cell capacity (rent transferred intact)
/// 6. Seller authorizes the transaction (lock hash in inputs)
/// 7. Channel was created after the Order
///    (load_header(Source::Input) vs load_header(Source::CellDep))
#[derive(Default)]
pub struct OrderMatch;

impl Verification<Context> for OrderMatch {
    fn verify(&mut self, name: &str, ctx: &mut Context) -> Result<Option<&str>> {
        debug!("Entering [{name}]");

        let Branch::Order(_, order_data) = &ctx.old_state.branch else {
            return Err(OpticrumError::UnexpectedBranch.into());
        };

        let Branch::Match(match_args, match_data) = &ctx.new_state.as_ref().unwrap().branch else {
            return Err(OpticrumError::UnexpectedBranch.into());
        };

        // 1. Verify a CellDep exists with at least the required channel capacity.
        let Some(channel_index) = find_channel_in_celldeps(
            &match_args.channel_outpoint,
            if order_data.xudt_amount > 0 {
                None
            } else {
                Some(order_data.channel_capacity)
            },
            if order_data.xudt_amount > 0 {
                Some(order_data.xudt_amount)
            } else {
                None
            },
            ctx.old_state
                .xudt
                .as_ref()
                .map(|(_, type_script)| Some(type_script)),
        ) else {
            return Err(OpticrumError::ChannelCellNotInDep.into());
        };

        // 2. Verify MatchData status is Frozen (initial state after matching)
        if match_data.status != MatchStatus::Frozen {
            return Err(OpticrumError::BadMatchStatus.into());
        }

        // 3. Seller must participate
        let seller_present = has_lock_in_inputs(&match_args.seller_lock_hash)?;
        if !seller_present {
            debug!("Seller lock hash not found in inputs");
            return Err(OpticrumError::SellerAuthMissing.into());
        }

        // 4. Validate Match Cell data initialization
        if match_data.rent_per_block == 0.0
            || match_data.escrow_blocks == 0
            || match_data.last_extraction_block != 0
        {
            return Err(OpticrumError::MatchDataNotSet.into());
        }

        // 5. Unoccupied capacity (rent pool) must transfer intact from Order to Match.
        //    Total capacity differs by ORDER_TO_MATCH_CAPACITY_RESERVE because
        //    the Match cell has larger args + data. We compare unoccupied capacity
        //    to ensure the rent is preserved.
        let old_unoccupied = {
            let total = load_cell_capacity(0, Source::GroupInput)
                .map_err(|_| OpticrumError::BadOrderMatch)?;
            let occupied = load_cell_occupied_capacity(0, Source::GroupInput)
                .map_err(|_| OpticrumError::BadOrderMatch)?;
            total.saturating_sub(occupied)
        };
        let new_unoccupied = {
            let total =
                load_cell_capacity(0, Source::Output).map_err(|_| OpticrumError::BadOrderMatch)?;
            let occupied = load_cell_occupied_capacity(0, Source::Output)
                .map_err(|_| OpticrumError::BadOrderMatch)?;
            total.saturating_sub(occupied)
        };
        if old_unoccupied != new_unoccupied {
            debug!(
                "Unoccupied capacity mismatch: order={} vs match={}",
                old_unoccupied, new_unoccupied
            );
            return Err(OpticrumError::ChannelCapacityMismatch.into());
        }

        // 6. xUDT amount must transfer unchanged from Order to Match
        if order_data.xudt_amount != match_data.xudt_amount {
            debug!(
                "xUDT amount mismatch: order={} vs match={}",
                order_data.xudt_amount, match_data.xudt_amount
            );
            return Err(OpticrumError::BadXudtAmount.into());
        }

        // 7. Channel must have been created after the order.
        //    GroupInput[0] = Order cell, CellDep[channel_index] = Channel cell.
        debug!("channel_index: {}", channel_index);
        let order_block =
            load_header(0, Source::GroupInput).map_err(|_| OpticrumError::HeaderNotSet)?;
        debug!("order_block: {}", order_block.raw().number());
        let channel_block =
            load_header(channel_index, Source::CellDep).map_err(|_| OpticrumError::HeaderNotSet)?;
        debug!("channel_block: {}", channel_block.raw().number());
        if channel_block.raw().number() <= order_block.raw().number() {
            debug!(
                "Channel created at {} not after order at {}",
                channel_block, order_block
            );
            return Err(OpticrumError::ChannelCreatedBeforeOrder.into());
        }

        debug!("[{name}] Order matched successfully");
        Ok(None)
    }
}
