//! Bundle builder: groups UserOperations into a single handleOps call.

use crate::{mempool::UserOperation, error::BundlerError};
use tracing::info;

/// A bundle ready to be submitted to the EntryPoint.
#[derive(Debug)]
pub struct Bundle {
    /// The UserOperations in this bundle.
    pub ops: Vec<UserOperation>,
    /// Address that receives the bundler fee.
    pub beneficiary: String,
    /// Estimated total gas for the handleOps call.
    pub estimated_gas: u64,
}

pub struct BundleBuilder {
    beneficiary: String,
    gas_overhead_per_op: u64,
}

impl BundleBuilder {
    pub fn new(beneficiary: impl Into<String>) -> Self {
        BundleBuilder {
            beneficiary: beneficiary.into(),
            gas_overhead_per_op: 5_000,
        }
    }

    /// Build a bundle from the given UserOperations.
    /// Rejects ops that would exceed block gas limit.
    pub fn build(&self, ops: Vec<UserOperation>, block_gas_limit: u64) -> Result<Bundle, BundlerError> {
        if ops.is_empty() {
            return Err(BundlerError::EmptyBundle);
        }

        let mut selected = Vec::new();
        let mut total_gas: u64 = 21_000; // base tx cost

        for op in ops {
            let op_gas = op.total_gas().saturating_add(self.gas_overhead_per_op);
            if total_gas.saturating_add(op_gas) > block_gas_limit {
                break; // gas limit reached, don't include this op
            }
            total_gas = total_gas.saturating_add(op_gas);
            selected.push(op);
        }

        info!(ops = selected.len(), total_gas, "bundle built");
        Ok(Bundle {
            ops: selected,
            beneficiary: self.beneficiary.clone(),
            estimated_gas: total_gas,
        })
    }

    /// Encode the handleOps(ops[], beneficiary) calldata.
    pub fn encode_handle_ops(&self, bundle: &Bundle) -> Vec<u8> {
        // Function selector: handleOps(UserOperation[],address) = 0x1fad948c
        let selector = [0x1f, 0xad, 0x94, 0x8c];
        // In production: ABI-encode the ops array and beneficiary address.
        // Placeholder: returns selector + op count for testing.
        let mut calldata = selector.to_vec();
        calldata.push(bundle.ops.len() as u8);
        calldata
    }
}