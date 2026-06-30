use ckb_cinnabar::calculator::{
    re_exports::{
        ckb_jsonrpc_types::{self, Uint32},
        ckb_sdk::constants::ONE_CKB,
        ckb_types::{prelude::hex_string, H256},
        eyre, tokio,
    },
    rpc::{RpcClient, RPC},
};
use opticrum_calculator::{calculator::rent_per_block_to_annual_yield, reader::scan_matches};

#[tokio::main]
pub async fn main() -> eyre::Result<()> {
    let rpc = RpcClient::new_testnet();
    let matches = scan_matches(&rpc, None).await?;

    println!("Found {} Match cells:\n", matches.len());

    for (i, m) in matches.iter().enumerate() {
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
        println!(
            "  rent_per_block: {:.0} shannons/block",
            m.match_data.shannons_per_block
        );
        // Look up the channel cell to get its capacity for annual yield
        let channel_outpoint = &m.match_args.channel_outpoint;
        let ckb_outpoint = ckb_jsonrpc_types::OutPoint {
            tx_hash: H256::from_slice(&channel_outpoint.tx_hash).expect("valid tx_hash"),
            index: Uint32::from(channel_outpoint.index),
        };
        match rpc.get_live_cell(&ckb_outpoint, false).await {
            Ok(cell_with_status) => {
                if let Some(ref cell_info) = cell_with_status.cell {
                    let capacity: u64 = cell_info.output.capacity.into();
                    println!(
                        "  annual_yield: {:.2}%",
                        rent_per_block_to_annual_yield(
                            m.match_data.shannons_per_block,
                            capacity
                        ) * 100.0
                    );
                } else {
                    println!("  annual_yield: unknown (channel cell not live)");
                }
            }
            Err(_) => {
                println!("  annual_yield: unknown (RPC lookup failed)");
            }
        }
        println!(
            "  ckb_capacity: {:.2} CKB",
            m.ckb_capacity as f64 / ONE_CKB as f64
        );
        if let Some(ref x) = m.xudt {
            println!("  xudt_amount: {}", x.amount);
        }
        println!(
            "  last_extraction_block: {}",
            m.match_data.last_extraction_block
        );
        println!("  match_current_block: {}", m.match_current_block);
        println!("\n");
    }

    Ok(())
}
