//! ZVM opcode definitions — EVM (0x00–0xFF address space) + ZBX-native (0xC0–0xC9).
//!
//! S7-ZVM-DOC2 (audit 2026-05-01): prior versions of this header claimed
//! "EVM (0x00–0xEF) + ZBX-native (0xF0–0xF9)" — both halves were wrong.
//! EVM uses the full 0x00–0xFF byte range (system-call ops 0xF0=CREATE
//! through 0xFF=SELFDESTRUCT live in the upper range), and ZBX-native
//! opcodes have always lived at 0xC0–0xC9 in the actual enum below
//! (chosen to avoid colliding with EVM system opcodes). S7-ZVM-DOC1 fixed
//! `lib.rs` and an inline comment at line 117 in Session 8 but missed
//! this file-header doc-block. Pure comment fix.

use std::fmt;

/// ZVM opcode enum: ~150 EVM opcodes from the standard 0x00–0xFF address
/// space, plus 10 ZBX-native opcodes at 0xC0–0xC9.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Opcode {
    // ── Stop & Arithmetic ────────────────────────────────────────────────
    STOP       = 0x00,
    ADD        = 0x01,
    MUL        = 0x02,
    SUB        = 0x03,
    DIV        = 0x04,
    SDIV       = 0x05,
    MOD        = 0x06,
    SMOD       = 0x07,
    ADDMOD     = 0x08,
    MULMOD     = 0x09,
    EXP        = 0x0A,
    SIGNEXTEND = 0x0B,

    // ── Comparison & Bitwise ─────────────────────────────────────────────
    LT     = 0x10,
    GT     = 0x11,
    SLT    = 0x12,
    SGT    = 0x13,
    EQ     = 0x14,
    ISZERO = 0x15,
    AND    = 0x16,
    OR     = 0x17,
    XOR    = 0x18,
    NOT    = 0x19,
    BYTE   = 0x1A,
    SHL    = 0x1B,
    SHR    = 0x1C,
    SAR    = 0x1D,

    // ── SHA3 ─────────────────────────────────────────────────────────────
    KECCAK256 = 0x20,

    // ── Environmental ────────────────────────────────────────────────────
    ADDRESS      = 0x30,
    BALANCE      = 0x31,
    ORIGIN       = 0x32,
    CALLER       = 0x33,
    CALLVALUE    = 0x34,
    CALLDATALOAD = 0x35,
    CALLDATASIZE = 0x36,
    CALLDATACOPY = 0x37,
    CODESIZE     = 0x38,
    CODECOPY     = 0x39,
    GASPRICE     = 0x3A,
    EXTCODESIZE  = 0x3B,
    EXTCODECOPY  = 0x3C,
    RETURNDATASIZE = 0x3D,
    RETURNDATACOPY = 0x3E,
    EXTCODEHASH  = 0x3F,

    // ── Block ────────────────────────────────────────────────────────────
    BLOCKHASH  = 0x40,
    COINBASE   = 0x41,
    TIMESTAMP  = 0x42,
    NUMBER     = 0x43,
    PREVRANDAO = 0x44,
    GASLIMIT   = 0x45,
    CHAINID    = 0x46,
    SELFBALANCE = 0x47,
    BASEFEE    = 0x48,
    BLOBHASH   = 0x49,
    BLOBBASEFEE = 0x4A,

    // ── Stack ────────────────────────────────────────────────────────────
    POP  = 0x50,
    MLOAD = 0x51,
    MSTORE = 0x52,
    MSTORE8 = 0x53,
    SLOAD  = 0x54,
    SSTORE = 0x55,
    JUMP   = 0x56,
    JUMPI  = 0x57,
    PC     = 0x58,
    MSIZE  = 0x59,
    GAS    = 0x5A,
    JUMPDEST = 0x5B,
    TLOAD  = 0x5C,
    TSTORE = 0x5D,
    MCOPY  = 0x5E,
    PUSH0  = 0x5F,

    // ── PUSH1–PUSH32 ─────────────────────────────────────────────────────
    PUSH1  = 0x60, PUSH2  = 0x61, PUSH3  = 0x62, PUSH4  = 0x63,
    PUSH5  = 0x64, PUSH6  = 0x65, PUSH7  = 0x66, PUSH8  = 0x67,
    PUSH9  = 0x68, PUSH10 = 0x69, PUSH11 = 0x6A, PUSH12 = 0x6B,
    PUSH13 = 0x6C, PUSH14 = 0x6D, PUSH15 = 0x6E, PUSH16 = 0x6F,
    PUSH17 = 0x70, PUSH18 = 0x71, PUSH19 = 0x72, PUSH20 = 0x73,
    PUSH21 = 0x74, PUSH22 = 0x75, PUSH23 = 0x76, PUSH24 = 0x77,
    PUSH25 = 0x78, PUSH26 = 0x79, PUSH27 = 0x7A, PUSH28 = 0x7B,
    PUSH29 = 0x7C, PUSH30 = 0x7D, PUSH31 = 0x7E, PUSH32 = 0x7F,

    // ── DUP1–DUP16 ───────────────────────────────────────────────────────
    DUP1  = 0x80, DUP2  = 0x81, DUP3  = 0x82, DUP4  = 0x83,
    DUP5  = 0x84, DUP6  = 0x85, DUP7  = 0x86, DUP8  = 0x87,
    DUP9  = 0x88, DUP10 = 0x89, DUP11 = 0x8A, DUP12 = 0x8B,
    DUP13 = 0x8C, DUP14 = 0x8D, DUP15 = 0x8E, DUP16 = 0x8F,

    // ── SWAP1–SWAP16 ─────────────────────────────────────────────────────
    SWAP1  = 0x90, SWAP2  = 0x91, SWAP3  = 0x92, SWAP4  = 0x93,
    SWAP5  = 0x94, SWAP6  = 0x95, SWAP7  = 0x96, SWAP8  = 0x97,
    SWAP9  = 0x98, SWAP10 = 0x99, SWAP11 = 0x9A, SWAP12 = 0x9B,
    SWAP13 = 0x9C, SWAP14 = 0x9D, SWAP15 = 0x9E, SWAP16 = 0x9F,

    // ── LOG ──────────────────────────────────────────────────────────────
    LOG0 = 0xA0, LOG1 = 0xA1, LOG2 = 0xA2, LOG3 = 0xA3, LOG4 = 0xA4,

    // ── System ───────────────────────────────────────────────────────────
    // Audit-2026-05-01 S7-ZVM-DOC1: prior comments here suggested 0xF0/0xF1
    // had been reassigned to PAYID/ZUSDBAL ("CREATE moved to ZVM_CREATE").
    // That remap was an abandoned design draft — these slots are still
    // standard EVM CREATE/CALL/CALLCODE/RETURN/DELEGATECALL/CREATE2 and
    // ZBX-native opcodes live at 0xC0–0xC9 below. Do not reintroduce the
    // remap: existing Solidity contracts depend on these standard hex codes.
    CREATE       = 0xF0,
    CALL         = 0xF1,
    CALLCODE     = 0xF2,
    RETURN       = 0xF3,
    DELEGATECALL = 0xF4,
    CREATE2      = 0xF5,
    STATICCALL   = 0xFA,
    REVERT       = 0xFD,
    INVALID      = 0xFE,
    SELFDESTRUCT = 0xFF,

    // ── ZVM Native Opcodes (0xC0–0xC9) ───────────────────────────────────
    // Note: placed at 0xC0 range to avoid collision with EVM system opcodes
    PAYID    = 0xC0, // Stack: [payid_ptr, payid_len] → [addr (20 bytes, zero-padded)]
    ZUSDBAL  = 0xC1, // Stack: [addr] → [balance_u256]
    ZBXPRICE = 0xC2, // Stack: [] → [price_usd_18dec]  e.g. 2_500_000_000_000_000_000_000 = $2500
    ZBXTIME  = 0xC3, // Stack: [] → [5000]  (ZBX block time in ms, always 5000)
    AASENDER = 0xC4, // Stack: [] → [original_sender_addr]  (ERC-4337 UserOp sender)
    CHAINVER = 0xC5, // Stack: [] → [1]  (ZVM version)
    BLOBFEE  = 0xC6, // Stack: [] → [blob_base_fee_wei]
    PAYIDSET = 0xC7, // Stack: [addr] → [1 if has Pay ID, 0 otherwise]
    ZBXBURN  = 0xC8, // Stack: [amount] → []  (burns ZBX from caller, reduces supply)
    ZVMLOG   = 0xC9, // Stack: [key_ptr, key_len, val_ptr, val_len] → [] (structured log)
}

/// ZVM-only opcodes for easy iteration.
pub const ZVM_OPCODES: &[Opcode] = &[
    Opcode::PAYID,
    Opcode::ZUSDBAL,
    Opcode::ZBXPRICE,
    Opcode::ZBXTIME,
    Opcode::AASENDER,
    Opcode::CHAINVER,
    Opcode::BLOBFEE,
    Opcode::PAYIDSET,
    Opcode::ZBXBURN,
    Opcode::ZVMLOG,
];

impl Opcode {
    /// Try to parse a byte into an Opcode.
    pub fn from_u8(byte: u8) -> Option<Self> {
        // Fast path: most opcodes map directly.
        // Full match table for safety.
        match byte {
            0x00 => Some(Self::STOP),
            0x01 => Some(Self::ADD),
            0x02 => Some(Self::MUL),
            0x03 => Some(Self::SUB),
            0x04 => Some(Self::DIV),
            0x05 => Some(Self::SDIV),
            0x06 => Some(Self::MOD),
            0x07 => Some(Self::SMOD),
            0x08 => Some(Self::ADDMOD),
            0x09 => Some(Self::MULMOD),
            0x0A => Some(Self::EXP),
            0x0B => Some(Self::SIGNEXTEND),
            0x10 => Some(Self::LT),
            0x11 => Some(Self::GT),
            0x12 => Some(Self::SLT),
            0x13 => Some(Self::SGT),
            0x14 => Some(Self::EQ),
            0x15 => Some(Self::ISZERO),
            0x16 => Some(Self::AND),
            0x17 => Some(Self::OR),
            0x18 => Some(Self::XOR),
            0x19 => Some(Self::NOT),
            0x1A => Some(Self::BYTE),
            0x1B => Some(Self::SHL),
            0x1C => Some(Self::SHR),
            0x1D => Some(Self::SAR),
            0x20 => Some(Self::KECCAK256),
            0x30 => Some(Self::ADDRESS),
            0x31 => Some(Self::BALANCE),
            0x32 => Some(Self::ORIGIN),
            0x33 => Some(Self::CALLER),
            0x34 => Some(Self::CALLVALUE),
            0x35 => Some(Self::CALLDATALOAD),
            0x36 => Some(Self::CALLDATASIZE),
            0x37 => Some(Self::CALLDATACOPY),
            0x38 => Some(Self::CODESIZE),
            0x39 => Some(Self::CODECOPY),
            0x3A => Some(Self::GASPRICE),
            0x3B => Some(Self::EXTCODESIZE),
            0x3C => Some(Self::EXTCODECOPY),
            0x3D => Some(Self::RETURNDATASIZE),
            0x3E => Some(Self::RETURNDATACOPY),
            0x3F => Some(Self::EXTCODEHASH),
            0x40 => Some(Self::BLOCKHASH),
            0x41 => Some(Self::COINBASE),
            0x42 => Some(Self::TIMESTAMP),
            0x43 => Some(Self::NUMBER),
            0x44 => Some(Self::PREVRANDAO),
            0x45 => Some(Self::GASLIMIT),
            0x46 => Some(Self::CHAINID),
            0x47 => Some(Self::SELFBALANCE),
            0x48 => Some(Self::BASEFEE),
            0x49 => Some(Self::BLOBHASH),
            0x4A => Some(Self::BLOBBASEFEE),
            0x50 => Some(Self::POP),
            0x51 => Some(Self::MLOAD),
            0x52 => Some(Self::MSTORE),
            0x53 => Some(Self::MSTORE8),
            0x54 => Some(Self::SLOAD),
            0x55 => Some(Self::SSTORE),
            0x56 => Some(Self::JUMP),
            0x57 => Some(Self::JUMPI),
            0x58 => Some(Self::PC),
            0x59 => Some(Self::MSIZE),
            0x5A => Some(Self::GAS),
            0x5B => Some(Self::JUMPDEST),
            0x5C => Some(Self::TLOAD),
            0x5D => Some(Self::TSTORE),
            0x5E => Some(Self::MCOPY),
            0x5F => Some(Self::PUSH0),
            // SAFETY: ZvmOpCode is repr(u8) and every byte in the matched
            // ranges corresponds to an explicit enum variant. The enclosing
            // match arm guarantees the value is in-range, so transmute is
            // sound.
            0x60..=0x7F => Some(unsafe { std::mem::transmute::<u8, Self>(byte) }),
            0x80..=0x8F => Some(unsafe { std::mem::transmute::<u8, Self>(byte) }),
            0x90..=0x9F => Some(unsafe { std::mem::transmute::<u8, Self>(byte) }),
            0xA0..=0xA4 => Some(unsafe { std::mem::transmute::<u8, Self>(byte) }),
            0xF0 => Some(Self::CREATE),
            0xF1 => Some(Self::CALL),
            0xF2 => Some(Self::CALLCODE),
            0xF3 => Some(Self::RETURN),
            0xF4 => Some(Self::DELEGATECALL),
            0xF5 => Some(Self::CREATE2),
            0xFA => Some(Self::STATICCALL),
            0xFD => Some(Self::REVERT),
            0xFE => Some(Self::INVALID),
            0xFF => Some(Self::SELFDESTRUCT),
            // ZVM native
            0xC0 => Some(Self::PAYID),
            0xC1 => Some(Self::ZUSDBAL),
            0xC2 => Some(Self::ZBXPRICE),
            0xC3 => Some(Self::ZBXTIME),
            0xC4 => Some(Self::AASENDER),
            0xC5 => Some(Self::CHAINVER),
            0xC6 => Some(Self::BLOBFEE),
            0xC7 => Some(Self::PAYIDSET),
            0xC8 => Some(Self::ZBXBURN),
            0xC9 => Some(Self::ZVMLOG),
            _ => None,
        }
    }

    /// Is this a ZVM-native opcode (not in standard EVM)?
    pub fn is_zvm_native(self) -> bool {
        (self as u8) >= 0xC0 && (self as u8) <= 0xC9
    }

    /// Human-readable opcode name.
    pub fn name(self) -> &'static str {
        match self {
            Self::STOP       => "STOP",
            Self::ADD        => "ADD",
            Self::MUL        => "MUL",
            Self::SUB        => "SUB",
            Self::DIV        => "DIV",
            Self::SDIV       => "SDIV",
            Self::MOD        => "MOD",
            Self::SMOD       => "SMOD",
            Self::ADDMOD     => "ADDMOD",
            Self::MULMOD     => "MULMOD",
            Self::EXP        => "EXP",
            Self::SIGNEXTEND => "SIGNEXTEND",
            Self::KECCAK256  => "KECCAK256",
            Self::CHAINID    => "CHAINID",
            Self::BASEFEE    => "BASEFEE",
            Self::BLOBHASH   => "BLOBHASH",
            Self::BLOBBASEFEE => "BLOBBASEFEE",
            Self::PUSH0      => "PUSH0",
            Self::RETURN     => "RETURN",
            Self::REVERT     => "REVERT",
            Self::INVALID    => "INVALID",
            Self::SELFDESTRUCT => "SELFDESTRUCT",
            // ZVM native
            Self::PAYID    => "PAYID",
            Self::ZUSDBAL  => "ZUSDBAL",
            Self::ZBXPRICE => "ZBXPRICE",
            Self::ZBXTIME  => "ZBXTIME",
            Self::AASENDER => "AASENDER",
            Self::CHAINVER => "CHAINVER",
            Self::BLOBFEE  => "BLOBFEE",
            Self::PAYIDSET => "PAYIDSET",
            Self::ZBXBURN  => "ZBXBURN",
            Self::ZVMLOG   => "ZVMLOG",
            _ => "UNKNOWN",
        }
    }
}

impl fmt::Display for Opcode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}(0x{:02X})", self.name(), *self as u8)
    }
}