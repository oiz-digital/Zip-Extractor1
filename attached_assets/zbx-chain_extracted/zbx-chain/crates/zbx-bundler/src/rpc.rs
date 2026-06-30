//! ERC-4337 bundler JSON-RPC API.
//!
//! Implements the standardised bundler RPC spec:
//! - `eth_sendUserOperation`
//! - `eth_estimateUserOperationGas`
//! - `eth_getUserOperationByHash`
//! - `eth_getUserOperationReceipt`
//! - `eth_supportedEntryPoints`

use crate::{mempool::{BundlerMempool, UserOperation}, simulation::UserOpSimulator, error::BundlerError};
use std::sync::Arc;
use tracing::{debug, info};

pub struct BundlerRpc {
    mempool: Arc<BundlerMempool>,
    simulator: Arc<UserOpSimulator>,
}

impl BundlerRpc {
    /// Construct a new bundler RPC handler.
    ///
    /// `chain_id` is intentionally NOT a parameter here: the bundler's chain
    /// binding lives on `BundlerMempool` (set at its construction). Storing it
    /// twice would re-introduce the S13-CHAIN-ID-DRIFT class of bug. If a
    /// caller needs the chain id, ask the mempool: `mempool.chain_id()`.
    pub fn new(mempool: Arc<BundlerMempool>, simulator: Arc<UserOpSimulator>) -> Self {
        BundlerRpc { mempool, simulator }
    }

    /// Chain id this RPC instance bundles for. Delegates to the mempool, which
    /// is the single source of truth.
    pub fn chain_id(&self) -> u64 {
        self.mempool.chain_id()
    }

    /// eth_sendUserOperation: validate + add to bundler mempool.
    pub async fn send_user_operation(
        &self,
        op: UserOperation,
        entry_point: &str,
    ) -> Result<String, BundlerError> {
        if entry_point != crate::ENTRY_POINT_ADDRESS {
            return Err(BundlerError::UnsupportedEntryPoint(entry_point.to_string()));
        }
        debug!(sender = %op.sender, "eth_sendUserOperation");

        // SEC-2026-05-09 Pass-15 architect-review (HIGH-R05 wiring):
        // enforce the (validAfter, validUntil) window at admission.
        // Pre-fix the helper existed but had no callers, so expired
        // UserOps still entered the mempool.
        let now_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        crate::validation::validate_user_op_time(&op, now_unix)?;

        // Simulate before accepting
        let result = self.simulator.simulate(&op).await?;
        if !result.valid {
            return Err(BundlerError::SimulationFailed("validation reverted".into()));
        }

        let hash = self.mempool.add(op)?;
        let hash_str = format!("0x{}", hex::encode(hash));
        info!(hash = %hash_str, "UserOperation accepted");
        Ok(hash_str)
    }

    /// eth_supportedEntryPoints: return supported entry point addresses.
    pub fn supported_entry_points(&self) -> Vec<String> {
        vec![crate::ENTRY_POINT_ADDRESS.to_string()]
    }

    /// eth_estimateUserOperationGas: estimate gas for a UserOperation.
    pub async fn estimate_user_operation_gas(
        &self,
        op: &UserOperation,
    ) -> Result<serde_json::Value, BundlerError> {
        let sim = self.simulator.simulate(op).await?;
        Ok(serde_json::json!({
            "preVerificationGas": format!("0x{:x}", sim.pre_op_gas),
            "verificationGasLimit": format!("0x{:x}", op.verification_gas_limit),
            "callGasLimit": format!("0x{:x}", op.call_gas_limit),
        }))
    }
}