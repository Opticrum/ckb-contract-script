use std::str::FromStr;

use ckb_cinnabar::calculator::{
    address::Address,
    instruction::{predefined::balance_and_sign_with_ckb_cli, DefaultInstruction},
    operation::basic::AddSecp256k1SighashCellDep,
    re_exports::{
        ckb_sdk::constants::ONE_CKB,
        ckb_types::{h256, prelude::hex_string},
        eyre, tokio,
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
        h256!("0x48c1f38d2cad56462319ec5a2b241e0c49e483eb9e5225e77de0359b1c9e60e1"); // TODO: fill real seller lock hash
    let channel_tx_hash = [0u8; 32]; // TODO: fill pre-created Fiber channel tx hash
    let channel_index: u32 = 0; // TODO: fill channel output index
    let order_index: usize = 0; // pick which scanned order to match

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

    let seller_fiber_pubkey = CompressedPubkey::new([0u8; 33]); // TODO: fill real seller funding pubkey

    let match_args = MatchArgs::new(
        order.order_args.clone(),
        OutPoint::new(channel_tx_hash, channel_index),
        seller_lock_hash.into(),
        seller_fiber_pubkey,
    );

    let prepare = DefaultInstruction::new(vec![Box::new(AddSecp256k1SighashCellDep {})]);
    let match_tx = match_order::<RpcClient>(seller_address.clone(), order.clone(), match_args);
    let balance = balance_and_sign_with_ckb_cli(&seller_address, 1000, None);

    let (tx, _) = TransactionCalculator::new(vec![prepare, match_tx, balance])
        .new_skeleton(&rpc)
        .await?;

    let tx_hash = tx.send_and_wait(&rpc, 0, None).await?;
    println!("Order matched! Tx hash: {}", hex_string(tx_hash.as_bytes()));

    Ok(())
}
