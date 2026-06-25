use std::str::FromStr;

use ckb_cinnabar::calculator::{
    address::Address,
    instruction::predefined::balance_and_sign,
    re_exports::{
        ckb_sdk::constants::ONE_CKB,
        ckb_types::{h256, prelude::hex_string},
        eyre,
        secp256k1::SecretKey,
        tokio,
    },
    rpc::RpcClient,
    TransactionCalculator,
};
use opticrum_calculator::{
    calculator::match_order,
    reader::scan_orders,
    types::{CompressedPubkey, MatchArgs, OutPoint},
};

#[tokio::main]
pub async fn main() -> eyre::Result<()> {
    let seller_address = Address::from_str(
        "ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqtz32u8mgzk57zdtt6n62z4y2zyh8egkdcahyxk3",
    )
    .unwrap();
    let seller_lock_hash =
        h256!("0x48c1f38d2cad56462319ec5a2b241e0c49e483eb9e5225e77de0359b1c9e60e1");
    let seller_fiber_pubkey = CompressedPubkey::from_slice(&hex::decode(
        "025bfeb476486c0464cb440c3ef2033fc34f0dd9b436579d4eceb430960633573f",
    )?)
    .unwrap();
    let channel_tx_hash =
        h256!("0x2e03493880b7e09b9ecabfd16e053bfb5cf1e0e7ecbd462e7cce6011b1b91f84");
    let channel_index: u32 = 0;
    let order_index: usize = 2; // pick which scanned order to match

    let rpc = RpcClient::new_testnet();

    // Scan for orders, pick one by index
    let orders = scan_orders(&rpc).await?;
    let order = orders
        .get(order_index)
        .expect("No order at that index — run scan_orders first");

    println!(
        "Matching order: {}:{}",
        hex_string(&order.order_outpoint.tx_hash),
        order.order_outpoint.index
    );
    println!(
        "  channel_capacity: {} CKB",
        order.order_data.channel_capacity as f64 / ONE_CKB as f64
    );

    let match_args = MatchArgs::new(
        order.order_args.clone(),
        OutPoint::new(channel_tx_hash.into(), channel_index),
        seller_lock_hash.into(),
        seller_fiber_pubkey,
    );

    let match_tx = match_order::<RpcClient>(seller_address.clone(), order.clone(), match_args);
    let balance = balance_and_sign(
        &seller_address,
        SecretKey::from_slice(&hex::decode(
            "736632957c05bf1d2eb480e1a53fa509bd160b842cd8fcd42af7f82ccdf14a16",
        )?)
        .unwrap(),
        1000,
    );

    let (tx, _) = TransactionCalculator::new(vec![match_tx, balance])
        .new_skeleton(&rpc)
        .await?;

    let tx_hash = tx.send_and_wait(&rpc, 0, None).await?;
    println!("Order matched! Tx hash: {}", hex_string(tx_hash.as_bytes()));

    Ok(())
}
