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
    calculator::create_order,
    config::ABOUT_ONE_DAY_BLOCKS,
    types::{AnnualYield, CompressedPubkey, OrderArgs, OrderData},
};

#[tokio::main]
pub async fn main() -> eyre::Result<()> {
    let buyer_address = Address::from_str(
        "ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqv5puz2ee96nuh9nmc6rtm0n8v7agju4rgdmxlnk",
    )
    .unwrap();

    // OrderArgs: fiber_pubkey (33 bytes) + buyer_lock_hash (32 bytes)
    let fiber_pubkey = CompressedPubkey::from_slice(&hex::decode(
        "02aa3beb0d770fe835db99bf894fb2d9afaf4df0d5ec1871fad731d4fc6c90faed",
    )?)
    .unwrap();
    let buyer_lock_hash =
        h256!("0xc97b60038e61afcba164ec5a1c49d4b2e573b2c2166ff03522bd8c6dbf2c7737");
    let order_args = OrderArgs::new(fiber_pubkey, buyer_lock_hash.into());

    // OrderData: xudt_amount (u128, ignored for CKB orders) + channel_capacity + escrow_blocks
    let channel_capacity = 1000u64 * ONE_CKB;
    let escrow_blocks = 10 * ABOUT_ONE_DAY_BLOCKS; // ~10 days
    let order_data = OrderData::new(0, channel_capacity, escrow_blocks);

    // Annual yield percentage (e.g. 5 = 5% APR)
    let annual_yield = AnnualYield(5);

    let prepare = DefaultInstruction::new(vec![Box::new(AddSecp256k1SighashCellDep {})]);
    let create = create_order::<RpcClient>(
        buyer_address.clone(),
        &order_args,
        &order_data,
        annual_yield,
        None, // no xUDT — pure CKB order
    );
    let balance = balance_and_sign_with_ckb_cli(&buyer_address, 1000, None);

    let rpc = RpcClient::new_testnet();
    let (tx, _) = TransactionCalculator::new(vec![prepare, create, balance])
        .new_skeleton(&rpc)
        .await?;

    let tx_hash = tx.send_and_wait(&rpc, 0, None).await?;
    println!("Order created! Tx hash: {}", hex_string(tx_hash.as_bytes()));

    Ok(())
}
