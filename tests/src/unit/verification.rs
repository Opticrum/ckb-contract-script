use crate::faker;
use opticrum_calculator::types::{MatchArgs, OrderArgs};

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
        bytes::Bytes,
        core::ScriptHashType,
        packed::Script,
        prelude::{Builder, Entity, Pack, Unpack},
        H256,
    };
    let script = Script::new_builder()
        .code_hash(H256([0xCCu8; 32]).pack())
        .hash_type(ScriptHashType::Data1.into())
        .args(Bytes::new().pack())
        .build();
    let hash: [u8; 32] = script.calc_script_hash().unpack();
    assert_eq!(
        hash,
        faker::CONTRACT_MOCK,
        "keep in sync with FIBER_FUNDING_TYPE_ID_MOCK"
    );
}
