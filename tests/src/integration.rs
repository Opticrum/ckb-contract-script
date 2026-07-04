//! Integration tests for the Opticrum contract.
//!
//! Uses ckb-cinnabar's TransactionSimulator to run full CKB-VM verification
//! against the compiled RISC-V binary.

use ckb_cinnabar_calculator::{
    operation::Log,
    re_exports::eyre,
    simulation::{FakeRpcClient, TransactionSimulator, DEFUALT_MAX_CYCLES},
};
use opticrum_calculator::{
    cancel_order, create_order, destroy_match, extract_rent, match_order, scan_matches,
    scan_orders,
    types::{MatchArgs, MatchData, OrderArgs, OrderData},
    update_match_buyer,
};

use crate::faker;

// ---------------------------------------------------------------------------
// Lifecycle: Create / Cancel
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_create_order() -> eyre::Result<()> {
    let mut rpc = FakeRpcClient::default();
    let mut skeleton = faker::celldeps_prepared_skeleton(&rpc).await?;
    faker::seed_user_cell(&mut rpc, 200_000_000_000);

    let buyer = faker::fake_address();
    let order_args = OrderArgs::new(faker::fiber_pubkey(), faker::user_lock_hash());
    let order_data = OrderData::new(0, faker::CHANNEL_CAPACITY, faker::SHANNONS_PER_BLOCK);
    let fiber_addr = "/ip4/192.168.1.1/tcp/9735/p2p/12D3KooWTest".to_string();

    let instruction = create_order(
        buyer,
        &order_args,
        &order_data,
        faker::RENT_CAPACITY,
        None,
        Some(fiber_addr.clone()),
    );

    // Run the instruction manually so we can inspect the skeleton's witnesses
    let mut log = Log::new();
    instruction.run(&rpc, &mut skeleton, &mut log).await?;

    // The order cell is output_index 0.
    // input_count is 1 (the buyer cell added by AddInputCellByAddress).
    // Witness slot: witnesses[input_count + output_index] = witnesses[1]
    let output_index = 0usize;
    let witness_index = skeleton.inputs.len() + output_index;
    assert!(
        witness_index < skeleton.witnesses.len(),
        "expected witness at index {}, but only {} witnesses exist",
        witness_index,
        skeleton.witnesses.len()
    );

    let witness = &skeleton.witnesses[witness_index];
    assert!(
        !witness.empty,
        "witness at index {} should not be empty when fiber_address is set",
        witness_index
    );
    assert!(
        witness.lock.is_empty(),
        "output_type witness should have empty lock field"
    );
    assert!(
        witness.input_type.is_empty(),
        "output_type witness should have empty input_type field"
    );
    assert_eq!(
        witness.output_type,
        fiber_addr.as_bytes().to_vec(),
        "fiber address should be stored in output_type field"
    );

    // Also verify the transaction passes on-chain verification
    let cycle = TransactionSimulator::default()
        .skeleton(skeleton)
        .link_cell_to_header(rpc.get_outpoint_to_headers())
        .async_verify(&rpc, vec![], DEFUALT_MAX_CYCLES)
        .await?;
    println!("create_order_with_fiber_address cycle: {}", cycle);
    Ok(())
}

#[tokio::test]
async fn test_cancel_order() -> eyre::Result<()> {
    let mut rpc = FakeRpcClient::default();
    let skeleton = faker::celldeps_prepared_skeleton(&rpc).await?;
    let buyer = faker::fake_address();
    faker::seed_user_cell(&mut rpc, 200_000_000_000);

    let order_args = OrderArgs::new(faker::fiber_pubkey(), faker::user_lock_hash());
    let order_data = OrderData::new(0, faker::CHANNEL_CAPACITY, faker::SHANNONS_PER_BLOCK);
    let packed = faker::seed_order_cell(
        &mut rpc,
        &skeleton,
        &order_args,
        &order_data,
        faker::RENT_CAPACITY,
    )?;
    let order_info = faker::to_order_info(&packed, order_args, order_data);
    let instruction = cancel_order(buyer, order_info);

    let cycle = TransactionSimulator::default()
        .skeleton(skeleton)
        .link_cell_to_header(rpc.get_outpoint_to_headers())
        .async_verify(&rpc, vec![instruction], DEFUALT_MAX_CYCLES)
        .await?;
    println!("cancel_order cycle: {}", cycle);
    Ok(())
}

// ---------------------------------------------------------------------------
// Lifecycle: Match
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_match_order() -> eyre::Result<()> {
    let mut rpc = FakeRpcClient::default();
    let skeleton = faker::celldeps_prepared_skeleton(&rpc).await?;
    faker::seed_user_cell(&mut rpc, 200_000_000_000);

    let seller = faker::fake_address();
    let order_args = OrderArgs::new(faker::fiber_pubkey(), faker::user_lock_hash());
    let order_data = OrderData::new(0, faker::CHANNEL_CAPACITY, faker::SHANNONS_PER_BLOCK);
    let match_args = MatchArgs::new(
        order_args.clone(),
        faker::channel_outpoint(),
        faker::user_lock_hash(),
    );

    let packed = faker::seed_order_cell_at(
        &mut rpc,
        &skeleton,
        &order_args,
        &order_data,
        faker::RENT_CAPACITY,
        faker::ORDER_CREATED_BLOCK,
    )?;
    faker::seed_channel_cell_at(
        &mut rpc,
        &match_args.channel_outpoint,
        faker::CHANNEL_CAPACITY,
        [0xABu8; 20],
        faker::CHANNEL_CREATED_BLOCK,
    );

    let order_info = faker::to_order_info(&packed, order_args, order_data);
    let instruction = match_order(seller, order_info, match_args);

    let cycle = TransactionSimulator::default()
        .skeleton(skeleton)
        .link_cell_to_header(rpc.get_outpoint_to_headers())
        .async_verify(&rpc, vec![instruction], DEFUALT_MAX_CYCLES)
        .await?;
    println!("match_order cycle: {}", cycle);
    Ok(())
}

// ---------------------------------------------------------------------------
// Lifecycle: Seller Extract Rent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_extract_rent() -> eyre::Result<()> {
    let mut rpc = FakeRpcClient::default();
    let skeleton = faker::celldeps_prepared_skeleton(&rpc).await?;
    let seller = faker::seed_user_cell_with_lock(&mut rpc, 200_000_000_000, vec![0x01]);

    let order_args = OrderArgs::new(faker::fiber_pubkey(), faker::user_lock_hash());
    let match_args = MatchArgs::new(
        order_args.clone(),
        faker::channel_outpoint(),
        faker::seller_lock_hash(), // distinct from buyer
    );
    // Match is always active — no status needed
    let match_data = MatchData::new(0, faker::SHANNONS_PER_BLOCK);

    let packed = faker::seed_match_cell(
        &mut rpc,
        &skeleton,
        &match_args,
        &match_data,
        faker::RENT_CAPACITY,
        faker::MATCH_CREATED_BLOCK,
    )?;
    faker::seed_match_channel_cell(&mut rpc, &match_args, faker::CHANNEL_CAPACITY);
    let tip = faker::MATCH_CREATED_BLOCK + 100;
    faker::seed_header(&mut rpc, tip, 1000);

    let match_info = faker::to_match_info(&packed, match_args, match_data);
    let instruction = extract_rent(seller, match_info, tip);

    let cycle = TransactionSimulator::default()
        .skeleton(skeleton)
        .link_cell_to_header(rpc.get_outpoint_to_headers())
        .async_verify(&rpc, vec![instruction], DEFUALT_MAX_CYCLES)
        .await?;
    println!("extract_rent cycle: {}", cycle);
    Ok(())
}

#[tokio::test]
async fn test_only_seller_can_extract() -> eyre::Result<()> {
    let mut rpc = FakeRpcClient::default();
    let skeleton = faker::celldeps_prepared_skeleton(&rpc).await?;
    let buyer = faker::fake_address();
    faker::seed_user_cell(&mut rpc, 200_000_000_000);

    let order_args = OrderArgs::new(faker::fiber_pubkey(), faker::user_lock_hash());
    let match_args = MatchArgs::new(
        order_args.clone(),
        faker::channel_outpoint(),
        faker::user_lock_hash(), // user_lock_hash = buyer, not seller
    );
    let match_data = MatchData::new(0, faker::SHANNONS_PER_BLOCK);

    // This match cell is not used directly — we create a second one below with seller_lock_hash
    let _packed = faker::seed_match_cell(
        &mut rpc,
        &skeleton,
        &match_args,
        &match_data,
        faker::RENT_CAPACITY,
        faker::MATCH_CREATED_BLOCK,
    )?;
    faker::seed_match_channel_cell(&mut rpc, &match_args, faker::CHANNEL_CAPACITY);
    let tip = faker::MATCH_CREATED_BLOCK + 100;
    faker::seed_header(&mut rpc, tip, 1000);

    // Buyer tries to extract (seller_lock_hash = user_lock_hash = buyer's hash)
    // But buyer_lock_hash is also the buyer's hash, so both would match.
    // Use a distinct seller_lock_hash for this test.
    let match_args2 = MatchArgs::new(
        order_args,
        faker::channel_outpoint(),
        faker::seller_lock_hash(), // different from buyer
    );
    let match_data2 = MatchData::new(0, faker::SHANNONS_PER_BLOCK);
    let packed2 = faker::seed_match_cell(
        &mut rpc,
        &skeleton,
        &match_args2,
        &match_data2,
        faker::RENT_CAPACITY,
        faker::MATCH_CREATED_BLOCK,
    )?;
    let match_info = faker::to_match_info(&packed2, match_args2, match_data2);
    let instruction = extract_rent(buyer, match_info, tip);

    let result = TransactionSimulator::default()
        .skeleton(skeleton)
        .link_cell_to_header(rpc.get_outpoint_to_headers())
        .async_verify(&rpc, vec![instruction], DEFUALT_MAX_CYCLES)
        .await;
    assert!(result.is_err(), "Buyer extracting rent should fail");
    Ok(())
}

// ---------------------------------------------------------------------------
// Lifecycle: Buyer Inject / Withdraw
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_buyer_inject_capacity() -> eyre::Result<()> {
    let mut rpc = FakeRpcClient::default();
    let skeleton = faker::celldeps_prepared_skeleton(&rpc).await?;
    faker::seed_user_cell(&mut rpc, 200_000_000_000);

    let buyer = faker::fake_address();
    let order_args = OrderArgs::new(faker::fiber_pubkey(), faker::user_lock_hash());
    let match_args = MatchArgs::new(
        order_args,
        faker::channel_outpoint(),
        faker::seller_lock_hash(), // distinct seller so only buyer auth matches
    );
    let match_data = MatchData::new(0, faker::SHANNONS_PER_BLOCK);

    let packed = faker::seed_match_cell(
        &mut rpc,
        &skeleton,
        &match_args,
        &match_data,
        faker::RENT_CAPACITY,
        faker::MATCH_CREATED_BLOCK,
    )?;

    let match_info = faker::to_match_info(&packed, match_args, match_data);
    // Inject 10_000_000_000 shannons (100 CKB)
    let instruction = update_match_buyer(buyer, match_info, 0, 10_000_000_000);

    let cycle = TransactionSimulator::default()
        .skeleton(skeleton)
        .link_cell_to_header(rpc.get_outpoint_to_headers())
        .async_verify(&rpc, vec![instruction], DEFUALT_MAX_CYCLES)
        .await?;
    println!("buyer_inject_capacity cycle: {}", cycle);
    Ok(())
}

#[tokio::test]
async fn test_buyer_withdraw_capacity() -> eyre::Result<()> {
    let mut rpc = FakeRpcClient::default();
    let skeleton = faker::celldeps_prepared_skeleton(&rpc).await?;
    faker::seed_user_cell(&mut rpc, 200_000_000_000);

    let buyer = faker::fake_address();
    let order_args = OrderArgs::new(faker::fiber_pubkey(), faker::user_lock_hash());
    let match_args = MatchArgs::new(
        order_args,
        faker::channel_outpoint(),
        faker::seller_lock_hash(), // distinct seller so only buyer auth matches
    );
    let match_data = MatchData::new(0, faker::SHANNONS_PER_BLOCK);

    let packed = faker::seed_match_cell(
        &mut rpc,
        &skeleton,
        &match_args,
        &match_data,
        faker::RENT_CAPACITY,
        faker::MATCH_CREATED_BLOCK,
    )?;

    let match_info = faker::to_match_info(&packed, match_args, match_data);
    // Withdraw 5_000_000_000 shannons (50 CKB) — should be okay since there's plenty of capacity
    let instruction = update_match_buyer(buyer, match_info, 0, -5_000_000_000);

    let cycle = TransactionSimulator::default()
        .skeleton(skeleton)
        .link_cell_to_header(rpc.get_outpoint_to_headers())
        .async_verify(&rpc, vec![instruction], DEFUALT_MAX_CYCLES)
        .await?;
    println!("buyer_withdraw_capacity cycle: {}", cycle);
    Ok(())
}

// ---------------------------------------------------------------------------
// Lifecycle: Destroy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_destroy_match_exhausted() -> eyre::Result<()> {
    let mut rpc = FakeRpcClient::default();
    let skeleton = faker::celldeps_prepared_skeleton(&rpc).await?;
    faker::seed_user_cell(&mut rpc, 200_000_000_000);

    let seller = faker::fake_address();
    let order_args = OrderArgs::new(faker::fiber_pubkey(), faker::user_lock_hash());
    let match_args = MatchArgs::new(
        order_args,
        faker::channel_outpoint(),
        faker::user_lock_hash(),
    );
    // Use a high rent_per_block so the match exhausts quickly
    let match_data = MatchData::new(0, faker::SHANNONS_PER_BLOCK);

    let packed = faker::seed_match_cell(
        &mut rpc,
        &skeleton,
        &match_args,
        &match_data,
        faker::RENT_CAPACITY,
        faker::MATCH_CREATED_BLOCK,
    )?;
    // Tip far enough past creation that rent exceeds capacity → exhausted
    let tip = faker::MATCH_CREATED_BLOCK + (faker::RENT_CAPACITY / faker::SHANNONS_PER_BLOCK) + 100;
    faker::seed_header(&mut rpc, faker::MATCH_CREATED_BLOCK, 0);
    faker::seed_header(&mut rpc, tip, 1000);

    let match_info = faker::to_match_info(&packed, match_args, match_data);
    let instruction = destroy_match(seller, match_info, tip);

    let cycle = TransactionSimulator::default()
        .skeleton(skeleton)
        .link_cell_to_header(rpc.get_outpoint_to_headers())
        .async_verify(&rpc, vec![instruction], DEFUALT_MAX_CYCLES)
        .await?;
    println!("destroy_match cycle: {}", cycle);
    Ok(())
}

#[tokio::test]
async fn test_destroy_not_exhausted_rejected() -> eyre::Result<()> {
    let mut rpc = FakeRpcClient::default();
    let skeleton = faker::celldeps_prepared_skeleton(&rpc).await?;
    faker::seed_user_cell(&mut rpc, 200_000_000_000);

    let seller = faker::fake_address();
    let order_args = OrderArgs::new(faker::fiber_pubkey(), faker::user_lock_hash());
    let match_args = MatchArgs::new(
        order_args,
        faker::channel_outpoint(),
        faker::user_lock_hash(),
    );
    let match_data = MatchData::new(0, faker::SHANNONS_PER_BLOCK);

    let packed = faker::seed_match_cell(
        &mut rpc,
        &skeleton,
        &match_args,
        &match_data,
        faker::RENT_CAPACITY,
        faker::MATCH_CREATED_BLOCK,
    )?;
    let tip = faker::MATCH_CREATED_BLOCK + 50;
    faker::seed_header(&mut rpc, tip, 1000);

    let match_info = faker::to_match_info(&packed, match_args, match_data);
    let instruction = destroy_match(seller, match_info, tip);

    let result = TransactionSimulator::default()
        .skeleton(skeleton)
        .link_cell_to_header(rpc.get_outpoint_to_headers())
        .async_verify(&rpc, vec![instruction], DEFUALT_MAX_CYCLES)
        .await;
    assert!(
        result.is_err(),
        "Destroying non-exhausted match should fail"
    );
    Ok(())
}

#[tokio::test]
async fn test_only_seller_can_destroy() -> eyre::Result<()> {
    let mut rpc = FakeRpcClient::default();
    let skeleton = faker::celldeps_prepared_skeleton(&rpc).await?;
    let seller = faker::seed_user_cell_with_lock(&mut rpc, 200_000_000_000, vec![0x01]);
    faker::seed_user_cell(&mut rpc, 200_000_000_000); // buyer cell

    let buyer = faker::fake_address();
    let order_args = OrderArgs::new(faker::fiber_pubkey(), faker::user_lock_hash());
    let match_args = MatchArgs::new(
        order_args,
        faker::channel_outpoint(),
        faker::seller_lock_hash(),
    );
    let match_data = MatchData::new(0, faker::SHANNONS_PER_BLOCK);

    let tip = faker::MATCH_CREATED_BLOCK + (faker::RENT_CAPACITY / faker::SHANNONS_PER_BLOCK) + 100;
    faker::seed_header(&mut rpc, faker::MATCH_CREATED_BLOCK, 0);
    faker::seed_header(&mut rpc, tip, 1000);

    // Buyer tries to destroy — should fail (buyer can't destroy)
    let packed_buyer = faker::seed_match_cell(
        &mut rpc,
        &skeleton,
        &match_args,
        &match_data,
        faker::RENT_CAPACITY,
        faker::MATCH_CREATED_BLOCK,
    )?;
    let match_info = faker::to_match_info(&packed_buyer, match_args.clone(), match_data.clone());
    let instruction = destroy_match(buyer.clone(), match_info, tip);
    let result = TransactionSimulator::default()
        .skeleton(skeleton.clone())
        .link_cell_to_header(rpc.get_outpoint_to_headers())
        .async_verify(&rpc, vec![instruction], DEFUALT_MAX_CYCLES)
        .await;
    assert!(result.is_err(), "Buyer destroying match should fail");

    // Seller destroys a separate match cell — should succeed
    let packed_seller = faker::seed_match_cell(
        &mut rpc,
        &skeleton,
        &match_args,
        &match_data,
        faker::RENT_CAPACITY,
        faker::MATCH_CREATED_BLOCK,
    )?;
    let match_info2 = faker::to_match_info(&packed_seller, match_args, match_data);
    let instruction2 = destroy_match(seller, match_info2, tip);
    let cycle = TransactionSimulator::default()
        .skeleton(skeleton)
        .link_cell_to_header(rpc.get_outpoint_to_headers())
        .async_verify(&rpc, vec![instruction2], DEFUALT_MAX_CYCLES)
        .await?;
    println!("seller_destroy_match cycle: {}", cycle);
    Ok(())
}

// ---------------------------------------------------------------------------
// Reader Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_scan_orders() -> eyre::Result<()> {
    let mut rpc = FakeRpcClient::default();
    let skeleton = faker::celldeps_prepared_skeleton(&rpc).await?;
    let order_args = OrderArgs::new(faker::fiber_pubkey(), faker::user_lock_hash());
    let order_data = OrderData::new(0, faker::CHANNEL_CAPACITY, faker::SHANNONS_PER_BLOCK);
    faker::seed_order_cell(
        &mut rpc,
        &skeleton,
        &order_args,
        &order_data,
        faker::RENT_CAPACITY,
    )?;
    let orders = scan_orders(&rpc, None).await?;
    assert_eq!(orders.len(), 1, "should find one Order cell");
    Ok(())
}

#[tokio::test]
async fn test_scan_matches() -> eyre::Result<()> {
    let mut rpc = FakeRpcClient::default();
    let skeleton = faker::celldeps_prepared_skeleton(&rpc).await?;
    let match_args = MatchArgs::new(
        OrderArgs::new(faker::fiber_pubkey(), faker::user_lock_hash()),
        faker::channel_outpoint(),
        faker::user_lock_hash(),
    );
    let match_data = MatchData::new(0, faker::SHANNONS_PER_BLOCK);
    faker::seed_match_cell(
        &mut rpc,
        &skeleton,
        &match_args,
        &match_data,
        faker::RENT_CAPACITY,
        1,
    )?;
    let matches = scan_matches(&rpc, None).await?;
    assert_eq!(matches.len(), 1, "should find one Match cell");
    Ok(())
}
