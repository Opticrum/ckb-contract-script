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
        AddFakeContractCelldepByName, FakeRpcClient,
    },
    skeleton::CellOutputEx,
    TransactionCalculator,
};
use opticrum_calculator::types::{
    CompressedPubkey, MatchArgs, MatchData, MatchInfo, OrderArgs, OrderData, OrderInfo, OutPoint,
};

// --- Constants ---

pub const CHANNEL_CAPACITY: u64 = 100_000_000_000;
/// Per-block rent rate in shannons (1 shannon = 10^-8 CKB).
pub const SHANNONS_PER_BLOCK: u64 = 1000;
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

/// Distinct seller lock hash for tests where buyer ≠ seller auth matters.
pub fn seller_lock_hash() -> [u8; 32] {
    let mut hash = [0u8; 32];
    let script_hash = ckb_cinnabar_calculator::re_exports::ckb_hash::blake2b_256(
        always_success_script(vec![0x01]).as_slice(),
    );
    hash.copy_from_slice(&script_hash);
    hash
}

/// Seed a user cell with a specific lock (for seller vs buyer distinction).
pub fn seed_user_cell_with_lock(
    rpc: &mut FakeRpcClient,
    capacity: u64,
    lock_args: Vec<u8>,
) -> Address {
    let lock = always_success_script(lock_args);
    let cell = CellOutputEx::new_from_scripts(
        lock.clone(),
        None,
        vec![],
        Some(Capacity::shannons(capacity)),
    )
    .expect("build user cell");
    let header = fake_header_view(1, random_u64(), random_u64());
    rpc.insert_fake_cell(fake_outpoint(), cell, Some(header));
    let payload = AddressPayload::from(lock);
    Address::new(ckb_cinnabar_calculator::rpc::Network::Fake, payload)
}

/// Hardcoded compressed secp256k1 pubkey (33 bytes, 0x02 prefix = even Y).
/// Used as the buyer's fiber_pubkey in OrderArgs for counterparty identification.
pub fn fiber_pubkey() -> CompressedPubkey {
    CompressedPubkey::new([0x02u8; 33])
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

// --- Channel cell seeding ---

/// Seed the Fiber channel CellDep referenced by a match.
pub fn seed_match_channel_cell(rpc: &mut FakeRpcClient, match_args: &MatchArgs, capacity: u64) {
    seed_channel_cell(rpc, &match_args.channel_outpoint, capacity, [0xABu8; 20]);
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
    let header = fake_header_view(1, random_u64(), random_u64());
    rpc.insert_fake_cell(fake_outpoint(), cell, Some(header));
}

pub fn seed_channel_cell(
    rpc: &mut FakeRpcClient,
    outpoint: &OutPoint,
    capacity: u64,
    lock_args: [u8; 20],
) {
    let channel_type = Script::new_builder()
        .code_hash(H256([0xCCu8; 32]).pack())
        .hash_type(ScriptHashType::Data1.into())
        .args(Bytes::new().pack())
        .build();
    let lock = Script::new_builder()
        .code_hash(H256(CONTRACT_MOCK).pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::copy_from_slice(&lock_args).pack())
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
    lock_args: [u8; 20],
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
        .args(Bytes::copy_from_slice(&lock_args).pack())
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
        fiber_address: None,
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
