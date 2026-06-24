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
        Network::Testnet => {
            h256!("0x3b009c195d6dd5617d687a0387bccefae99eac8a1a5e393bf2563a3afb7feb49")
        }
        Network::Fake => H256::default(),
        Network::Mainnet => {
            unimplemented!("Mainnet type_id not implemented")
        }
        _ => random_hash().into(),
    }
}
