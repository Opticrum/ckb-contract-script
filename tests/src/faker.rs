use ckb_cinnabar_calculator::re_exports::ckb_types::packed::OutPoint as PackedOutPoint;
use ckb_cinnabar_calculator::simulation::random_hash;
use ckb_cinnabar_calculator::skeleton::TransactionSkeleton;
use ckb_cinnabar_calculator::{
    address::{Address, AddressPayload},
    instruction::Instruction,
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
        AddFakeContractCelldep, AddFakeContractCelldepByName, FakeRpcClient,
    },
    skeleton::CellOutputEx,
    TransactionCalculator,
};
use opticrum_calculator::types::{
    CompressedPubkey, MatchArgs, MatchData, MatchInfo, OrderArgs, OrderData, OrderInfo, OutPoint,
};

// --- Constants ---

pub const CHANNEL_CAPACITY: u64 = 100_000_000_000;
pub const ESCROW_BLOCKS: u64 = 43200;
pub const RENT_CAPACITY: u64 = 30_000_000_000;
pub const MATCH_CREATED_BLOCK: u64 = 1000;

/// Block number where Order cells are seeded (must be < CHANNEL_CREATED_BLOCK).
pub const ORDER_CREATED_BLOCK: u64 = 10;
/// Block number where Channel cells are seeded (must be > ORDER_CREATED_BLOCK
/// to pass the temporal check).
pub const CHANNEL_CREATED_BLOCK: u64 = 20;

// --- Identity helpers ---

/// Compute the blake2b_256 hash of always_success_script(vec![]).
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

/// Must stay in sync with `FIBER_FUNDING_TYPE_ID_MOCK` in the contract.
pub const CONTRACT_MOCK: [u8; 32] = [
    0x77, 0xc9, 0x16, 0x3a, 0xdd, 0xbf, 0x87, 0xc8, 0x05, 0xbe, 0x3b, 0x6c, 0x85, 0x69, 0xb8, 0xe0,
    0x15, 0xa4, 0xca, 0x0e, 0xf3, 0xc6, 0x89, 0x15, 0x02, 0x34, 0xf0, 0xc8, 0x02, 0xa7, 0x69, 0x00,
];

pub fn channel_outpoint() -> OutPoint {
    let out_point = fake_outpoint();
    OutPoint::new(out_point.tx_hash().unpack(), out_point.index().unpack())
}

pub fn random_u64() -> u64 {
    let hash = random_hash();
    u64::from_le_bytes(hash[..8].try_into().unwrap())
}

// --- MuSig2 helpers ---

/// Fiber funding lock args: blake160(x-only MuSig2 aggregated key).
pub fn funding_lock_args(buyer: &CompressedPubkey, seller: &CompressedPubkey) -> [u8; 20] {
    let xonly = aggregate_funding_keys_xonly(buyer, seller).unwrap();
    let hash = ckb_cinnabar_calculator::re_exports::ckb_hash::blake2b_256(xonly);
    let mut args = [0u8; 20];
    args.copy_from_slice(&hash[..20]);
    args
}

/// BIP-327 MuSig2* key aggregation via the canonical `musig2` crate.
///
/// The contract binary calls our C function (`compute_musig2_key_aggregation_xonly`);
/// this helper produces the expected result using the `musig2` reference
/// implementation, so passing tests mean the C function ≡ musig2.
pub fn aggregate_funding_keys_xonly(
    pk_a: &CompressedPubkey,
    pk_b: &CompressedPubkey,
) -> Result<[u8; 32], &'static str> {
    use musig2::KeyAggContext;
    use secp256k1::PublicKey;

    let key_a = PublicKey::from_slice(pk_a.as_bytes()).map_err(|_| "bad pubkey")?;
    let key_b = PublicKey::from_slice(pk_b.as_bytes()).map_err(|_| "bad pubkey")?;

    let (k1, k2) = if pk_a.as_bytes() <= pk_b.as_bytes() {
        (key_a, key_b)
    } else {
        (key_b, key_a)
    };

    let ctx = KeyAggContext::new([k1, k2]).map_err(|_| "keyagg")?;
    let agg: PublicKey = ctx.aggregated_pubkey();
    Ok(agg.x_only_public_key().0.serialize())
}

/// Seed the Fiber channel CellDep referenced by a match.
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
        funding_lock_args(&order_args.fiber_pubkey, &match_args.fiber_pubkey),
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
pub async fn celldeps_prepared_skeleton(rpc: &FakeRpcClient) -> eyre::Result<TransactionSkeleton> {
    let secp256k1_data_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../secp256k1/ckb-lib-secp256k1/build/secp256k1_data"
    );
    let secp256k1_data = std::fs::read(secp256k1_data_path)?;

    let prepare = Instruction::<FakeRpcClient>::new(vec![
        Box::new(AddFakeAlwaysSuccessCelldep {}),
        Box::new(AddFakeContractCelldepByName {
            contract: "opticrum".to_string(),
            type_id_args: Some(H256::default()),
            contract_binary_path: "../build/release".to_string(),
        }),
        Box::new(AddFakeContractCelldep {
            name: "secp256k1_data".to_string(),
            contract_data: secp256k1_data,
            type_id_args: None,
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
    let header = fake_header_view(1, random_u64(), random_u64());
    rpc.insert_fake_cell(fake_outpoint(), cell, Some(header));
}

pub fn seed_channel_cell(
    rpc: &mut FakeRpcClient,
    outpoint: &OutPoint,
    capacity: u64,
    funding_lock_args: [u8; 20],
) {
    let channel_type = Script::new_builder()
        .code_hash(H256([0xCCu8; 32]).pack())
        .hash_type(ScriptHashType::Data1.into())
        .args(Bytes::new().pack())
        .build();
    let lock = Script::new_builder()
        .code_hash(H256(CONTRACT_MOCK).pack())
        .hash_type(ScriptHashType::Type.into())
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
    let header = fake_header_view(CHANNEL_CREATED_BLOCK, random_u64(), random_u64());
    rpc.insert_fake_cell(packed, cell, Some(header));
}

/// Seed a channel cell at a custom block number (for temporal check testing).
pub fn seed_channel_cell_at(
    rpc: &mut FakeRpcClient,
    outpoint: &OutPoint,
    capacity: u64,
    funding_lock_args: [u8; 20],
    block_number: u64,
) {
    let channel_type = Script::new_builder()
        .code_hash(H256([0xCCu8; 32]).pack())
        .hash_type(ScriptHashType::Data1.into())
        .args(Bytes::new().pack())
        .build();
    let lock = Script::new_builder()
        .code_hash(H256(CONTRACT_MOCK).pack())
        .hash_type(ScriptHashType::Type.into())
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
    let header = fake_header_view(block_number, random_u64(), random_u64());
    rpc.insert_fake_cell(packed, cell, Some(header));
}

pub fn seed_header(rpc: &mut FakeRpcClient, block_number: u64, timestamp: u64) {
    rpc.insert_fake_header(fake_header_view(block_number, timestamp, random_u64()));
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
    let header = fake_header_view(ORDER_CREATED_BLOCK, random_u64(), random_u64());
    rpc.insert_fake_cell(outpoint.clone(), cell, Some(header));
    Ok(outpoint)
}

/// Seed an order cell at a custom block number (for temporal check testing).
pub fn seed_order_cell_at(
    rpc: &mut FakeRpcClient,
    skeleton: &TransactionSkeleton,
    order_args: &OrderArgs,
    order_data: &OrderData,
    capacity: u64,
    block_number: u64,
) -> eyre::Result<PackedOutPoint> {
    let lock = build_opticrum_lock(order_args.to_bytes().to_vec(), skeleton)?;
    let cell = CellOutputEx::new_from_scripts(
        lock,
        None,
        order_data.to_bytes().to_vec(),
        Some(Capacity::shannons(capacity)),
    )?;
    let outpoint = fake_outpoint();
    let header = fake_header_view(block_number, random_u64(), random_u64());
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
    let header = fake_header_view(creation_block, random_u64(), random_u64());
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
