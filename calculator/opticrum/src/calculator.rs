//! High-level `Instruction` builders for Opticrum transactions.
//!
//! These compose ckb-cinnabar's basic `Operation`s into ready-to-use
//! transaction recipes. Application developers call these without
//! understanding the underlying cell structure.

use ckb_cinnabar_calculator::{
    address::Address,
    instruction::Instruction,
    operation::{
        basic::{
            AddCellDep, AddHeaderDepByBlockNumber, AddInputCellByAddress, AddInputCellByOutPoint,
            AddOutputCell, AddOutputCellByInputIndex, CapacityAdjustment,
        },
        udt::AddXudtCelldep,
        Operation,
    },
    re_exports::ckb_types::{core::DepType, packed::Script},
    rpc::RPC,
};

use crate::{
    config::ORDER_TO_MATCH_CAPACITY_RESERVE,
    operation::{opticrum_lock, AddOpticrumContractCelldep},
    types::{AnnualYield, MatchArgs, MatchData, MatchInfo, OrderArgs, OrderData, OrderInfo},
};

// ---------------------------------------------------------------------------
// 1. Create Order — buyer offers rent for inbound liquidity
// ---------------------------------------------------------------------------

/// Creates an Order Cell on-chain.
///
/// # Transaction Structure
///
/// ```text
/// CellDeps:
///   [0] Opticrum contract code cell (resolved via type_id)
///
/// Inputs:
///   [0] Buyer's cell (must match buyer_lock_hash in Order args)
///       lock:   buyer's personal lock (provides signature/witness)
///
/// Outputs:
///   [0] Order Cell
///       lock:   Opticrum (ORDER_ARGS_LEN-byte args)
///       type:   none / xUDT type script
///       data:   OrderData (ORDER_DATA_LEN bytes: xudt_amount + channel_capacity + escrow_blocks)
///       capacity: rent_capacity (+ ORDER_TO_MATCH_CAPACITY_RESERVE for CKB) so
///                 Order→Match can use Keep without seller CKB; xUDT adds only the reserve
/// ```
///
/// The Order Cell is created with the Opticrum lock script. The lock does NOT
/// execute on creation (ScriptPattern::Create is rejected), so no verification
/// runs at this point. The buyer's own lock on Inputs[0] handles signing.
pub fn create_order<T: RPC>(
    buyer: Address,
    order_args: &OrderArgs,
    order_data: &OrderData,
    annual_yield: AnnualYield,
    xudt_type_script: Option<Script>,
) -> Instruction<T> {
    let args = order_args.to_bytes().to_vec();

    // Compute the actual values from yield
    let xudt_amount = annual_yield.to_xudt(order_data);
    let rent_capacity = annual_yield.to_ckb(order_data);

    // Build the complete OrderData to store on-chain
    let stored_order_data = OrderData::new(
        if xudt_type_script.is_some() {
            xudt_amount
        } else {
            0
        },
        order_data.channel_capacity,
        order_data.escrow_blocks,
    );

    let mut operations: Vec<Box<dyn Operation<T>>> = vec![
        Box::new(AddOpticrumContractCelldep {}),
        Box::new(AddInputCellByAddress {
            address: buyer.clone(),
        }),
    ];
    if xudt_type_script.is_some() {
        operations.push(Box::new(AddXudtCelldep {}));
        operations.push(Box::new(AddOutputCell {
            lock_script: opticrum_lock(args),
            type_script: xudt_type_script.map(|x| x.into()),
            data: stored_order_data.to_bytes().to_vec(),
            capacity: ORDER_TO_MATCH_CAPACITY_RESERVE,
            absolute_capacity: false,
            type_id: false,
        }));
    } else {
        operations.push(Box::new(AddOutputCell {
            lock_script: opticrum_lock(args),
            type_script: None,
            data: stored_order_data.to_bytes().to_vec(),
            capacity: rent_capacity + ORDER_TO_MATCH_CAPACITY_RESERVE,
            absolute_capacity: false,
            type_id: false,
        }));
    }

    Instruction::new(operations)
}

// ---------------------------------------------------------------------------
// 2. Cancel Order — buyer reclaims unmatched order
// ---------------------------------------------------------------------------

/// Cancels an unmatched Order Cell, returning capacity to the buyer.
///
/// # Transaction Structure
///
/// ```text
/// CellDeps:
///   [0] Opticrum contract code cell (resolved via type_id)
///
/// Inputs:
///   [0] Order Cell (consumed — ScriptPattern::Burn)
///       lock:   Opticrum (ORDER_ARGS_LEN-byte Order args)
///       data:   OrderData (ORDER_DATA_LEN bytes)
///       capacity: original rent_capacity
///   [1] Buyer's cell (must match buyer_lock_hash in Order args)
///       lock:   buyer's personal lock (provides signature/witness)
/// ```
///
/// The Order Cell is burned (appears only in inputs), routing to OrderCancel
/// verifier. The verifier checks that Inputs[1]'s lock hash matches the
/// `buyer_lock_hash` embedded in the Order args — proving the buyer
/// authorized the cancellation.
pub fn cancel_order<T: RPC>(buyer: Address, order_info: OrderInfo) -> Instruction<T> {
    let mut operations: Vec<Box<dyn Operation<T>>> = vec![
        Box::new(AddOpticrumContractCelldep {}),
        Box::new(AddInputCellByOutPoint {
            tx_hash: order_info.order_outpoint.tx_hash.into(),
            index: order_info.order_outpoint.index,
            since: None,
        }),
        Box::new(AddInputCellByAddress {
            address: buyer.clone(),
        }),
    ];

    if let Some(x) = order_info.xudt {
        operations.push(Box::new(AddXudtCelldep {}));
        operations.push(Box::new(AddOutputCell {
            lock_script: buyer.into(),
            type_script: Some(x.type_script.into()),
            data: x.amount.to_le_bytes().to_vec(),
            capacity: 0,
            absolute_capacity: false,
            type_id: false,
        }));
    }

    Instruction::new(operations)
}

// ---------------------------------------------------------------------------
// 3. Match Order — seller matches order with pre-created channel
// ---------------------------------------------------------------------------

/// Matches an Order Cell with a pre-created Fiber channel.
///
/// The channel must already exist on-chain (created via Fiber API).
/// This transaction references it as a CellDep and produces a Match Cell.
///
/// # Transaction Structure
///
/// ```text
/// CellDeps:
///   [0] Opticrum contract code cell (resolved via type_id)
///   [1] Channel Cell (pre-created via Fiber API)
///       lock:   the channel's lock script
///       capacity: >= order.channel_capacity (verified on-chain)
///       This cell is NOT consumed — referenced via its OutPoint to prove
///       existence and verify capacity.
///
/// Inputs:
///   [0] Order Cell (consumed — ScriptPattern::Transfer)
///       lock:   Opticrum (ORDER_ARGS_LEN-byte args)
///       data:   OrderData (ORDER_DATA_LEN bytes)
///       capacity: rent_capacity
///   [1] Seller's cell (provides CKB for fees + witness/signature)
///       lock:   seller's personal lock
///
/// Outputs:
///   [0] Match Cell (produced from Order Cell)
///       lock:   Opticrum (MATCH_ARGS_LEN-byte args: Order args
///                         + channel_outpoint + seller_lock_hash + fiber_pubkey)
///       type:   none
///       data:   MatchData (MATCH_DATA_LEN bytes, last_extraction=0)
///       capacity: rent_capacity (MUST equal Inputs[0].capacity)
///   [1] Seller change cell
///       lock:   same as Inputs[1] (seller's lock)
///       capacity: Inputs[1].capacity - tx_fee (adjusted)
/// ```
///
/// The Order Cell undergoes ScriptPattern::Transfer because a matching Opticrum
/// output (the Match Cell) is produced. The OrderMatch verifier checks:
/// - Channel Cell identified by channel_outpoint has capacity >= order.channel_capacity
/// - Match args' first ORDER_ARGS_LEN bytes match Order args
/// - Match data initialized correctly (rent_per_block > 0, escrow_blocks > 0, last_extraction == 0)
/// - Match capacity equals Order capacity
pub fn match_order<T: RPC>(
    seller: Address,
    order_info: OrderInfo,
    match_args: MatchArgs,
) -> Instruction<T> {
    let escrow_blocks = order_info.order_data.escrow_blocks;
    let match_data = if let Some(ref x) = order_info.xudt {
        MatchData::new(
            x.amount,
            x.amount as f64 / escrow_blocks as f64,
            escrow_blocks,
        )
    } else {
        MatchData::new(
            0,
            order_info.ckb_capacity as f64 / escrow_blocks as f64,
            escrow_blocks,
        )
    };

    Instruction::new(vec![
        // Opticrum contract dep
        Box::new(AddOpticrumContractCelldep {}),
        // Channel Cell as CellDep (pre-created via Fiber)
        Box::new(AddCellDep {
            name: "fiber_channel".into(),
            tx_hash: match_args.channel_outpoint.tx_hash.into(),
            index: match_args.channel_outpoint.index,
            dep_type: DepType::Code,
            with_data: true,
        }),
        // Consume the Order Cell
        Box::new(AddInputCellByOutPoint {
            tx_hash: order_info.order_outpoint.tx_hash.into(),
            index: order_info.order_outpoint.index,
            since: None,
        }),
        // Seller provides capacity + signing
        Box::new(AddInputCellByAddress {
            address: seller.clone(),
        }),
        // Produce Match Cell
        Box::new(AddOutputCellByInputIndex {
            input_index: 0,
            lock_script: Some(opticrum_lock(match_args.to_bytes().to_vec())),
            type_script: None,
            data: Some(match_data.to_bytes().to_vec()),
            adjust_capacity: CapacityAdjustment::Keep,
        }),
    ])
}

// ---------------------------------------------------------------------------
// 4. Extract Rent — seller withdraws linear rent
// ---------------------------------------------------------------------------

/// Seller extracts rent from a Match Cell.
///
/// Must be called periodically before escrow expires.
///
/// # Transaction Structure
///
/// ```text
/// CellDeps:
///   [0] Opticrum contract code cell (resolved via type_id)
///
/// HeaderDeps:
///   [0] Header at match_creation_block (only on first extraction)
///   [1] Header at tip_block (current chain tip for elapsed-block calculation)
///
/// Inputs:
///   [0] Match Cell (consumed — ScriptPattern::Transfer)
///       lock:   Opticrum (MATCH_ARGS_LEN-byte Match args)
///       data:   MatchData (MATCH_DATA_LEN bytes)
///       capacity: current remaining rent
///   [1] Seller's cell (must match seller_lock_hash in Match args)
///       lock:   seller's personal lock (provides signature/witness)
///
/// Outputs:
///   [0] Updated Match Cell (unless exhausted)
///       lock:   same as Inputs[0] (Match args unchanged)
///       type:   none
///       data:   MatchData (same xudt_amount, same rent_per_block,
///                           same escrow_blocks,
///                           last_extraction_block = tip_block)
///       capacity: Inputs[0].capacity - extracted_rent
///   [1] Seller cell + extracted rent
///       lock:   same as Inputs[1] (seller's lock)
///       capacity: Inputs[1].capacity + extracted_rent - tx_fee
/// ```
///
/// The Match Cell undergoes ScriptPattern::Transfer because an updated Match
/// Cell appears in outputs. The MatchExtract verifier checks:
/// - Seller participates (lock hash in inputs)
/// - Exactly 1 Match input → 1 Match output (or 0 if exhausted)
/// - Extraction amount == rent_per_block × elapsed_blocks (linear)
/// - If accumulated rent exceeds remaining capacity, match is "exhausted":
///   all remaining capacity + xUDT is released to seller
/// - Match data updated correctly (only last_extraction_block changes;
///   rent_per_block and escrow_blocks stay the same)
/// - Match args unchanged
///
/// Rent formula (linear):
///   extractable = rent_per_block × (tip_block - last_extraction_block)
pub fn extract_rent<T: RPC>(
    seller: Address,
    match_info: MatchInfo,
    tip_block: u64,
) -> Instruction<T> {
    if match_info.is_exhausted(tip_block) {
        return destroy_match(seller, match_info, tip_block);
    }

    // If the match is exhausted, return the remaining xudt or ckb to the seller
    let mut operations: Vec<Box<dyn Operation<T>>> = vec![
        Box::new(AddOpticrumContractCelldep {}),
        Box::new(AddCellDep {
            name: "fiber_channel".into(),
            tx_hash: match_info.match_args.channel_outpoint.tx_hash.into(),
            index: match_info.match_args.channel_outpoint.index,
            dep_type: DepType::Code,
            with_data: true,
        }),
        Box::new(AddHeaderDepByBlockNumber {
            block_number: tip_block,
        }),
        Box::new(AddInputCellByOutPoint {
            tx_hash: match_info.match_outpoint.tx_hash.into(),
            index: match_info.match_outpoint.index,
            since: None,
        }),
        Box::new(AddInputCellByAddress {
            address: seller.clone(),
        }),
    ];

    if match_info.match_data.last_extraction_block == 0 {
        operations.push(Box::new(AddHeaderDepByBlockNumber {
            block_number: match_info.match_current_block,
        }));
    }

    let rent_extraction = match_info.extraction_amount(tip_block);
    let mut new_match_data = match_info.match_data;
    new_match_data.last_extraction_block = tip_block;
    new_match_data.xudt_amount = 0;
    if let Some(ref x) = match_info.xudt {
        new_match_data.xudt_amount = x.amount.saturating_sub(rent_extraction as u128);
        operations.push(Box::new(AddOutputCellByInputIndex {
            input_index: 0,
            data: Some(new_match_data.to_bytes().to_vec()),
            lock_script: None,
            type_script: None,
            adjust_capacity: CapacityAdjustment::Keep,
        }));
    } else {
        operations.push(Box::new(AddOutputCellByInputIndex {
            input_index: 0,
            data: Some(new_match_data.to_bytes().to_vec()),
            lock_script: None,
            type_script: None,
            adjust_capacity: CapacityAdjustment::Subtract(rent_extraction),
        }));
    }

    Instruction::new(operations)
}

// ---------------------------------------------------------------------------
// 5. Destroy Match — anyone sweeps remaining after exhaustion
// ---------------------------------------------------------------------------

/// Destroys an exhausted Match Cell, returning remaining funds to the claimant.
///
/// This is the safety valve: if the seller abandons the match (forgets or
/// fails to extract), anyone can call this after enough blocks have passed
/// for the rent to fully vest.
///
/// # Transaction Structure
///
/// ```text
/// CellDeps:
///   [0] Opticrum contract code cell (resolved via type_id)
///
/// HeaderDeps:
///   [0] Header at match_creation_block (proves when the match was created)
///   [1] Header at tip_block (current chain tip for exhaustion check)
///
/// Inputs:
///   [0] Match Cell (consumed — ScriptPattern::Burn)
///       lock:   Opticrum (MATCH_ARGS_LEN-byte Match args)
///       data:   MatchData (MATCH_DATA_LEN bytes)
///       capacity: remaining rent
///   [1] Claimant's cell (provides CKB for fees + witness/signature)
///       lock:   claimant's personal lock
///
/// No authorization required beyond the exhaustion check — any third party
/// can sweep an abandoned Match. The seller's economic incentive to extract
/// regularly prevents premature destruction.
/// ```
///
/// The Match Cell undergoes ScriptPattern::Burn because no Opticrum output
/// is produced. The MatchDestroy verifier checks:
/// - Match is exhausted: rent_per_block × (tip - last_extraction_or_creation)
///   >= remaining capacity
pub fn destroy_match<T: RPC>(
    claimant: Address,
    match_info: MatchInfo,
    tip_block: u64,
) -> Instruction<T> {
    let mut operations: Vec<Box<dyn Operation<T>>> = vec![
        Box::new(AddOpticrumContractCelldep {}),
        Box::new(AddHeaderDepByBlockNumber {
            block_number: tip_block,
        }),
        Box::new(AddInputCellByOutPoint {
            tx_hash: match_info.match_outpoint.tx_hash.into(),
            index: match_info.match_outpoint.index,
            since: None,
        }),
        Box::new(AddInputCellByAddress {
            address: claimant.clone(),
        }),
    ];

    if match_info.match_data.last_extraction_block == 0 {
        operations.push(Box::new(AddHeaderDepByBlockNumber {
            block_number: match_info.match_current_block,
        }));
    }

    // Transfer remaining xUDT back to claimant if any
    if let Some(ref x) = match_info.xudt {
        operations.push(Box::new(AddXudtCelldep {}));
        operations.push(Box::new(AddOutputCellByInputIndex {
            input_index: 0,
            data: Some(x.amount.to_le_bytes().to_vec()),
            lock_script: Some(claimant.into()),
            type_script: None,
            adjust_capacity: CapacityAdjustment::Keep,
        }));
    } else {
        operations.push(Box::new(AddOutputCellByInputIndex {
            input_index: 0,
            data: Some(Vec::new()),
            lock_script: Some(claimant.into()),
            type_script: None,
            adjust_capacity: CapacityAdjustment::Keep,
        }));
    }

    Instruction::new(operations)
}
