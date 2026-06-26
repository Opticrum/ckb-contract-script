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
            AddCellDep, AddHeaderDepByBlockNumber, AddHeaderDepByCellDepIndex,
            AddHeaderDepByInputIndex, AddInputCellByAddress, AddInputCellByOutPoint, AddOutputCell,
            AddOutputCellByInputIndex, CapacityAdjustment,
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
    types::{MatchArgs, MatchData, MatchInfo, OrderArgs, OrderData, OrderInfo},
};

// ---------------------------------------------------------------------------
// 1. Create Order — buyer offers rent for inbound liquidity
// ---------------------------------------------------------------------------

/// Creates an Order Cell on-chain.
///
/// The buyer directly specifies `rent_per_block` in the OrderData — the
/// per-block rate the seller will extract from the Match cell. The total
/// rent capacity locked up is passed as `rent_capacity` (for CKB orders).
///
/// # Transaction Structure
///
/// ```text
/// CellDeps:
///   [0] Opticrum contract code cell (resolved via type_id)
///
/// Inputs:
///   [0] Buyer's cell (must match buyer_lock_hash in Order args)
///
/// Outputs:
///   [0] Order Cell
///       lock:   Opticrum (ORDER_ARGS_LEN-byte args)
///       type:   none / xUDT type script
///       data:   OrderData (xudt_amount + channel_capacity + rent_per_block)
///       capacity: rent_capacity (+ ORDER_TO_MATCH_CAPACITY_RESERVE)
/// ```
pub fn create_order<T: RPC>(
    buyer: Address,
    order_args: &OrderArgs,
    order_data: &OrderData,
    rent_capacity: u64,
    xudt_type_script: Option<Script>,
) -> Instruction<T> {
    let args = order_args.to_bytes().to_vec();

    // Build the OrderData to store on-chain.
    // xudt_amount is zero for CKB-only orders (no xUDT type script).
    let stored_order_data = OrderData::new(
        if xudt_type_script.is_some() {
            order_data.xudt_amount
        } else {
            0
        },
        order_data.channel_capacity,
        order_data.shannons_per_block,
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
            capacity: 0,
            absolute_capacity: false,
            type_id: false,
        }));
    } else {
        operations.push(Box::new(AddOutputCell {
            lock_script: opticrum_lock(args),
            type_script: None,
            data: stored_order_data.to_bytes().to_vec(),
            capacity: rent_capacity,
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
/// The Order Cell is burned (appears only in inputs), routing to OrderCancel
/// verifier. The verifier checks that the buyer's lock hash matches the
/// `buyer_lock_hash` embedded in the Order args.
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
/// `rent_per_block` is copied directly from the buyer's OrderData — no
/// derivation from AnnualYield needed.
///
/// # Transaction Structure
///
/// ```text
/// CellDeps:
///   [0] Opticrum contract code cell
///   [1] Channel Cell (pre-created via Fiber API, NOT consumed)
///
/// Inputs:
///   [0] Order Cell (consumed)
///   [1] Seller's cell (provides CKB + witness)
///
/// Outputs:
///   [0] Match Cell (produced from Order Cell)
///       lock:   Opticrum (MATCH_ARGS_LEN-byte args)
///       data:   MatchData (rent_per_block from order, last_extraction_block=0)
///       capacity: Order capacity + ORDER_TO_MATCH_CAPACITY_RESERVE
/// ```
pub fn match_order<T: RPC>(
    seller: Address,
    order_info: OrderInfo,
    match_args: MatchArgs,
) -> Instruction<T> {
    let rent_per_block = order_info.order_data.shannons_per_block;
    let xudt_amount = order_info.xudt.as_ref().map(|x| x.amount).unwrap_or(0);

    let match_data = MatchData::new(xudt_amount, rent_per_block);

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
        // HeaderDep for block where the channel was created
        Box::new(AddHeaderDepByCellDepIndex {
            celldep_index: usize::MAX,
        }),
        // Consume the Order Cell
        Box::new(AddInputCellByOutPoint {
            tx_hash: order_info.order_outpoint.tx_hash.into(),
            index: order_info.order_outpoint.index,
            since: None,
        }),
        // HeaderDep for block where the order was created
        Box::new(AddHeaderDepByInputIndex {
            input_index: usize::MAX,
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
            adjust_capacity: CapacityAdjustment::Add(ORDER_TO_MATCH_CAPACITY_RESERVE),
        }),
    ])
}

// ---------------------------------------------------------------------------
// 4. Extract Rent — seller withdraws linear rent
// ---------------------------------------------------------------------------

/// Seller extracts rent from a Match Cell.
///
/// No status check needed — the match is always active after creation.
/// If exhausted, delegates to `destroy_match`.
///
/// Rent formula (linear):
///   extractable = rent_per_block × (tip_block - last_extraction_block)
///
/// # Transaction Structure
///
/// ```text
/// CellDeps:
///   [0] Opticrum contract code cell
///   [1] Channel Cell (existence check)
///
/// HeaderDeps:
///   [0] tip block
///   [1] match creation block (only on first extraction)
///
/// Inputs:
///   [0] Match Cell (consumed)
///   [1] Seller's cell (must match seller_lock_hash)
///
/// Outputs:
///   [0] Updated Match Cell (unless exhausted)
///       data: MatchData (last_extraction_block updated)
///       capacity: reduced by rent_extraction (CKB) or kept (xUDT)
/// ```
pub fn extract_rent<T: RPC>(
    seller: Address,
    match_info: MatchInfo,
    tip_block: u64,
) -> Instruction<T> {
    if match_info.is_exhausted(tip_block) {
        return destroy_match(seller, match_info, tip_block);
    }

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
        Box::new(AddInputCellByAddress {
            address: seller.clone(),
        }),
        Box::new(AddInputCellByOutPoint {
            tx_hash: match_info.match_outpoint.tx_hash.into(),
            index: match_info.match_outpoint.index,
            since: None,
        }),
    ];

    if match_info.match_data.last_extraction_block == 0 {
        // Match cell is at input_index 0 (added first above)
        operations.push(Box::new(AddHeaderDepByInputIndex {
            input_index: usize::MAX,
        }));
    }

    let rent_extraction = match_info.extraction_amount(tip_block);
    let mut new_match_data = match_info.match_data;
    new_match_data.last_extraction_block = tip_block;
    new_match_data.xudt_amount = 0;
    if let Some(ref x) = match_info.xudt {
        new_match_data.xudt_amount = x.amount.saturating_sub(rent_extraction as u128);
        operations.push(Box::new(AddOutputCellByInputIndex {
            input_index: usize::MAX,
            data: Some(new_match_data.to_bytes().to_vec()),
            lock_script: None,
            type_script: None,
            adjust_capacity: CapacityAdjustment::Keep,
        }));
    } else {
        operations.push(Box::new(AddOutputCellByInputIndex {
            input_index: usize::MAX,
            data: Some(new_match_data.to_bytes().to_vec()),
            lock_script: None,
            type_script: None,
            adjust_capacity: CapacityAdjustment::Subtract(rent_extraction),
        }));
    }

    Instruction::new(operations)
}

// ---------------------------------------------------------------------------
// 5. Update Match (Buyer) — inject or withdraw funds
// ---------------------------------------------------------------------------

/// Buyer injects or withdraws capacity / xUDT from a Match Cell.
///
/// The buyer can freely adjust the cell's value (xUDT amount or CKB capacity)
/// but cannot destroy the cell. At most they can empty it down to the
/// minimum occupied capacity.
///
/// `rent_per_block` and `last_extraction_block` are preserved.
///
/// # Transaction Structure
///
/// ```text
/// CellDeps:
///   [0] Opticrum contract code cell
///
/// Inputs:
///   [0] Match Cell (consumed)
///   [1] Buyer's cell (must match buyer_lock_hash)
///
/// Outputs:
///   [0] Updated Match Cell
///       data: MatchData (xudt_amount may change, rent_per_block + last_extraction_block preserved)
///       capacity: adjusted by capacity_delta
/// ```
pub fn update_match_buyer<T: RPC>(
    buyer: Address,
    match_info: MatchInfo,
    new_xudt_amount: u128,
    capacity_delta: i64,
) -> Instruction<T> {
    let mut new_match_data = match_info.match_data;
    new_match_data.xudt_amount = new_xudt_amount;
    // rent_per_block and last_extraction_block are preserved by not changing them

    let adjust = if capacity_delta >= 0 {
        CapacityAdjustment::Add(capacity_delta as u64)
    } else {
        CapacityAdjustment::Subtract((-capacity_delta) as u64)
    };

    let operations: Vec<Box<dyn Operation<T>>> = vec![
        Box::new(AddOpticrumContractCelldep {}),
        Box::new(AddInputCellByOutPoint {
            tx_hash: match_info.match_outpoint.tx_hash.into(),
            index: match_info.match_outpoint.index,
            since: None,
        }),
        Box::new(AddInputCellByAddress {
            address: buyer.clone(),
        }),
        Box::new(AddOutputCellByInputIndex {
            input_index: 0,
            lock_script: None,
            type_script: None,
            data: Some(new_match_data.to_bytes().to_vec()),
            adjust_capacity: adjust,
        }),
    ];

    Instruction::new(operations)
}

// ---------------------------------------------------------------------------
// 6. Destroy Match — seller sweeps exhausted match
// ---------------------------------------------------------------------------

/// Destroys an exhausted Match Cell, returning remaining funds to the seller.
///
/// Only the seller can destroy, and only when the match is exhausted
/// (accumulated rent >= remaining value).
///
/// # Transaction Structure
///
/// ```text
/// CellDeps:
///   [0] Opticrum contract code cell
///
/// HeaderDeps:
///   [0] tip block
///   [1] match creation block (if never extracted)
///
/// Inputs:
///   [0] Match Cell (consumed — Burn)
///   [1] Seller's cell
///
/// Outputs:
///   [0] Seller cell with remaining funds
/// ```
pub fn destroy_match<T: RPC>(
    seller: Address,
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
            address: seller.clone(),
        }),
    ];

    if match_info.match_data.last_extraction_block == 0 {
        operations.push(Box::new(AddHeaderDepByBlockNumber {
            block_number: match_info.match_current_block,
        }));
    }

    // Transfer remaining xUDT back to seller if any
    if let Some(ref x) = match_info.xudt {
        operations.push(Box::new(AddXudtCelldep {}));
        operations.push(Box::new(AddOutputCellByInputIndex {
            input_index: 0,
            data: Some(x.amount.to_le_bytes().to_vec()),
            lock_script: Some(seller.into()),
            type_script: None,
            adjust_capacity: CapacityAdjustment::Keep,
        }));
    } else {
        operations.push(Box::new(AddOutputCellByInputIndex {
            input_index: 0,
            data: Some(Vec::new()),
            lock_script: Some(seller.into()),
            type_script: None,
            adjust_capacity: CapacityAdjustment::Keep,
        }));
    }

    Instruction::new(operations)
}
