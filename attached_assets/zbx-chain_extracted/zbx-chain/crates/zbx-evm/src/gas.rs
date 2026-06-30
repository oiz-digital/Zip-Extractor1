//! EVM gas accounting and opcode costs (Berlin + London rules).

/// Static gas costs for common opcodes.
pub const GAS_ZERO: u64 = 0;
pub const GAS_BASE: u64 = 2;
pub const GAS_VERY_LOW: u64 = 3;
pub const GAS_LOW: u64 = 5;
pub const GAS_MID: u64 = 8;
pub const GAS_HIGH: u64 = 10;
pub const GAS_JUMPDEST: u64 = 1;
pub const GAS_SLOAD_WARM: u64 = 100;
pub const GAS_SLOAD_COLD: u64 = 2100;
pub const GAS_SSTORE_SET: u64 = 20_000;
pub const GAS_SSTORE_RESET: u64 = 2_900;
pub const GAS_SSTORE_CLEARS: u64 = 4_800;
pub const GAS_CALL: u64 = 100;
pub const GAS_CALL_VALUE: u64 = 9_000;
pub const GAS_CALL_NEW_ACCOUNT: u64 = 25_000;
pub const GAS_CREATE: u64 = 32_000;
pub const GAS_SHA3: u64 = 30;
pub const GAS_SHA3_WORD: u64 = 6;
pub const GAS_COPY_WORD: u64 = 3;
pub const GAS_LOG_BASE: u64 = 375;
pub const GAS_LOG_DATA: u64 = 8;
pub const GAS_LOG_TOPIC: u64 = 375;
pub const GAS_KECCAK256: u64 = 30;
pub const GAS_KECCAK256_WORD: u64 = 6;

/// Memory expansion cost: cost(new_words) - cost(old_words).
/// cost(n) = 3*n + n^2/512
///
/// Implemented in u128 internally — `new_words^2` overflows a u64 once
/// `new_words` exceeds ~4.3e9 (= 2^32). Even though `MAX_MEMORY` currently
/// caps `new_words` at 1_048_576, the contract between `Memory::ensure` and
/// this function would silently produce wrong gas if `MAX_MEMORY` were ever
/// raised — which is exactly the kind of foot-gun consensus depends on us
/// not having. We saturate to u64::MAX (which simply means "out of gas") on
/// the way out. See AUDIT_2026-04-30.md H-12.
pub fn memory_expansion_cost(old_words: u64, new_words: u64) -> u64 {
    let cost = |n: u64| -> u128 {
        let n = n as u128;
        3u128.saturating_mul(n).saturating_add(n.saturating_mul(n) / 512)
    };
    let diff = cost(new_words).saturating_sub(cost(old_words));
    if diff > u64::MAX as u128 { u64::MAX } else { diff as u64 }
}

/// Dynamic gas for CALLDATACOPY / CODECOPY / RETURNDATACOPY.
pub fn copy_cost(size_bytes: u64) -> u64 {
    let words = (size_bytes + 31) / 32;
    GAS_COPY_WORD * words
}

/// Dynamic gas for SHA3.
pub fn sha3_cost(size_bytes: u64) -> u64 {
    let words = (size_bytes + 31) / 32;
    GAS_SHA3 + GAS_SHA3_WORD * words
}

/// Dynamic gas for LOG0-LOG4.
pub fn log_cost(data_size: u64, topic_count: u64) -> u64 {
    GAS_LOG_BASE + GAS_LOG_DATA * data_size + GAS_LOG_TOPIC * topic_count
}

/// EIP-3860: initcode cost (2 gas per 32-byte word of initcode).
pub fn initcode_cost(size: usize) -> u64 {
    let words = (size as u64 + 31) / 32;
    2 * words
}

// ---------------------------------------------------------------------------
//  S32 — CALL / CREATE / SELFDESTRUCT gas constants (EIP-150 / 2929 / 3529 /
//  6780). Only the subset actually consumed by the interpreter today; more
//  granular EIP-2200 SSTORE refunds are out of scope for this sprint.
// ---------------------------------------------------------------------------

/// EIP-2929: cold address access surcharge (added on top of GAS_CALL warm).
pub const GAS_COLD_ACCOUNT_ACCESS: u64 = 2_500;
/// EIP-150: positive-value transfer surcharge inside CALL/CALLCODE.
pub const GAS_CALL_VALUE_TRANSFER: u64 = 9_000;
/// EIP-150: extra surcharge when the value transfer creates a new account.
pub const GAS_NEW_ACCOUNT: u64 = 25_000;
/// EIP-150: minimum gas forwarded to the callee when value > 0.
pub const GAS_CALL_STIPEND: u64 = 2_300;
/// EIP-150: maximum nesting depth for CALL/CREATE.
pub const CALL_DEPTH_LIMIT: usize = 1024;
/// CREATE base cost (Yellow Paper, unchanged).
pub const GAS_CREATE_BASE: u64 = 32_000;
/// EIP-3860: per-word initcode hashing surcharge (2 gas per 32-byte word).
pub const GAS_INITCODE_WORD: u64 = 2;
/// Yellow Paper §6.1: per-byte cost of writing deployed runtime code.
pub const GAS_CODE_DEPOSIT_PER_BYTE: u64 = 200;
/// EIP-150: SELFDESTRUCT static cost (modern EVM treats refund as 0 per
/// EIP-3529 — ZBX inherits that).
pub const GAS_SELFDESTRUCT: u64 = 5_000;
/// EIP-3860: hard cap on initcode bytes per CREATE / CREATE2.
pub const MAX_INITCODE_SIZE: usize = 49_152;
/// EIP-170: hard cap on deployed runtime code size.
pub const MAX_CONTRACT_CODE_SIZE: usize = 24_576;

/// EIP-150 "all-but-one-64th" rule: parent frame retains at least
/// `gas_remaining / 64` after forwarding the rest. The callee's effective
/// gas budget is `min(requested, gas_remaining - gas_remaining/64)`.
pub fn forward_gas_eip150(gas_remaining: u64, requested: u64) -> u64 {
    let max_forwardable = gas_remaining.saturating_sub(gas_remaining / 64);
    requested.min(max_forwardable)
}