use ckb_cinnabar::calculator::{
    re_exports::{ckb_sdk::constants::ONE_CKB, ckb_types::prelude::hex_string, eyre, tokio},
    rpc::RpcClient,
};
use opticrum_calculator::reader::scan_orders;

#[tokio::main]
pub async fn main() -> eyre::Result<()> {
    let rpc = RpcClient::new_testnet();
    let orders = scan_orders(&rpc).await?;

    println!("Found {} Order cells:\n", orders.len());

    for (i, o) in orders.iter().enumerate() {
        println!("--- Order #{} ---", i);
        println!(
            "  outpoint: {}:{}",
            hex_string(&o.order_outpoint.tx_hash),
            o.order_outpoint.index
        );
        println!(
            "  fiber_pubkey: {}",
            hex_string(o.order_args.fiber_pubkey.as_bytes())
        );
        println!(
            "  buyer_lock_hash: {}",
            hex_string(&o.order_args.buyer_lock_hash)
        );
        println!(
            "  channel_capacity: {} CKB",
            o.order_data.channel_capacity as f64 / ONE_CKB as f64
        );
        println!(
            "  rent_per_block: {:.0} shannons/block",
            o.order_data.shannons_per_block
        );
        println!(
            "  rent_capacity: {} CKB",
            o.ckb_capacity as f64 / ONE_CKB as f64
        );
        if let Some(ref x) = o.xudt {
            println!("  xudt_amount: {}", x.amount);
        }
    }

    Ok(())
}
