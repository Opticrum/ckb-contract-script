//! Custom `Operation` implementations for Opticrum transactions.
//!
//! These provide Opticrum-specific helper functions for constructing
//! Order Cell args, Match Cell args, and Match Cell data.

use ckb_cinnabar_calculator::{
    operation::{basic::AddCellDepByTypeId, Log, Operation},
    re_exports::{async_trait::async_trait, ckb_types::core::DepType, eyre},
    rpc::RPC,
    skeleton::{ScriptEx, TransactionSkeleton},
};

use crate::config::{opticrum_contract_type_id, OPTICRUM_CONTRACT_NAME};

// ---------------------------------------------------------------------------
// Opticrum lock script helper
// ---------------------------------------------------------------------------

/// Build a `ScriptEx::Reference` for the Opticrum contract lock.
///
/// The caller is responsible for providing correctly encoded args
/// (use `OrderArgs::to_bytes()` or `MatchArgs::to_bytes()`).
pub fn opticrum_lock(args: Vec<u8>) -> ScriptEx {
    ScriptEx::Reference(OPTICRUM_CONTRACT_NAME.into(), args)
}

/// Add the Opticrum contract code cell as a CellDep to the transaction skeleton.
pub struct AddOpticrumContractCelldep {}

#[async_trait(?Send)]
impl<T: RPC> Operation<T> for AddOpticrumContractCelldep {
    async fn run(
        self: Box<Self>,
        rpc: &T,
        skeleton: &mut TransactionSkeleton,
        log: &mut Log,
    ) -> eyre::Result<()> {
        Box::new(AddCellDepByTypeId {
            name: OPTICRUM_CONTRACT_NAME.to_string(),
            type_args: opticrum_contract_type_id(rpc.network()),
            dep_type: DepType::Code,
            with_data: false,
        })
        .run(rpc, skeleton, log)
        .await
    }
}
