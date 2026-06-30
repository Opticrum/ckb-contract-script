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
use opticrum_calculator::{calculator::update_match_buyer, reader::scan_matches};

#[tokio::main]
pub async fn main() -> eyre::Result<()> {
    let _ = dotenvy::dotenv();

    let buyer_address = Address::from_str(
        "ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqv5puz2ee96nuh9nmc6rtm0n8v7agju4rgdmxlnk",
    )
    .unwrap();
    let buyer_lock_hash =
        h256!("0xc97b60038e61afcba164ec5a1c49d4b2e573b2c2166ff03522bd8c6dbf2c7737");

    let match_index: usize = 0; // pick which scanned match to decline from
    let capacity_delta: i64 = -((5000u64 * ONE_CKB) as i64); // withdraw 5,000 CKB

    let buyer_pk_hex = std::env::var("BUYER_PRIVATE_KEY").map_err(|_| {
        eyre::eyre!(
            "BUYER_PRIVATE_KEY not set.\n\
             Create a .env file or export the environment variable:\n\
             BUYER_PRIVATE_KEY=<64-char hex private key>"
        )
    })?;
    let buyer_key = SecretKey::from_slice(&hex::decode(&buyer_pk_hex)?).unwrap();

    let rpc = RpcClient::new_testnet();

    // Scan for matches, pick one by index
    let matches = scan_matches(&rpc, None).await?;
    let match_info = matches
        .get(match_index)
        .expect("No match at that index — run scan_matches first");

    // Verify this match belongs to the buyer
    let buyer_lock_hash_bytes: [u8; 32] = buyer_lock_hash.into();
    if match_info.match_args.order_args.buyer_lock_hash != buyer_lock_hash_bytes {
        eyre::bail!(
            "Match at index {} does not belong to buyer (lock_hash mismatch)",
            match_index
        );
    }

    let current_xudt = match_info
        .xudt
        .as_ref()
        .map(|x| x.amount)
        .unwrap_or(0);

    println!(
        "Declining rent from match: {}:{}",
        hex_string(&match_info.match_outpoint.tx_hash),
        match_info.match_outpoint.index
    );
    println!(
        "  current capacity: {:.2} CKB",
        match_info.ckb_capacity as f64 / ONE_CKB as f64
    );
    println!(
        "  capacity_delta: {:.2} CKB",
        capacity_delta as f64 / ONE_CKB as f64
    );

    let prepare = DefaultInstruction::new(vec![Box::new(AddSecp256k1SighashCellDep {})]);
    let decline = update_match_buyer::<RpcClient>(
        buyer_address.clone(),
        match_info.clone(),
        current_xudt,
        capacity_delta,
    );
    let balance = balance_and_sign(&buyer_address, buyer_key, 1000);

    let (tx, _) = TransactionCalculator::new(vec![prepare, decline, balance])
        .new_skeleton(&rpc)
        .await?;

    let tx_hash = tx.send_and_wait(&rpc, 0, None).await?;
    println!("Rent declined! Tx hash: {}", hex_string(tx_hash.as_bytes()));

    Ok(())
}
