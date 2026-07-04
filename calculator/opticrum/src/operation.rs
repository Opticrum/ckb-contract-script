//! Custom `Operation` implementations for Opticrum transactions.
//!
//! These provide Opticrum-specific helper functions for constructing
//! Order Cell args, Match Cell args, and Match Cell data.

use ckb_cinnabar_calculator::{
    operation::{basic::AddCellDepByTypeId, Log, Operation},
    re_exports::{async_trait::async_trait, ckb_types::core::DepType, eyre},
    rpc::RPC,
    skeleton::{ScriptEx, TransactionSkeleton, WitnessEx},
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

// ---------------------------------------------------------------------------
// Output-type witness helper — for off-chain metadata
// ---------------------------------------------------------------------------

/// Sets a witness entry at `witnesses[input_count + output_index]` with the
/// given `output_type` bytes.
///
/// Used to attach the buyer's Fiber node address (multiaddr) to the order
/// creation transaction so sellers can discover it when scanning orders.
///
/// Must run **after** all inputs are added (i.e., after balance) because the
/// witness index depends on the final input count.
pub struct SetOutputWitness {
    pub output_index: usize,
    pub data: Vec<u8>,
}

#[async_trait(?Send)]
impl<T: RPC> Operation<T> for SetOutputWitness {
    async fn run(
        self: Box<Self>,
        _rpc: &T,
        skeleton: &mut TransactionSkeleton,
        _log: &mut Log,
    ) -> eyre::Result<()> {
        let witness_index = skeleton.inputs.len() + self.output_index;

        // Pad with default witnesses if the vec isn't long enough yet
        while skeleton.witnesses.len() <= witness_index {
            skeleton.witnesses.push(WitnessEx::default());
        }

        let witness = &mut skeleton.witnesses[witness_index];
        witness.output_type = self.data;
        witness.empty = false;
        Ok(())
    }
}
