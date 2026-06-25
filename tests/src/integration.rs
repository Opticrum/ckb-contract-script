//! Integration tests for the Opticrum contract.
//!
//! Uses ckb-cinnabar's TransactionSimulator to run full CKB-VM verification
//! against the compiled RISC-V binary.

use ckb_cinnabar_calculator::{
    re_exports::{ckb_types::prelude::Unpack, eyre},
    simulation::{FakeRpcClient, TransactionSimulator, DEFUALT_MAX_CYCLES},
};
use opticrum_calculator::{
    cancel_order, create_order, destroy_match, extract_rent, match_order, scan_matches,
    scan_orders,
    types::{AnnualYield, MatchArgs, MatchData, OrderArgs, OrderData, MATCH_ARGS_LEN},
};

use crate::faker;

// ---------------------------------------------------------------------------
// VM-Verified Integration Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn match_skeleton_channel_celldep() -> eyre::Result<()> {
    use ckb_cinnabar_calculator::operation::Log;

    let mut rpc = FakeRpcClient::default();
    let mut skeleton = faker::celldeps_prepared_skeleton(&rpc).await?;
    faker::seed_user_cell(&mut rpc, 200_000_000_000);

    let seller = faker::fake_address();
    let order_args = OrderArgs::new(faker::fiber_pubkey(), faker::user_lock_hash());
    let order_data = OrderData::new(0, faker::CHANNEL_CAPACITY, faker::ESCROW_BLOCKS);
    let match_args = MatchArgs::new(
        order_args.clone(),
        faker::channel_outpoint(),
        faker::user_lock_hash(),
        faker::seller_fiber_pubkey(),
    );

    let packed = faker::seed_order_cell(
        &mut rpc,
        &skeleton,
        &order_args,
        &order_data,
        faker::RENT_CAPACITY,
    )?;
    faker::seed_match_channel_cell(&mut rpc, &order_args, &match_args, faker::CHANNEL_CAPACITY);

    let order_info = faker::to_order_info(&packed, order_args, order_data);
    let instruction = match_order(seller, order_info, match_args.clone());
    let mut log = Log::new();
    instruction.run(&rpc, &mut skeleton, &mut log).await?;

    let dep = skeleton
        .get_celldep_by_name("fiber_channel")
        .expect("fiber_channel celldep");
    let type_hash: [u8; 32] = dep.output.calc_type_hash().expect("type hash").into();
    assert_eq!(type_hash, faker::CONTRACT_MOCK);
    let capacity: u64 = dep.output.output.capacity().unpack();
    assert!(
        capacity >= faker::CHANNEL_CAPACITY,
        "channel capacity must satisfy order"
    );
    let tx_hash: [u8; 32] = dep.celldep.out_point().tx_hash().unpack();
    let index: u32 = dep.celldep.out_point().index().unpack();
    assert_eq!(tx_hash, match_args.channel_outpoint.tx_hash);
    assert_eq!(index, match_args.channel_outpoint.index);

    let match_lock_args = skeleton
        .outputs
        .iter()
        .find_map(|o| {
            let args = o.lock_script().args().raw_data();
            (args.len() == MATCH_ARGS_LEN).then(|| args.to_vec())
        })
        .expect("match output lock args");
    let parsed = MatchArgs::from_slice(&match_lock_args).expect("parse match output args");
    assert_eq!(
        parsed.channel_outpoint.tx_hash,
        match_args.channel_outpoint.tx_hash
    );
    assert_eq!(
        parsed.channel_outpoint.index,
        match_args.channel_outpoint.index
    );

    let resolved = skeleton.into_resolved_transaction(&rpc).await?;
    assert!(
        !resolved.transaction.cell_deps().is_empty(),
        "match tx must include cell deps"
    );
    let channel_meta = resolved
        .resolved_cell_deps
        .iter()
        .find(|m| {
            let tx_hash: [u8; 32] = m.out_point.tx_hash().unpack();
            let index: u32 = m.out_point.index().unpack();
            tx_hash == match_args.channel_outpoint.tx_hash
                && index == match_args.channel_outpoint.index
        })
        .expect("channel celldep in resolved tx");
    let resolved_cap: u64 = channel_meta.cell_output.capacity().unpack();
    assert!(resolved_cap >= faker::CHANNEL_CAPACITY);

    Ok(())
}

// ---------------------------------------------------------------------------
// Lifecycle: Create / Cancel
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_create_order() -> eyre::Result<()> {
    let mut rpc = FakeRpcClient::default();
    let skeleton = faker::celldeps_prepared_skeleton(&rpc).await?;
    faker::seed_user_cell(&mut rpc, 200_000_000_000);

    let buyer = faker::fake_address();
    let order_args = OrderArgs::new(faker::fiber_pubkey(), faker::user_lock_hash());
    let order_data = OrderData::new(0, faker::CHANNEL_CAPACITY, faker::ESCROW_BLOCKS);
    let instruction = create_order(buyer, &order_args, &order_data, AnnualYield(10), None);

    let cycle = TransactionSimulator::default()
        .skeleton(skeleton)
        .link_cell_to_header(rpc.get_outpoint_to_headers())
        .async_verify(&rpc, vec![instruction], DEFUALT_MAX_CYCLES)
        .await?;
    println!("create_order cycle: {}", cycle);
    Ok(())
}

#[tokio::test]
async fn test_cancel_order() -> eyre::Result<()> {
    let mut rpc = FakeRpcClient::default();
    let skeleton = faker::celldeps_prepared_skeleton(&rpc).await?;
    let buyer = faker::fake_address();
    faker::seed_user_cell(&mut rpc, 200_000_000_000);

    let order_args = OrderArgs::new(faker::fiber_pubkey(), faker::user_lock_hash());
    let order_data = OrderData::new(0, faker::CHANNEL_CAPACITY, faker::ESCROW_BLOCKS);
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
    let order_data = OrderData::new(0, faker::CHANNEL_CAPACITY, faker::ESCROW_BLOCKS);
    let match_args = MatchArgs::new(
        order_args.clone(),
        faker::channel_outpoint(),
        faker::user_lock_hash(),
        faker::seller_fiber_pubkey(),
    );

    let packed = faker::seed_order_cell(
        &mut rpc,
        &skeleton,
        &order_args,
        &order_data,
        faker::RENT_CAPACITY,
    )?;
    faker::seed_match_channel_cell(&mut rpc, &order_args, &match_args, faker::CHANNEL_CAPACITY);

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

#[tokio::test]
async fn test_match_order_rejects_wrong_seller_fiber_pubkey() -> eyre::Result<()> {
    let mut rpc = FakeRpcClient::default();
    let skeleton = faker::celldeps_prepared_skeleton(&rpc).await?;
    faker::seed_user_cell(&mut rpc, 200_000_000_000);

    let seller = faker::fake_address();
    let order_args = OrderArgs::new(faker::fiber_pubkey(), faker::user_lock_hash());
    let order_data = OrderData::new(0, faker::CHANNEL_CAPACITY, faker::ESCROW_BLOCKS);
    // Use wrong seller fiber pubkey in match args
    let match_args = MatchArgs::new(
        order_args.clone(),
        faker::channel_outpoint(),
        faker::user_lock_hash(),
        faker::wrong_seller_fiber_pubkey(),
    );

    let packed = faker::seed_order_cell(
        &mut rpc,
        &skeleton,
        &order_args,
        &order_data,
        faker::RENT_CAPACITY,
    )?;
    // Seed channel with the CORRECT seller pubkey (what the channel was created with)
    let correct_match_args = MatchArgs::new(
        order_args.clone(),
        faker::channel_outpoint(),
        faker::user_lock_hash(),
        faker::seller_fiber_pubkey(),
    );
    faker::seed_match_channel_cell(
        &mut rpc,
        &order_args,
        &correct_match_args,
        faker::CHANNEL_CAPACITY,
    );

    let order_info = faker::to_order_info(&packed, order_args, order_data);
    let instruction = match_order(seller, order_info, match_args);

    let result = TransactionSimulator::default()
        .skeleton(skeleton)
        .link_cell_to_header(rpc.get_outpoint_to_headers())
        .async_verify(&rpc, vec![instruction], DEFUALT_MAX_CYCLES)
        .await;
    assert!(result.is_err(), "should reject wrong seller fiber pubkey");
    Ok(())
}

#[tokio::test]
async fn test_match_order_rejects_wrong_channel_funding_lock() -> eyre::Result<()> {
    let mut rpc = FakeRpcClient::default();
    let skeleton = faker::celldeps_prepared_skeleton(&rpc).await?;
    faker::seed_user_cell(&mut rpc, 200_000_000_000);

    let seller = faker::fake_address();
    let order_args = OrderArgs::new(faker::fiber_pubkey(), faker::user_lock_hash());
    let order_data = OrderData::new(0, faker::CHANNEL_CAPACITY, faker::ESCROW_BLOCKS);
    let match_args = MatchArgs::new(
        order_args.clone(),
        faker::channel_outpoint(),
        faker::user_lock_hash(),
        faker::seller_fiber_pubkey(),
    );

    let packed = faker::seed_order_cell(
        &mut rpc,
        &skeleton,
        &order_args,
        &order_data,
        faker::RENT_CAPACITY,
    )?;
    // Seed channel with wrong funding lock args (wrong aggregated key)
    let wrong_funding_lock_args: [u8; 20] = [0xAA; 20];
    faker::seed_channel_cell(
        &mut rpc,
        &match_args.channel_outpoint,
        faker::CHANNEL_CAPACITY,
        wrong_funding_lock_args,
    );

    let order_info = faker::to_order_info(&packed, order_args, order_data);
    let instruction = match_order(seller, order_info, match_args);

    let result = TransactionSimulator::default()
        .skeleton(skeleton)
        .link_cell_to_header(rpc.get_outpoint_to_headers())
        .async_verify(&rpc, vec![instruction], DEFUALT_MAX_CYCLES)
        .await;
    assert!(result.is_err(), "should reject wrong channel funding lock");
    Ok(())
}

// ---------------------------------------------------------------------------
// Lifecycle: Extract & Destroy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_extract_rent() -> eyre::Result<()> {
    let mut rpc = FakeRpcClient::default();
    let skeleton = faker::celldeps_prepared_skeleton(&rpc).await?;
    faker::seed_user_cell(&mut rpc, 200_000_000_000);

    let seller = faker::fake_address();
    let order_args = OrderArgs::new(faker::fiber_pubkey(), faker::user_lock_hash());
    let match_args = MatchArgs::new(
        order_args.clone(),
        faker::channel_outpoint(),
        faker::user_lock_hash(),
        faker::seller_fiber_pubkey(),
    );
    let rent_per_block = faker::RENT_CAPACITY as f64 / faker::ESCROW_BLOCKS as f64;
    let match_data = MatchData::new(0, rent_per_block, faker::ESCROW_BLOCKS);

    let packed = faker::seed_match_cell(
        &mut rpc,
        &skeleton,
        &match_args,
        &match_data,
        faker::RENT_CAPACITY,
        faker::MATCH_CREATED_BLOCK,
    )?;
    faker::seed_match_channel_cell(&mut rpc, &order_args, &match_args, faker::CHANNEL_CAPACITY);
    let tip = faker::MATCH_CREATED_BLOCK + 100;
    faker::seed_header(&mut rpc, faker::MATCH_CREATED_BLOCK, 0);
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
async fn test_destroy_match() -> eyre::Result<()> {
    let mut rpc = FakeRpcClient::default();
    let skeleton = faker::celldeps_prepared_skeleton(&rpc).await?;
    faker::seed_user_cell(&mut rpc, 200_000_000_000);

    let claimant = faker::fake_address();
    let order_args = OrderArgs::new(faker::fiber_pubkey(), faker::user_lock_hash());
    let match_args = MatchArgs::new(
        order_args,
        faker::channel_outpoint(),
        faker::user_lock_hash(),
        faker::seller_fiber_pubkey(),
    );
    let rent_per_block = faker::RENT_CAPACITY as f64 / faker::ESCROW_BLOCKS as f64;
    let match_data = MatchData::new(0, rent_per_block, faker::ESCROW_BLOCKS);

    let packed = faker::seed_match_cell(
        &mut rpc,
        &skeleton,
        &match_args,
        &match_data,
        faker::RENT_CAPACITY,
        faker::MATCH_CREATED_BLOCK,
    )?;
    let tip_after_expiry = faker::MATCH_CREATED_BLOCK + faker::ESCROW_BLOCKS + 100;
    faker::seed_header(&mut rpc, faker::MATCH_CREATED_BLOCK, 0);
    faker::seed_header(&mut rpc, tip_after_expiry, 1000);

    let match_info = faker::to_match_info(&packed, match_args, match_data);
    let instruction = destroy_match(claimant, match_info, tip_after_expiry);

    let cycle = TransactionSimulator::default()
        .skeleton(skeleton)
        .link_cell_to_header(rpc.get_outpoint_to_headers())
        .async_verify(&rpc, vec![instruction], DEFUALT_MAX_CYCLES)
        .await?;
    println!("destroy_match cycle: {}", cycle);
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
    let order_data = OrderData::new(0, faker::CHANNEL_CAPACITY, faker::ESCROW_BLOCKS);
    faker::seed_order_cell(
        &mut rpc,
        &skeleton,
        &order_args,
        &order_data,
        faker::RENT_CAPACITY,
    )?;
    let orders = scan_orders(&rpc).await?;
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
        faker::seller_fiber_pubkey(),
    );
    let match_data = MatchData::new(0, 1.0, faker::ESCROW_BLOCKS);
    faker::seed_match_cell(
        &mut rpc,
        &skeleton,
        &match_args,
        &match_data,
        faker::RENT_CAPACITY,
        1,
    )?;
    let matches = scan_matches(&rpc).await?;
    assert_eq!(matches.len(), 1, "should find one Match cell");
    Ok(())
}
