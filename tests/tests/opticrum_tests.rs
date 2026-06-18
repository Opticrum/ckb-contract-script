//! Integration tests for the Opticrum contract.
//!
//! Uses ckb-cinnabar's TransactionSimulator to run full CKB-VM verification
//! against the compiled RISC-V binary. Pattern follows spore-war's test structure.

use ckb_cinnabar_calculator::{
    address::{Address, AddressPayload},
    instruction::{Instruction, TransactionCalculator},
    re_exports::{
        ckb_types::{
            bytes::Bytes,
            core::{Capacity, ScriptHashType},
            packed::Script,
            prelude::{Builder, Entity, Pack, Unpack},
            H256,
        },
        eyre,
    },
    simulation::{
        always_success_script, fake_header_view, fake_outpoint, AddFakeAlwaysSuccessCelldep,
        AddFakeContractCelldepByName, FakeRpcClient, TransactionSimulator, DEFUALT_MAX_CYCLES,
    },
    skeleton::CellOutputEx,
};
use opticrum_calculator::{
    cancel_order, create_order, destroy_match, extract_rent, match_order, scan_matches,
    scan_orders,
    types::{
        AnnualYield, MatchArgs, MatchData, MatchInfo, OrderArgs, OrderData, OrderInfo, OutPoint,
        MATCH_ARGS_LEN, MATCH_DATA_LEN, ORDER_ARGS_LEN, ORDER_DATA_LEN,
    },
};

// ---------------------------------------------------------------------------
// Faker module
// ---------------------------------------------------------------------------

mod faker {
    use super::*;
    use ckb_cinnabar_calculator::{
        re_exports::ckb_types::packed::OutPoint as PackedOutPoint, skeleton::TransactionSkeleton,
    };

    // --- Constants ---

    pub const CHANNEL_CAPACITY: u64 = 100_000_000_000;
    pub const ESCROW_BLOCKS: u64 = 43200;
    pub const RENT_CAPACITY: u64 = 30_000_000_000; // must exceed min occupied (~22_100_000_000)
    pub const MATCH_CREATED_BLOCK: u64 = 1000;

    // --- Identity helpers ---

    /// Compute the blake2b_256 hash of always_success_script(vec![]).
    /// Used for buyer/seller lock hashes so they match the seeded user cells.
    pub fn user_lock_hash() -> [u8; 32] {
        let mut hash = [0u8; 32];
        let script_hash = ckb_cinnabar_calculator::re_exports::ckb_hash::blake2b_256(
            always_success_script(vec![]).as_slice(),
        );
        hash.copy_from_slice(&script_hash);
        hash
    }
    pub fn fiber_pubkey() -> [u8; 32] {
        [0x03u8; 32]
    }
    pub fn channel_outpoint() -> OutPoint {
        OutPoint::new([0x04u8; 32], 0)
    }

    // --- Address / Lock Script builders ---

    pub fn fake_address() -> Address {
        let lock = always_success_script(vec![]);
        let payload = AddressPayload::from(lock);
        Address::new(ckb_cinnabar_calculator::rpc::Network::Fake, payload)
    }

    fn build_opticrum_lock(args: Vec<u8>, skeleton: &TransactionSkeleton) -> eyre::Result<Script> {
        use ckb_cinnabar_calculator::skeleton::ScriptEx;
        Ok(ScriptEx::Reference("opticrum".into(), args).to_script(skeleton)?)
    }

    // --- Skeleton preparation ---

    /// Build a reusable skeleton pre-loaded with all contract celldeps.
    /// Uses the real opticrum binary from ../build/release and the always_success
    /// binary for mock channel (both loaded as in-memory fake celldeps).
    pub async fn celldeps_prepared_skeleton(
        rpc: &FakeRpcClient,
    ) -> eyre::Result<TransactionSkeleton> {
        let prepare = Instruction::<FakeRpcClient>::new(vec![
            Box::new(AddFakeAlwaysSuccessCelldep {}),
            Box::new(AddFakeContractCelldepByName {
                contract: "opticrum".to_string(),
                type_id_args: Some(H256::default()),
                contract_binary_path: "../build/release".to_string(),
            }),
        ]);
        let (skeleton, _) = TransactionCalculator::default()
            .instruction(prepare)
            .new_skeleton(rpc)
            .await?;
        Ok(skeleton)
    }

    // --- Cell seeding ---

    pub fn seed_user_cell(rpc: &mut FakeRpcClient, capacity: u64) {
        let lock = always_success_script(vec![]);
        let cell =
            CellOutputEx::new_from_scripts(lock, None, vec![], Some(Capacity::shannons(capacity)))
                .expect("build user cell");
        let header = fake_header_view(1, 0, 0);
        rpc.insert_fake_cell(fake_outpoint(), cell, Some(header));
    }

    pub fn seed_channel_cell(rpc: &mut FakeRpcClient, outpoint: &OutPoint, capacity: u64) {
        // Flat type script: blake2b_256(code_hash=[0xCC;32] || Data1 || empty)
        // matches MOCK_FIBER_FUNDING_TYPE_HASH in the contract.
        let channel_type = Script::new_builder()
            .code_hash(H256([0xCCu8; 32]).pack())
            .hash_type(ScriptHashType::Data1.into())
            .args(Bytes::new().pack())
            .build();
        let cell = CellOutputEx::new_from_scripts(
            Script::default(),
            Some(channel_type),
            vec![],
            Some(Capacity::shannons(capacity)),
        )
        .expect("build channel cell");
        let packed = PackedOutPoint::new_builder()
            .tx_hash(H256(outpoint.tx_hash).pack())
            .index(outpoint.index.pack())
            .build();
        let header = fake_header_view(1, 0, 0);
        rpc.insert_fake_cell(packed, cell, Some(header));
    }

    pub fn seed_header(rpc: &mut FakeRpcClient, block_number: u64, timestamp: u64) {
        rpc.insert_fake_header(fake_header_view(block_number, timestamp, 0));
    }

    pub fn seed_order_cell(
        rpc: &mut FakeRpcClient,
        skeleton: &TransactionSkeleton,
        order_args: &OrderArgs,
        order_data: &OrderData,
        capacity: u64,
    ) -> eyre::Result<PackedOutPoint> {
        let lock = build_opticrum_lock(order_args.to_bytes().to_vec(), skeleton)?;
        let cell = CellOutputEx::new_from_scripts(
            lock,
            None,
            order_data.to_bytes().to_vec(),
            Some(Capacity::shannons(capacity)),
        )?;
        let outpoint = fake_outpoint();
        let header = fake_header_view(1, 0, 0);
        rpc.insert_fake_cell(outpoint.clone(), cell, Some(header));
        Ok(outpoint)
    }

    pub fn seed_match_cell(
        rpc: &mut FakeRpcClient,
        skeleton: &TransactionSkeleton,
        match_args: &MatchArgs,
        match_data: &MatchData,
        capacity: u64,
        creation_block: u64,
    ) -> eyre::Result<PackedOutPoint> {
        let lock = build_opticrum_lock(match_args.to_bytes().to_vec(), skeleton)?;
        let cell = CellOutputEx::new_from_scripts(
            lock,
            None,
            match_data.to_bytes().to_vec(),
            Some(Capacity::shannons(capacity)),
        )?;
        let outpoint = fake_outpoint();
        let header = fake_header_view(creation_block, 0, 0);
        rpc.insert_fake_cell(outpoint.clone(), cell, Some(header));
        Ok(outpoint)
    }

    // --- Conversion helpers ---

    pub fn to_proto_outpoint(packed: &PackedOutPoint) -> OutPoint {
        OutPoint::new(packed.tx_hash().unpack(), packed.index().unpack())
    }

    pub fn to_order_info(
        packed: &PackedOutPoint,
        order_args: OrderArgs,
        order_data: OrderData,
    ) -> OrderInfo {
        OrderInfo {
            order_args,
            order_data,
            xudt: None,
            ckb_capacity: RENT_CAPACITY,
            order_outpoint: to_proto_outpoint(packed),
        }
    }

    pub fn to_match_info(
        packed: &PackedOutPoint,
        match_args: MatchArgs,
        match_data: MatchData,
    ) -> MatchInfo {
        MatchInfo {
            match_args,
            match_data,
            xudt: None,
            ckb_capacity: RENT_CAPACITY,
            match_outpoint: to_proto_outpoint(packed),
            match_current_block: MATCH_CREATED_BLOCK,
        }
    }
}

// ---------------------------------------------------------------------------
// VM-Verified Integration Tests
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
    faker::seed_user_cell(&mut rpc, 200_000_000_000);

    let buyer = faker::fake_address();
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
    );

    let packed = faker::seed_order_cell(
        &mut rpc,
        &skeleton,
        &order_args,
        &order_data,
        faker::RENT_CAPACITY,
    )?;
    faker::seed_channel_cell(
        &mut rpc,
        &faker::channel_outpoint(),
        faker::CHANNEL_CAPACITY,
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

#[tokio::test]
async fn test_extract_rent() -> eyre::Result<()> {
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
// Unit Tests: Encoding + Rent Math
// ---------------------------------------------------------------------------

#[test]
fn test_args_encoding() {
    let order = OrderArgs::new([0x03u8; 32], [0x01u8; 32]);
    let args = order.to_bytes();
    assert_eq!(args.len(), ORDER_ARGS_LEN);

    let match_args = MatchArgs::new(order.clone(), faker::channel_outpoint(), [0x02u8; 32]);
    let match_bytes = match_args.to_bytes();
    assert_eq!(match_bytes.len(), MATCH_ARGS_LEN);
    assert_eq!(&match_bytes[..ORDER_ARGS_LEN], &args[..]);
}

#[test]
fn test_match_data_encoding() {
    let match_data = MatchData::new(0, 1.5, faker::ESCROW_BLOCKS);
    let data = match_data.to_bytes();
    assert_eq!(data.len(), MATCH_DATA_LEN);

    let parsed = MatchData::from_slice(&data).expect("roundtrip");
    assert_eq!(parsed.xudt_amount, 0);
    assert_eq!(parsed.rent_per_block, 1.5);
    assert_eq!(parsed.escrow_blocks, faker::ESCROW_BLOCKS);
    assert_eq!(parsed.last_extraction_block, 0);
}

#[test]
fn test_linear_rent_calculation() {
    let rent_per_block: f64 = 1.0;
    let elapsed: u64 = 500;
    let extractable = (rent_per_block * elapsed as f64) as u64;
    assert_eq!(extractable, 500);

    let total_rent = (rent_per_block * 1000u64 as f64) as u64;
    assert_eq!(total_rent, 1000);
}

#[test]
fn test_complete_lifecycle_types() {
    let order_args = OrderArgs::new([0x03u8; 32], [0x01u8; 32]);
    let order_bytes = order_args.to_bytes();
    assert_eq!(order_bytes.len(), ORDER_ARGS_LEN);

    let match_args = MatchArgs::new(order_args.clone(), faker::channel_outpoint(), [0x02u8; 32]);
    let match_bytes = match_args.to_bytes();
    assert_eq!(match_bytes.len(), MATCH_ARGS_LEN);
    assert_eq!(&match_bytes[..ORDER_ARGS_LEN], &order_bytes[..]);

    let order_data = OrderData::new(0, faker::CHANNEL_CAPACITY, faker::ESCROW_BLOCKS);
    assert_eq!(order_data.to_bytes().len(), ORDER_DATA_LEN);

    let match_data = MatchData::new(0, 1.0, faker::ESCROW_BLOCKS);
    assert_eq!(match_data.to_bytes().len(), MATCH_DATA_LEN);
    assert_eq!(match_data.escrow_blocks, faker::ESCROW_BLOCKS);
    assert_eq!(match_data.last_extraction_block, 0);
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
