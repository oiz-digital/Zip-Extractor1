//! ZVM gas model — EVM gas + ZVM native opcode costs.

use crate::opcodes::Opcode;

/// Gas costs for standard EVM opcodes (same as EIP-3529 / cancun).
pub fn evm_gas_cost(op: Opcode) -> u64 {
    match op {
        Opcode::STOP | Opcode::RETURN | Opcode::REVERT => 0,
        Opcode::ADD | Opcode::SUB | Opcode::LT | Opcode::GT |
        Opcode::SLT | Opcode::SGT | Opcode::EQ | Opcode::ISZERO |
        Opcode::AND | Opcode::OR | Opcode::XOR | Opcode::NOT |
        Opcode::BYTE | Opcode::SHL | Opcode::SHR | Opcode::SAR => 3,
        Opcode::MUL | Opcode::DIV | Opcode::SDIV | Opcode::MOD |
        Opcode::SMOD | Opcode::SIGNEXTEND => 5,
        Opcode::ADDMOD | Opcode::MULMOD => 8,
        Opcode::EXP => 10,          // + 50 per exponent byte
        Opcode::KECCAK256 => 30,    // + 6 per word
        Opcode::ADDRESS | Opcode::ORIGIN | Opcode::CALLER |
        Opcode::CALLVALUE | Opcode::CALLDATASIZE | Opcode::CODESIZE |
        Opcode::GASPRICE | Opcode::COINBASE | Opcode::TIMESTAMP |
        Opcode::NUMBER | Opcode::PREVRANDAO | Opcode::GASLIMIT |
        Opcode::CHAINID | Opcode::SELFBALANCE | Opcode::BASEFEE |
        Opcode::BLOBBASEFEE => 2,
        Opcode::BALANCE | Opcode::EXTCODESIZE | Opcode::EXTCODEHASH => 100,
        Opcode::BLOCKHASH => 20,
        Opcode::CALLDATALOAD => 3,
        Opcode::CALLDATACOPY | Opcode::CODECOPY | Opcode::RETURNDATACOPY => 3,
        Opcode::EXTCODECOPY => 100,
        Opcode::RETURNDATASIZE => 2,
        Opcode::POP | Opcode::PC | Opcode::MSIZE | Opcode::GAS => 2,
        Opcode::MLOAD | Opcode::MSTORE | Opcode::MSTORE8 => 3,
        Opcode::SLOAD => 100,
        Opcode::SSTORE => 100,      // EIP-3529 cold/warm accounting
        Opcode::JUMP => 8,
        Opcode::JUMPI => 10,
        Opcode::JUMPDEST => 1,
        Opcode::PUSH0 => 2,
        Opcode::TLOAD | Opcode::TSTORE => 100,
        Opcode::MCOPY => 3,
        Opcode::BLOBHASH => 3,
        _ if (op as u8) >= 0x60 && (op as u8) <= 0x7F => 3, // PUSH1-PUSH32
        _ if (op as u8) >= 0x80 && (op as u8) <= 0x8F => 3, // DUP1-DUP16
        _ if (op as u8) >= 0x90 && (op as u8) <= 0x9F => 3, // SWAP1-SWAP16
        _ if (op as u8) >= 0xA0 && (op as u8) <= 0xA4 => 375, // LOG0-LOG4
        Opcode::CREATE => 32000,
        Opcode::CREATE2 => 32000,
        // ZVM-01 FIX (HIGH): CALLCODE must pay the same 100-gas base as the
        // other CALL-family opcodes. Pre-fix it fell through to `_ => 0`,
        // making CALLCODE "free" and letting contracts loop through it at
        // essentially no cost — a DoS vector and a Yellow Paper deviation.
        Opcode::CALL | Opcode::CALLCODE | Opcode::STATICCALL | Opcode::DELEGATECALL => 100,
        Opcode::SELFDESTRUCT => 5000,
        _ => 0,
    }
}

/// Gas costs for ZVM-native opcodes.
pub fn zvm_gas_cost(op: Opcode) -> u64 {
    match op {
        // PAYID: on-chain registry lookup (similar to SLOAD)
        Opcode::PAYID    => 200,
        // ZUSDBAL: reads ZUSD contract storage
        Opcode::ZUSDBAL  => 100,
        // ZBXPRICE: reads oracle storage
        Opcode::ZBXPRICE => 50,
        // ZBXTIME: constant, cheap
        Opcode::ZBXTIME  => 2,
        // AASENDER: reads call frame data
        Opcode::AASENDER => 2,
        // CHAINVER: constant
        Opcode::CHAINVER => 2,
        // BLOBFEE: reads DA fee state
        Opcode::BLOBFEE  => 50,
        // PAYIDSET: registry existence check
        Opcode::PAYIDSET => 100,
        // ZBXBURN: write + event
        Opcode::ZBXBURN  => 500,
        // ZVMLOG: structured log (more expensive than LOG4)
        Opcode::ZVMLOG   => 600,
        _ => 0,
    }
}

/// Total gas cost for an opcode.
pub fn opcode_gas(op: Opcode) -> u64 {
    if op.is_zvm_native() {
        zvm_gas_cost(op)
    } else {
        evm_gas_cost(op)
    }
}

// ─── SEC-2026-05-09 Pass-15 — EIP-2929 / EIP-150 helpers ─────────────────────

/// EIP-2929 warm/cold storage costs.
pub const COLD_SLOAD_COST:  u64 = 2100;
pub const WARM_SLOAD_COST:  u64 = 100;
pub const COLD_ACCOUNT_COST: u64 = 2600;
pub const WARM_ACCOUNT_COST: u64 = 100;
/// EIP-2929 SSTORE base (pre-warm logic) cold delta.
pub const SSTORE_COLD_DELTA: u64 = 2100;

/// EIP-160 EXP dynamic cost: 50 gas per non-zero byte of the exponent.
/// Pre-Pass-16 EXP charged a flat 10 gas regardless of exponent size,
/// letting a contract pay 10 gas to perform a 256-bit modexp loop.
pub fn exp_dynamic_gas(exponent_be: &[u8; 32]) -> u64 {
    let leading_zeros = exponent_be.iter().take_while(|&&b| b == 0).count();
    let nonzero_bytes = 32u64.saturating_sub(leading_zeros as u64);
    50u64.saturating_mul(nonzero_bytes)
}

/// KECCAK256 / SHA3 dynamic cost: 6 gas per 32-byte word of input.
pub fn keccak256_dynamic_gas(len: usize) -> u64 {
    let words = (len as u64 + 31) / 32;
    6u64.saturating_mul(words)
}

/// CALLDATACOPY / CODECOPY / RETURNDATACOPY / EXTCODECOPY / MCOPY
/// dynamic copy cost: 3 gas per 32-byte word copied (plus mem-expansion).
pub fn copy_dynamic_gas(len: usize) -> u64 {
    let words = (len as u64 + 31) / 32;
    3u64.saturating_mul(words)
}

/// LOG0..LOG4 dynamic cost: 8 gas per byte of data + 375 per topic.
pub fn log_dynamic_gas(n_topics: u8, data_len: usize) -> u64 {
    let topic_cost = 375u64.saturating_mul(n_topics as u64);
    let data_cost  = 8u64.saturating_mul(data_len as u64);
    topic_cost.saturating_add(data_cost)
}

/// EIP-150 memory expansion gas: cost(W) = 3*W + W²/512.
/// Charges only the *delta* between old and new word counts.
/// Pre-Pass-15 the ZVM had no memory-expansion gas at all — a contract
/// could reference 1 GB of memory in a single opcode and pay nothing,
/// while the host allocated and zero-filled the entire region. Cap on
/// new_words bounds worst-case allocator pressure to ~16 MiB.
pub fn memory_gas_delta(old_words: u64, new_words: u64) -> u64 {
    if new_words <= old_words {
        return 0;
    }
    fn cost(w: u64) -> u64 {
        // 3w + w²/512, saturating so a 2^32 word reference can't overflow.
        let lin = 3u64.saturating_mul(w);
        let sq  = w.saturating_mul(w) / 512;
        lin.saturating_add(sq)
    }
    cost(new_words).saturating_sub(cost(old_words))
}