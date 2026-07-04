//! Configuration — loads Opticrum contract deployment records.
//!
//! Provides the contract name and type_id used by ScriptEx::Reference
//! to resolve code hashes across mainnet/testnet/simulator.

use ckb_cinnabar_calculator::{
    re_exports::ckb_types::{h256, H256},
    rpc::Network,
};
use opticrum_protocol::{MATCH_ARGS_LEN, MATCH_DATA_LEN, ORDER_ARGS_LEN, ORDER_DATA_LEN};

pub const OPTICRUM_CONTRACT_NAME: &str = "opticrum";
pub const CKB_DECIMAL: u64 = 100_000_000;

/// Approximate blocks per year for CKB (~12s block interval).
/// 365.25 × 24 × 3600 / 12 ≈ 2,629,800
pub const BLOCKS_PER_YEAR: u64 = 2_629_800;

/// Extra capacity (shannons) pre-funded on Order cells above rent so Order→Match
/// with `CapacityAdjustment::Keep` succeeds without the seller injecting CKB.
///
/// Match cells have larger lock args and data; occupied grows by this many bytes.
/// CKB occupied rate: 1 byte → CKB_DECIMAL shannons.
pub const ORDER_TO_MATCH_CAPACITY_RESERVE: u64 =
    (MATCH_ARGS_LEN - ORDER_ARGS_LEN + MATCH_DATA_LEN - ORDER_DATA_LEN) as u64 * CKB_DECIMAL;

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
        Network::Custom(_) => {
            unimplemented!(
                "Custom network not implemented — server must resolve to Testnet or Mainnet"
            )
        }
    }
}
