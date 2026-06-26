use std::str::FromStr;

use ckb_cinnabar::calculator::{
    address::Address,
    instruction::{predefined::balance_and_sign, DefaultInstruction},
    operation::basic::AddSecp256k1SighashCellDep,
    re_exports::{
        ckb_sdk::constants::ONE_CKB, ckb_types::prelude::hex_string, eyre, secp256k1::SecretKey,
        tokio,
    },
    rpc::RpcClient,
    TransactionCalculator,
};
use opticrum_calculator::{
    calculator::{confirm_match, discard_match},
    reader::scan_matches,
    types::MatchStatus,
};

/// Buyer-facing tool for reviewing Frozen matches.
///
/// Usage: review_match <action> [match_index]
///   action: "confirm" or "discard"
///   match_index: which Frozen match to act on (default: 0)
#[tokio::main]
pub async fn main() -> eyre::Result<()> {
    let _ = dotenvy::dotenv();

    let action: String = std::env::args().nth(1).unwrap_or_else(|| "list".into());
    let match_index: usize = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "0".into())
        .parse()
        .unwrap_or(0);

    let buyer_address = Address::from_str(
        "ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqv5puz2ee96nuh9nmc6rtm0n8v7agju4rgdmxlnk",
    )
    .unwrap();

    let rpc = RpcClient::new_testnet();
    let all_matches = scan_matches(&rpc).await?;

    // Filter to Frozen matches
    let frozen: Vec<_> = all_matches
        .iter()
        .enumerate()
        .filter(|(_, m)| m.match_data.status == MatchStatus::Frozen)
        .collect();

    if frozen.is_empty() {
        println!("No Frozen matches found.");
        return Ok(());
    }

    println!("Found {} Frozen match(es):\n", frozen.len());
    for (i, (_global_idx, m)) in frozen.iter().enumerate() {
        let status_str = format!("{:?}", m.match_data.status);
        println!(
            "  [{}] outpoint: {}:{}",
            i,
            hex_string(&m.match_outpoint.tx_hash),
            m.match_outpoint.index
        );
        println!("      status: {}", status_str);
        println!(
            "      channel_outpoint: {}:{:08x}",
            hex_string(&m.match_args.channel_outpoint.tx_hash),
            m.match_args.channel_outpoint.index
        );
        println!(
            "      ckb_capacity: {} CKB",
            m.ckb_capacity as f64 / ONE_CKB as f64
        );
        if let Some(ref x) = m.xudt {
            println!("      xudt_amount: {}", x.amount);
        }
        println!("      escrow_blocks: {}", m.match_data.escrow_blocks);
        println!();
    }

    if action == "list" {
        println!("Usage: review_match <confirm|discard> [match_index]");
        return Ok(());
    }

    let (_, match_info) = frozen
        .get(match_index)
        .ok_or_else(|| eyre::eyre!("Match index {} out of range", match_index))?;

    let instruction = match action.as_str() {
        "confirm" => {
            println!(
                "Confirming match {} (outpoint: {}:{})",
                match_index,
                hex_string(&match_info.match_outpoint.tx_hash),
                match_info.match_outpoint.index
            );
            confirm_match::<RpcClient>(buyer_address.clone(), (*match_info).clone())
        }
        "discard" => {
            println!(
                "Discarding match {} (outpoint: {}:{})",
                match_index,
                hex_string(&match_info.match_outpoint.tx_hash),
                match_info.match_outpoint.index
            );
            discard_match::<RpcClient>(buyer_address.clone(), (*match_info).clone())
        }
        _ => eyre::bail!("Unknown action: {}. Use 'confirm' or 'discard'", action),
    };

    let buyer_pk_hex = std::env::var("BUYER_PRIVATE_KEY").map_err(|_| {
        eyre::eyre!(
            "BUYER_PRIVATE_KEY not set.\n\
             Create a .env file or export the environment variable:\n\
             BUYER_PRIVATE_KEY=<64-char hex private key>"
        )
    })?;
    let buyer_key = SecretKey::from_slice(&hex::decode(&buyer_pk_hex)?).unwrap();
    let balance = balance_and_sign(&buyer_address, buyer_key, 1000);

    let prepare = DefaultInstruction::new(vec![Box::new(AddSecp256k1SighashCellDep {})]);
    let (tx, _) = TransactionCalculator::new(vec![prepare, instruction, balance])
        .new_skeleton(&rpc)
        .await?;

    let tx_hash = tx.send_and_wait(&rpc, 0, None).await?;
    println!(
        "Match {}ed! Tx hash: {}",
        action,
        hex_string(tx_hash.as_bytes())
    );

    Ok(())
}
