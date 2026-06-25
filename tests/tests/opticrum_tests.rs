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
        AnnualYield, CompressedPubkey, MatchArgs, MatchData, MatchInfo, OrderArgs, OrderData,
        OrderInfo, OutPoint, MATCH_ARGS_LEN, MATCH_DATA_LEN, ORDER_ARGS_LEN, ORDER_DATA_LEN,
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
    pub fn fiber_pubkey() -> CompressedPubkey {
        keypair_from([0x11; 32])
    }
    pub fn seller_fiber_pubkey() -> CompressedPubkey {
        keypair_from([0x22; 32])
    }
    pub fn wrong_seller_fiber_pubkey() -> CompressedPubkey {
        keypair_from([0x33; 32])
    }

    fn keypair_from(secret: [u8; 32]) -> CompressedPubkey {
        use secp256k1::{PublicKey, Secp256k1, SecretKey};
        let secp = Secp256k1::new();
        let sk = SecretKey::from_slice(&secret).expect("valid secret");
        CompressedPubkey::new(PublicKey::from_secret_key(&secp, &sk).serialize())
    }
    pub fn channel_outpoint() -> OutPoint {
        OutPoint::new([0x04u8; 32], 0)
    }

    /// Fiber funding lock args: blake160(x-only MuSig2 aggregated key).
    /// Uses an inline BIP-327 MuSig2* implementation (same algorithm as
    /// the C on-chain function and the old `keyagg` module).
    pub fn funding_lock_args(buyer: &CompressedPubkey, seller: &CompressedPubkey) -> [u8; 20] {
        let xonly = aggregate_funding_keys_xonly(buyer, seller).unwrap();
        let hash = ckb_cinnabar_calculator::re_exports::ckb_hash::blake2b_256(xonly);
        let mut args = [0u8; 20];
        args.copy_from_slice(&hash[..20]);
        args
    }

    /// BIP-327 MuSig2* key aggregation for 2-of-2 (inline test helper).
    ///
    /// Replaces the deleted `opticrum_protocol::keyagg` module.  Uses the
    /// same `secp256k1` v0.30 crate already available in test deps (for
    /// keypair generation) plus `sha2` for tagged hashes.
    pub fn aggregate_funding_keys_xonly(
        pk_a: &CompressedPubkey,
        pk_b: &CompressedPubkey,
    ) -> Result<[u8; 32], &'static str> {
        use secp256k1::{PublicKey, Scalar, Secp256k1};
        use sha2::{Digest, Sha256};

        const CURVE_ORDER: [u8; 32] = [
            0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
            0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE,
            0xBA, 0xAE, 0xDC, 0xE6, 0xAF, 0x48, 0xA0, 0x3B,
            0xBF, 0xD2, 0x5E, 0x8C, 0xD0, 0x36, 0x41, 0x41,
        ];

        fn tagged_hash(tag: &[u8], parts: &[&[u8]]) -> [u8; 32] {
            let tag_hash = Sha256::digest(tag);
            let mut hasher = Sha256::new();
            hasher.update(tag_hash);
            hasher.update(tag_hash);
            for part in parts {
                hasher.update(part);
            }
            hasher.finalize().into()
        }

        fn reduce_mod_n(bytes: &[u8; 32]) -> [u8; 32] {
            let mut out = *bytes;
            if out >= CURVE_ORDER {
                let mut borrow = 0i16;
                for i in (0..32).rev() {
                    let diff = out[i] as i16 - CURVE_ORDER[i] as i16 - borrow;
                    if diff < 0 {
                        out[i] = (diff + 256) as u8;
                        borrow = 1;
                    } else {
                        out[i] = diff as u8;
                        borrow = 0;
                    }
                }
            }
            out
        }

        let key_a =
            PublicKey::from_slice(pk_a.as_bytes()).map_err(|_| "bad pubkey")?;
        let key_b =
            PublicKey::from_slice(pk_b.as_bytes()).map_err(|_| "bad pubkey")?;

        // Sort ascending (mirrors Fiber's order_things_for_musig2)
        let (pk1, pk2) = if pk_a.as_bytes() <= pk_b.as_bytes() {
            (key_a, key_b)
        } else {
            (key_b, key_a)
        };

        let pk1_bytes = pk1.serialize();
        let pk2_bytes = pk2.serialize();

        // L = tagged_hash("KeyAgg list", pk1 || pk2)
        let l = tagged_hash(b"KeyAgg list", &[&pk1_bytes, &pk2_bytes]);

        // a1 = int(tagged_hash("KeyAgg coefficient", L || pk1)) mod n
        let a1_hash = tagged_hash(b"KeyAgg coefficient", &[&l, &pk1_bytes]);
        let a1 = Scalar::from_be_bytes(reduce_mod_n(&a1_hash))
            .map_err(|_| "bad scalar")?;

        let secp = Secp256k1::new();
        // effective1 = a1 * P1;  MuSig2*: coefficient 1 for P2
        let effective1 = pk1
            .mul_tweak(&secp, &a1)
            .map_err(|_| "tweak")?;
        let agg = PublicKey::combine_keys(&[&effective1, &pk2])
            .map_err(|_| "combine")?;

        Ok(agg.x_only_public_key().0.serialize())
    }

    /// Seed the Fiber channel CellDep referenced by a match with the MuSig2 funding lock args.
    pub fn seed_match_channel_cell(
        rpc: &mut FakeRpcClient,
        order_args: &OrderArgs,
        match_args: &MatchArgs,
        capacity: u64,
    ) {
        seed_channel_cell(
            rpc,
            &match_args.channel_outpoint,
            capacity,
            funding_lock_args(
                &order_args.fiber_pubkey,
                &match_args.fiber_pubkey,
            ),
        );
    }

    // --- Address / Lock Script builders ---

    pub fn fake_address() -> Address {
        let lock = always_success_script(vec![]);
        let payload = AddressPayload::from(lock);
        Address::new(ckb_cinnabar_calculator::rpc::Network::Fake, payload)
    }

    fn build_opticrum_lock(args: Vec<u8>, skeleton: &TransactionSkeleton) -> eyre::Result<Script> {
        use ckb_cinnabar_calculator::skeleton::ScriptEx;
        ScriptEx::Reference("opticrum".into(), args).to_script(skeleton)
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

    pub fn seed_channel_cell(
        rpc: &mut FakeRpcClient,
        outpoint: &OutPoint,
        capacity: u64,
        funding_lock_args: [u8; 20],
    ) {
        // Flat type script: blake2b_256(code_hash=[0xCC;32] || Data1 || empty)
        // matches FIBER_FUNDING_TYPE_ID_MOCK in the contract.
        let channel_type = Script::new_builder()
            .code_hash(H256([0xCCu8; 32]).pack())
            .hash_type(ScriptHashType::Data1.into())
            .args(Bytes::new().pack())
            .build();
        let lock = Script::new_builder()
            .code_hash(H256([0xDDu8; 32]).pack())
            .hash_type(ScriptHashType::Data1.into())
            .args(Bytes::copy_from_slice(&funding_lock_args).pack())
            .build();
        let cell = CellOutputEx::new_from_scripts(
            lock,
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
async fn match_skeleton_channel_celldep() -> eyre::Result<()> {
    use ckb_cinnabar_calculator::operation::Log;

    const CONTRACT_MOCK: [u8; 32] = [
        0x77, 0xc9, 0x16, 0x3a, 0xdd, 0xbf, 0x87, 0xc8, 0x05, 0xbe, 0x3b, 0x6c, 0x85, 0x69, 0xb8,
        0xe0, 0x15, 0xa4, 0xca, 0x0e, 0xf3, 0xc6, 0x89, 0x15, 0x02, 0x34, 0xf0, 0xc8, 0x02, 0xa7,
        0x69, 0x00,
    ];

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
    faker::seed_match_channel_cell(
        &mut rpc,
        &order_args,
        &match_args,
        faker::CHANNEL_CAPACITY,
    );

    let order_info = faker::to_order_info(&packed, order_args, order_data);
    let instruction = match_order(seller, order_info, match_args.clone());
    let mut log = Log::new();
    instruction.run(&rpc, &mut skeleton, &mut log).await?;

    let dep = skeleton
        .get_celldep_by_name("fiber_channel")
        .expect("fiber_channel celldep");
    let type_hash: [u8; 32] = dep.output.calc_type_hash().expect("type hash").into();
    assert_eq!(type_hash, CONTRACT_MOCK);
    let capacity: u64 = dep.output.output.capacity().unpack();
    assert!(capacity >= faker::CHANNEL_CAPACITY, "channel capacity must satisfy order");
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
    assert_eq!(parsed.channel_outpoint.tx_hash, match_args.channel_outpoint.tx_hash);
    assert_eq!(parsed.channel_outpoint.index, match_args.channel_outpoint.index);

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
            tx_hash == match_args.channel_outpoint.tx_hash && index == match_args.channel_outpoint.index
        })
        .expect("channel celldep in resolved tx");
    let resolved_cap: u64 = channel_meta.cell_output.capacity().unpack();
    assert!(resolved_cap >= faker::CHANNEL_CAPACITY);

    Ok(())
}

#[tokio::test]
async fn channel_celldep_matches_contract_mock() -> eyre::Result<()> {
    use ckb_cinnabar_calculator::{
        instruction::Instruction,
        operation::{basic::AddCellDep, Log},
        re_exports::ckb_types::core::DepType,
    };

    const CONTRACT_MOCK: [u8; 32] = [
        0x77, 0xc9, 0x16, 0x3a, 0xdd, 0xbf, 0x87, 0xc8, 0x05, 0xbe, 0x3b, 0x6c, 0x85, 0x69, 0xb8,
        0xe0, 0x15, 0xa4, 0xca, 0x0e, 0xf3, 0xc6, 0x89, 0x15, 0x02, 0x34, 0xf0, 0xc8, 0x02, 0xa7,
        0x69, 0x00,
    ];

    let mut rpc = FakeRpcClient::default();
    let mut skeleton = faker::celldeps_prepared_skeleton(&rpc).await?;
    let order_args = OrderArgs::new(faker::fiber_pubkey(), faker::user_lock_hash());
    let match_args = MatchArgs::new(
        order_args.clone(),
        faker::channel_outpoint(),
        faker::user_lock_hash(),
        faker::seller_fiber_pubkey(),
    );
    faker::seed_match_channel_cell(
        &mut rpc,
        &order_args,
        &match_args,
        faker::CHANNEL_CAPACITY,
    );

    let mut log = Log::new();
    Instruction::<FakeRpcClient>::new(vec![Box::new(AddCellDep {
        name: "fiber_channel".into(),
        tx_hash: match_args.channel_outpoint.tx_hash.into(),
        index: match_args.channel_outpoint.index,
        dep_type: DepType::Code,
        with_data: true,
    })])
    .run(&rpc, &mut skeleton, &mut log)
    .await?;

    let dep = skeleton
        .get_celldep_by_name("fiber_channel")
        .expect("channel celldep");
    let type_hash: [u8; 32] = dep.output.calc_type_hash().expect("type hash").into();
    assert_eq!(type_hash, CONTRACT_MOCK);
    Ok(())
}

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
        faker::seller_fiber_pubkey(),
    );

    let packed = faker::seed_order_cell(
        &mut rpc,
        &skeleton,
        &order_args,
        &order_data,
        faker::RENT_CAPACITY,
    )?;
    faker::seed_match_channel_cell(
        &mut rpc,
        &order_args,
        &match_args,
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
async fn test_match_order_rejects_wrong_seller_fiber_pubkey() -> eyre::Result<()> {
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
        faker::wrong_seller_fiber_pubkey(),
    );

    let packed = faker::seed_order_cell(
        &mut rpc,
        &skeleton,
        &order_args,
        &order_data,
        faker::RENT_CAPACITY,
    )?;
    // Channel lock args were built from the real seller key, not the wrong one in MatchArgs.
    faker::seed_match_channel_cell(
        &mut rpc,
        &order_args,
        &MatchArgs::new(
            order_args.clone(),
            faker::channel_outpoint(),
            faker::user_lock_hash(),
            faker::seller_fiber_pubkey(),
        ),
        faker::CHANNEL_CAPACITY,
    );

    let order_info = faker::to_order_info(&packed, order_args, order_data);
    let instruction = match_order(seller, order_info, match_args);

    let result = TransactionSimulator::default()
        .skeleton(skeleton)
        .link_cell_to_header(rpc.get_outpoint_to_headers())
        .async_verify(&rpc, vec![instruction], DEFUALT_MAX_CYCLES)
        .await;
    assert!(result.is_err(), "match with wrong seller fiber pubkey must fail");
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
    faker::seed_channel_cell(
        &mut rpc,
        &faker::channel_outpoint(),
        faker::CHANNEL_CAPACITY,
        faker::funding_lock_args(&order_args.fiber_pubkey, &faker::wrong_seller_fiber_pubkey()),
    );

    let order_info = faker::to_order_info(&packed, order_args, order_data);
    let instruction = match_order(seller, order_info, match_args);

    let result = TransactionSimulator::default()
        .skeleton(skeleton)
        .link_cell_to_header(rpc.get_outpoint_to_headers())
        .async_verify(&rpc, vec![instruction], DEFUALT_MAX_CYCLES)
        .await;
    assert!(
        result.is_err(),
        "match with channel funding lock not matching aggregated pubkey must fail"
    );
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
    faker::seed_match_channel_cell(
        &mut rpc,
        &order_args,
        &match_args,
        faker::CHANNEL_CAPACITY,
    );
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
// Unit Tests: Encoding + Rent Math
// ---------------------------------------------------------------------------

#[test]
fn funding_lock_args_match_parsed_match_args() {
    let order_args = OrderArgs::new(faker::fiber_pubkey(), [0x01; 32]);
    let match_args = MatchArgs::new(
        order_args.clone(),
        faker::channel_outpoint(),
        [0x02; 32],
        faker::seller_fiber_pubkey(),
    );
    let parsed = MatchArgs::from_slice(&match_args.to_bytes()).expect("parse match args");
    let lock = faker::funding_lock_args(&parsed.order_args.fiber_pubkey, &parsed.fiber_pubkey);
    // Verify the lock args match direct key aggregation (using inline test helper)
    let xonly = faker::aggregate_funding_keys_xonly(
        &parsed.order_args.fiber_pubkey,
        &parsed.fiber_pubkey,
    )
    .expect("aggregate");
    let hash = ckb_cinnabar_calculator::re_exports::ckb_hash::blake2b_256(xonly);
    assert_eq!(lock.as_slice(), &hash[..20]);
}

#[test]
fn mock_fiber_funding_type_hash() {
    use ckb_cinnabar_calculator::re_exports::ckb_types::{
        bytes::Bytes,
        core::ScriptHashType,
        packed::Script,
        prelude::{Builder, Entity, Pack, Unpack},
        H256,
    };
    const CONTRACT_MOCK: [u8; 32] = [
        0x77, 0xc9, 0x16, 0x3a, 0xdd, 0xbf, 0x87, 0xc8, 0x05, 0xbe, 0x3b, 0x6c, 0x85, 0x69, 0xb8,
        0xe0, 0x15, 0xa4, 0xca, 0x0e, 0xf3, 0xc6, 0x89, 0x15, 0x02, 0x34, 0xf0, 0xc8, 0x02, 0xa7,
        0x69, 0x00,
    ];
    let script = Script::new_builder()
        .code_hash(H256([0xCCu8; 32]).pack())
        .hash_type(ScriptHashType::Data1.into())
        .args(Bytes::new().pack())
        .build();
    let hash: [u8; 32] = script.calc_script_hash().unpack();
    assert_eq!(hash, CONTRACT_MOCK, "keep in sync with FIBER_FUNDING_TYPE_ID_MOCK");
}

#[test]
fn test_args_encoding() {
    let order = OrderArgs::new(CompressedPubkey::new([0x03u8; 33]), [0x01u8; 32]);
    let args = order.to_bytes();
    assert_eq!(args.len(), ORDER_ARGS_LEN);

    let match_args = MatchArgs::new(
        order.clone(),
        faker::channel_outpoint(),
        [0x02u8; 32],
        faker::seller_fiber_pubkey(),
    );
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

    let total_rent = (rent_per_block * 1000_f64) as u64;
    assert_eq!(total_rent, 1000);
}

#[test]
fn test_complete_lifecycle_types() {
    let order_args = OrderArgs::new(CompressedPubkey::new([0x03u8; 33]), [0x01u8; 32]);
    let order_bytes = order_args.to_bytes();
    assert_eq!(order_bytes.len(), ORDER_ARGS_LEN);

    let match_args = MatchArgs::new(
        order_args.clone(),
        faker::channel_outpoint(),
        [0x02u8; 32],
        faker::seller_fiber_pubkey(),
    );
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
