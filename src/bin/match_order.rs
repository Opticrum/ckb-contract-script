use std::str::FromStr;

use ckb_cinnabar::calculator::{
    address::Address,
    instruction::{predefined::balance_and_sign, DefaultInstruction},
    operation::basic::AddSecp256k1SighashCellDep,
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
    types::{MatchArgs, OutPoint},
};

#[tokio::main]
pub async fn main() -> eyre::Result<()> {
    let _ = dotenvy::dotenv();

    let seller_address = Address::from_str(
        "ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqtz32u8mgzk57zdtt6n62z4y2zyh8egkdcahyxk3",
    )
    .unwrap();
    let seller_lock_hash =
        h256!("0x48c1f38d2cad56462319ec5a2b241e0c49e483eb9e5225e77de0359b1c9e60e1");
    let channel_tx_hash =
        h256!("0x74b41bedb5f0f9add71bcbff7f822f916781e356fdd016e195305f6e85956983");
    let channel_index: u32 = 0;
    let order_index: usize = 2; // pick which scanned order to match

    let seller_pk_hex = std::env::var("SELLER_PRIVATE_KEY").map_err(|_| {
        eyre::eyre!(
            "SELLER_PRIVATE_KEY not set.\n\
             Create a .env file or export the environment variable:\n\
             SELLER_PRIVATE_KEY=<64-char hex private key>"
        )
    })?;
    let seller_key = SecretKey::from_slice(&hex::decode(&seller_pk_hex)?).unwrap();

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
    );

    let prepare = DefaultInstruction::new(vec![Box::new(AddSecp256k1SighashCellDep {})]);
    let match_tx = match_order::<RpcClient>(seller_address.clone(), order.clone(), match_args);
    let balance = balance_and_sign(&seller_address, seller_key, 1000);

    let (tx, _) = TransactionCalculator::new(vec![prepare, match_tx, balance])
        .new_skeleton(&rpc)
        .await?;

    let tx_hash = tx.send_and_wait(&rpc, 0, None).await?;
    println!("Order matched! Tx hash: {}", hex_string(tx_hash.as_bytes()));

    Ok(())
}
