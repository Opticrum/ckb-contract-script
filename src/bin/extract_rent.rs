use std::str::FromStr;

use ckb_cinnabar::calculator::{
    address::Address,
    instruction::{predefined::balance_and_sign_with_ckb_cli, DefaultInstruction},
    operation::basic::AddSecp256k1SighashCellDep,
    re_exports::{ckb_sdk::constants::ONE_CKB, ckb_types::prelude::hex_string, eyre, tokio},
    rpc::{RpcClient, RPC},
    TransactionCalculator,
};
use opticrum_calculator::{calculator::extract_rent, reader::scan_matches};

#[tokio::main]
pub async fn main() -> eyre::Result<()> {
    // --- CONFIGURE THESE VALUES ---
    let seller_address = Address::from_str(
        "ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqtz32u8mgzk57zdtt6n62z4y2zyh8egkdcahyxk3",
    )
    .unwrap();
    let match_index: usize = 0; // pick which scanned match to extract from

    let rpc = RpcClient::new_testnet();
    let tip_block: u64 = rpc.get_tip_block_number().await?.into();

    // Scan for matches, pick one by index
    let matches = scan_matches(&rpc).await?;
    let match_info = matches
        .get(match_index)
        .expect("No match at that index — run scan_orders first");

    println!(
        "Extracting from match: {}:{}",
        hex_string(&match_info.match_outpoint.tx_hash),
        match_info.match_outpoint.index
    );
    println!(
        "  remaining capacity: {} CKB",
        match_info.ckb_capacity as f64 / ONE_CKB as f64
    );
    println!(
        "  rent_per_block: {}",
        match_info.match_data.shannons_per_block
    );
    println!(
        "  last_extraction_block: {}",
        match_info.match_data.last_extraction_block
    );

    if match_info.is_exhausted(tip_block) {
        println!("  Match is EXHAUSTED — destroy_match will be used");
    }

    let prepare = DefaultInstruction::new(vec![Box::new(AddSecp256k1SighashCellDep {})]);
    let extract = extract_rent::<RpcClient>(seller_address.clone(), match_info.clone(), tip_block);
    let balance = balance_and_sign_with_ckb_cli(&seller_address, 1000, None);

    let (tx, _) = TransactionCalculator::new(vec![prepare, extract, balance])
        .new_skeleton(&rpc)
        .await?;

    let tx_hash = tx.send_and_wait(&rpc, 0, None).await?;
    println!(
        "Rent extracted! Tx hash: {}",
        hex_string(tx_hash.as_bytes())
    );

    Ok(())
}
