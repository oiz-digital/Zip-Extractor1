//! EVM transaction executor.

use crate::EvmError;
use zbx_types::CHAIN_ID_MAINNET;
use tracing::{info, debug};

/// Result of executing a transaction.
#[derive(Debug)]
pub struct ExecResult {
    pub success:      bool,
    pub gas_used:     u64,
    pub return_data:  Vec<u8>,
    pub logs:         Vec<EvmLog>,
    pub error:        Option<String>,
}

/// An EVM log (event).
#[derive(Debug, Clone)]
pub struct EvmLog {
    pub address: [u8; 20],
    pub topics:  Vec<[u8; 32]>,
    pub data:    Vec<u8>,
}

/// Transaction input for the EVM executor.
#[derive(Debug, Clone)]
pub struct TxInput {
    pub from:          [u8; 20],
    pub to:            Option<[u8; 20]>,
    pub value:         u128,
    pub data:          Vec<u8>,
    pub gas_limit:     u64,
    pub max_fee:       u128,
    pub max_priority:  u128,
    pub nonce:         u64,
}

/// EVM executor — executes transactions against the state.
pub struct EvmExecutor {
    pub chain_id: u64,
}

impl EvmExecutor {
    pub fn new() -> Self {
        Self { chain_id: CHAIN_ID_MAINNET }
    }

    /// Execute a transaction.
    pub fn execute(&self, tx: &TxInput) -> Result<ExecResult, EvmError> {
        debug!(
            from = hex::encode(tx.from),
            to   = tx.to.map(hex::encode).unwrap_or_default(),
            gas  = tx.gas_limit,
            "Executing EVM transaction"
        );

        // Production: delegate to revm::EVM with ZBX state backend.
        // Here we return a stub result for the source browser.
        Ok(ExecResult {
            success:     true,
            gas_used:    21_000,
            return_data: vec![],
            logs:        vec![],
            error:       None,
        })
    }

    /// Execute a read-only eth_call (no state change).
    pub fn call(&self, tx: &TxInput) -> Result<Vec<u8>, EvmError> {
        debug!("eth_call (read-only)");
        Ok(vec![])
    }
}

impl Default for EvmExecutor {
    fn default() -> Self { Self::new() }
}