use ckb_cinnabar::calculator::{
    re_exports::{ckb_sdk::constants::ONE_CKB, ckb_types::prelude::hex_string, eyre, tokio},
    rpc::RpcClient,
};
use opticrum_calculator::reader::scan_matches;

#[tokio::main]
pub async fn main() -> eyre::Result<()> {
    let rpc = RpcClient::new_testnet();
    let matches = scan_matches(&rpc).await?;

    println!("Found {} Match cells:\n", matches.len());

    for (i, m) in matches.iter().enumerate() {
        let status_str = format!("{:?}", m.match_data.status);
        println!("--- Match #{} ---", i);
        println!(
            "  outpoint: {}:{}",
            hex_string(&m.match_outpoint.tx_hash),
            m.match_outpoint.index
        );
        println!(
            "  channel_outpoint: {}:{:08x}",
            hex_string(&m.match_args.channel_outpoint.tx_hash),
            m.match_args.channel_outpoint.index
        );
        println!(
            "  buyer_lock_hash: {}",
            hex_string(&m.match_args.order_args.buyer_lock_hash)
        );
        println!(
            "  buyer_fiber_pubkey: {}",
            hex_string(m.match_args.order_args.fiber_pubkey.as_bytes())
        );
        println!(
            "  seller_lock_hash: {}",
            hex_string(&m.match_args.seller_lock_hash)
        );
        println!("  status: {}", status_str);
        println!(
            "  ckb_capacity: {} CKB",
            m.ckb_capacity as f64 / ONE_CKB as f64
        );
        if let Some(ref x) = m.xudt {
            println!("  xudt_amount: {}", x.amount);
        }
        println!("  escrow_blocks: {}", m.match_data.escrow_blocks);
        println!(
            "  last_extraction_block: {}",
            m.match_data.last_extraction_block
        );
        println!("  match_current_block: {}", m.match_current_block);
        println!();
    }

    Ok(())
}
