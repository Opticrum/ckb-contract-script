use crate::faker;
use opticrum_calculator::types::{CompressedPubkey, MatchArgs, OrderArgs};

/// Real Fiber Network keys from production.
const FIBER_BUYER_PK: &str = "028db409b3f88502105c58cf698d0f16c13d5cb167f4c968973a356776f0e03f9e";
const FIBER_SELLER_PK: &str = "025bfeb476486c0464cb440c3ef2033fc34f0dd9b436579d4eceb430960633573f";
/// Expected blake160(x-only MuSig2 aggregated key) from real Fiber output.
const FIBER_EXPECTED_LOCK_ARGS: &str = "a7e3591fb98a01dd12ad38671788047fc23694bf";

fn pubkey_from_hex(hex: &str) -> CompressedPubkey {
    let bytes = hex::decode(hex).expect("valid hex");
    CompressedPubkey::from_slice(&bytes).expect("valid pubkey")
}

fn blake160_of(xonly: &[u8; 32]) -> [u8; 20] {
    use ckb_cinnabar_calculator::re_exports::ckb_hash;
    let hash = ckb_hash::blake2b_256(*xonly);
    let mut out = [0u8; 20];
    out.copy_from_slice(&hash[..20]);
    out
}

/// Verify MuSig2* key aggregation against real Fiber production data.
///
/// This test uses the same pubkeys from a real Fiber channel funding transaction.
/// The expected lock args (blake160 of the x-only aggregated key) were obtained
/// from Fiber's `get_deterministic_musig2_agg_context` output.
#[test]
fn musig2_key_aggregation_real_fiber_keys() {
    let buyer = pubkey_from_hex(FIBER_BUYER_PK);
    let seller = pubkey_from_hex(FIBER_SELLER_PK);

    // Compute via the musig2 crate (Rust reference BIP-327 MuSig2*)
    let xonly = faker::aggregate_funding_keys_xonly(&buyer, &seller)
        .expect("musig2 key aggregation should succeed");
    let lock_args = blake160_of(&xonly);

    println!("musig2 crate x-only: 0x{}", hex::encode(xonly));
    println!("lock_args (blake160): 0x{}", hex::encode(lock_args));
    println!("expected (Fiber):     0x{}", FIBER_EXPECTED_LOCK_ARGS);

    assert_eq!(
        hex::encode(lock_args),
        FIBER_EXPECTED_LOCK_ARGS,
        "MuSig2* lock_args must match Fiber's production output. \
         The C function compute_musig2_key_aggregation_xonly likely diverges \
         from Fiber's get_deterministic_musig2_agg_context."
    );
}

/// Verify order-independence of key aggregation.
#[test]
fn musig2_key_aggregation_order_independent() {
    let buyer = pubkey_from_hex(FIBER_BUYER_PK);
    let seller = pubkey_from_hex(FIBER_SELLER_PK);
    let x_ab = faker::aggregate_funding_keys_xonly(&buyer, &seller).unwrap();
    let x_ba = faker::aggregate_funding_keys_xonly(&seller, &buyer).unwrap();
    assert_eq!(x_ab, x_ba, "Key aggregation must be order-independent");
}

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
    let xonly =
        faker::aggregate_funding_keys_xonly(&parsed.order_args.fiber_pubkey, &parsed.fiber_pubkey)
            .expect("aggregate");
    let hash = ckb_cinnabar_calculator::re_exports::ckb_hash::blake2b_256(xonly);
    assert_eq!(lock.as_slice(), &hash[..20]);
}

#[test]
fn mock_fiber_funding_type_hash() {
    use ckb_cinnabar_calculator::re_exports::ckb_types::{
        bytes::Bytes, core::ScriptHashType,
        packed::Script, prelude::{Builder, Entity, Pack, Unpack}, H256,
    };
    let script = Script::new_builder()
        .code_hash(H256([0xCCu8; 32]).pack())
        .hash_type(ScriptHashType::Data1.into())
        .args(Bytes::new().pack())
        .build();
    let hash: [u8; 32] = script.calc_script_hash().unpack();
    assert_eq!(hash, faker::CONTRACT_MOCK, "keep in sync with FIBER_FUNDING_TYPE_ID_MOCK");
}
