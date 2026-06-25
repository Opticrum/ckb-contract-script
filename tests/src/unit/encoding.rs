use opticrum_calculator::types::{
    CompressedPubkey, MatchArgs, MatchData, OrderArgs, OrderData, MATCH_ARGS_LEN, MATCH_DATA_LEN,
    ORDER_ARGS_LEN, ORDER_DATA_LEN,
};

use crate::faker;

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
