//! ZVM execution context — input/output for a single ZVM call frame.

use serde::{Deserialize, Serialize};

/// A 256-bit value (ZVM word size, same as EVM).
pub type U256 = [u8; 32];

/// A 20-byte address.
pub type Address = [u8; 20];

/// Input to a ZVM execution.
#[derive(Clone, Debug)]
pub struct ZvmContext {
    /// Contract bytecode to execute.
    pub bytecode: Vec<u8>,
    /// Call input data (calldata).
    pub calldata: Vec<u8>,
    /// Caller address.
    pub caller: Address,
    /// Contract address being executed.
    pub contract: Address,
    /// ZBX value sent with this call (in wei).
    pub value: u128,
    /// Gas limit for this call.
    pub gas_limit: u64,
    /// Block number.
    pub block_number: u64,
    /// Block timestamp (Unix seconds).
    pub block_timestamp: u64,
    /// Base fee (wei per gas).
    pub base_fee: u128,
    /// Blob base fee (wei per byte).
    pub blob_base_fee: u128,
    /// Chain ID (8989 for ZBX mainnet, 8990 for testnet+devnet).
    pub chain_id: u64,
    /// Whether this is a static (read-only) call.
    pub is_static: bool,
    /// AA UserOperation original sender (if called via bundler).
    pub aa_sender: Option<Address>,
    /// Current ZBX/USD price (18 decimals). 0 if oracle unavailable.
    pub zbx_price_usd: u128,
    /// SEC-2026-05-09 Pass-13 (ZVM-T0-ORIGIN): EIP-3 transaction origin
    /// (the EOA that signed the outer transaction). Propagated UNCHANGED
    /// across CALL / CALLCODE / DELEGATECALL / STATICCALL sub-frames so
    /// `tx.origin` returns the same address at every depth — matching
    /// Ethereum semantics. Pre-Pass-13 the ORIGIN opcode aliased to
    /// CALLER, which let `tx.origin == msg.sender` checks (the classic
    /// "is this an EOA caller?" pattern) silently pass for *every*
    /// reentered contract → trivial auth bypass.
    pub origin: Address,
}

impl ZvmContext {
    /// Create a default context for testing.
    pub fn test_default() -> Self {
        ZvmContext {
            bytecode:       vec![],
            calldata:       vec![],
            caller:         [0u8; 20],
            contract:       [0u8; 20],
            value:          0,
            gas_limit:      1_000_000,
            block_number:   1,
            block_timestamp: 1_700_000_000,
            base_fee:       1_000_000_000,
            blob_base_fee:  1,
            chain_id:       zbx_types::CHAIN_ID_MAINNET,
            is_static:      false,
            aa_sender:      None,
            zbx_price_usd:  2_500 * 10u128.pow(18), // $2500 mock price
            origin:         [0u8; 20],
        }
    }
}

/// Execution status after ZVM run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ExecutionStatus {
    /// Execution completed successfully.
    Success,
    /// REVERT opcode executed — reverted with data.
    Revert,
    /// Out of gas.
    OutOfGas,
    /// Invalid opcode or stack underflow.
    InvalidOpcode(u8),
    /// Stack overflow (> 1024 items).
    StackOverflow,
    /// ZVM-specific error.
    ZvmError(String),
}

/// Result of a ZVM execution.
#[derive(Clone, Debug)]
pub struct ZvmResult {
    /// Final execution status.
    pub status: ExecutionStatus,
    /// Return data (from RETURN or REVERT).
    pub return_data: Vec<u8>,
    /// Gas remaining after execution.
    pub gas_remaining: u64,
    /// Gas used.
    pub gas_used: u64,
    /// Logs emitted during execution.
    pub logs: Vec<ZvmLog>,
    /// ZVM structured logs (from ZVMLOG opcode).
    pub zvm_logs: Vec<ZvmStructuredLog>,
}

/// EVM-compatible log (from LOG0–LOG4).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZvmLog {
    pub address: Address,
    pub topics:  Vec<[u8; 32]>,
    pub data:    Vec<u8>,
}

/// ZVM structured log (from ZVMLOG opcode — key-value pairs).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZvmStructuredLog {
    pub key:   String,
    pub value: String,
    pub block: u64,
    pub index: u32,
}