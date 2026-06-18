//! Configuration — loads Opticrum contract deployment records.
//!
//! Provides the contract name and type_id used by ScriptEx::Reference
//! to resolve code hashes across mainnet/testnet/simulator.

use ckb_cinnabar_calculator::{
    re_exports::ckb_types::{h256, H256},
    rpc::Network,
    simulation::random_hash,
};

pub const OPTICRUM_CONTRACT_NAME: &str = "opticrum";
pub const ABOUT_ONE_DAY_BLOCKS: u64 = 10_000;
pub const CKB_DECIMAL: u64 = 100_000_000;

/// The canonical type_id used in ScriptEx::Reference lookups.
pub fn opticrum_contract_type_id(network: Network) -> H256 {
    match network {
        Network::Mainnet | Network::Testnet | Network::Fake => {
            h256!("0x0000000000000000000000000000000000000000000000000000000000000000")
        }
        _ => random_hash().into(),
    }
}
