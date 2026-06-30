//! EVM opcode definitions — Cancun-era complete set.

/// EVM opcodes as per the Yellow Paper + EIPs.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(non_camel_case_types)]
pub enum OpCode {
    // ─── Stop & Arithmetic ───────────────────────────────────────────────
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
    EXP        = 0x0a,
    SIGNEXTEND = 0x0b,

    // ─── Comparison & Bitwise ────────────────────────────────────────────
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
    BYTE   = 0x1a,
    SHL    = 0x1b,
    SHR    = 0x1c,
    SAR    = 0x1d,

    // ─── SHA3 ────────────────────────────────────────────────────────────
    KECCAK256 = 0x20,

    // ─── Environmental Information ───────────────────────────────────────
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
    GASPRICE     = 0x3a,
    EXTCODESIZE  = 0x3b,
    EXTCODECOPY  = 0x3c,
    RETURNDATASIZE = 0x3d,
    RETURNDATACOPY = 0x3e,
    EXTCODEHASH  = 0x3f,

    // ─── Block Information ───────────────────────────────────────────────
    BLOCKHASH  = 0x40,
    COINBASE   = 0x41,
    TIMESTAMP  = 0x42,
    NUMBER     = 0x43,
    DIFFICULTY = 0x44, // PREVRANDAO post-Merge
    GASLIMIT   = 0x45,
    CHAINID    = 0x46,
    SELFBALANCE = 0x47,
    BASEFEE    = 0x48,
    BLOBHASH   = 0x49, // EIP-4844
    BLOBBASEFEE = 0x4a, // EIP-7516

    // ─── Stack, Memory, Storage and Flow ─────────────────────────────────
    POP      = 0x50,
    MLOAD    = 0x51,
    MSTORE   = 0x52,
    MSTORE8  = 0x53,
    SLOAD    = 0x54,
    SSTORE   = 0x55,
    JUMP     = 0x56,
    JUMPI    = 0x57,
    PC       = 0x58,
    MSIZE    = 0x59,
    GAS      = 0x5a,
    JUMPDEST = 0x5b,
    TLOAD    = 0x5c, // EIP-1153 transient
    TSTORE   = 0x5d,
    MCOPY    = 0x5e, // EIP-5656

    // ─── PUSH0 (EIP-3855) ───────────────────────────────────────────────
    PUSH0 = 0x5f,

    // ─── Push Operations ─────────────────────────────────────────────────
    PUSH1  = 0x60, PUSH2  = 0x61, PUSH3  = 0x62, PUSH4  = 0x63,
    PUSH5  = 0x64, PUSH6  = 0x65, PUSH7  = 0x66, PUSH8  = 0x67,
    PUSH9  = 0x68, PUSH10 = 0x69, PUSH11 = 0x6a, PUSH12 = 0x6b,
    PUSH13 = 0x6c, PUSH14 = 0x6d, PUSH15 = 0x6e, PUSH16 = 0x6f,
    PUSH17 = 0x70, PUSH18 = 0x71, PUSH19 = 0x72, PUSH20 = 0x73,
    PUSH21 = 0x74, PUSH22 = 0x75, PUSH23 = 0x76, PUSH24 = 0x77,
    PUSH25 = 0x78, PUSH26 = 0x79, PUSH27 = 0x7a, PUSH28 = 0x7b,
    PUSH29 = 0x7c, PUSH30 = 0x7d, PUSH31 = 0x7e, PUSH32 = 0x7f,

    // ─── Duplication Operations ───────────────────────────────────────────
    DUP1  = 0x80, DUP2  = 0x81, DUP3  = 0x82, DUP4  = 0x83,
    DUP5  = 0x84, DUP6  = 0x85, DUP7  = 0x86, DUP8  = 0x87,
    DUP9  = 0x88, DUP10 = 0x89, DUP11 = 0x8a, DUP12 = 0x8b,
    DUP13 = 0x8c, DUP14 = 0x8d, DUP15 = 0x8e, DUP16 = 0x8f,

    // ─── Exchange Operations ──────────────────────────────────────────────
    SWAP1  = 0x90, SWAP2  = 0x91, SWAP3  = 0x92, SWAP4  = 0x93,
    SWAP5  = 0x94, SWAP6  = 0x95, SWAP7  = 0x96, SWAP8  = 0x97,
    SWAP9  = 0x98, SWAP10 = 0x99, SWAP11 = 0x9a, SWAP12 = 0x9b,
    SWAP13 = 0x9c, SWAP14 = 0x9d, SWAP15 = 0x9e, SWAP16 = 0x9f,

    // ─── Logging Operations ───────────────────────────────────────────────
    LOG0 = 0xa0, LOG1 = 0xa1, LOG2 = 0xa2, LOG3 = 0xa3, LOG4 = 0xa4,

    // ─── System Operations ────────────────────────────────────────────────
    CREATE       = 0xf0,
    CALL         = 0xf1,
    CALLCODE     = 0xf2,
    RETURN       = 0xf3,
    DELEGATECALL = 0xf4,
    CREATE2      = 0xf5,
    STATICCALL   = 0xfa,
    REVERT       = 0xfd,
    INVALID      = 0xfe,
    SELFDESTRUCT = 0xff,
}

impl OpCode {
    pub fn from_byte(b: u8) -> Option<Self> {
        // SAFETY: repr(u8) — all variants are valid if in range.
        match b {
            0x00..=0x0b | 0x10..=0x1d | 0x20
            | 0x30..=0x4a | 0x50..=0x5f
            | 0x60..=0x7f | 0x80..=0x8f | 0x90..=0x9f
            | 0xa0..=0xa4 | 0xf0..=0xff => {
                Some(unsafe { std::mem::transmute(b) })
            }
            _ => None,
        }
    }

    /// Static gas cost (before dynamic costs).
    pub fn static_gas(self) -> u64 {
        match self {
            Self::STOP | Self::RETURN | Self::REVERT => 0,
            Self::ADD | Self::SUB | Self::NOT | Self::LT | Self::GT
            | Self::SLT | Self::SGT | Self::EQ | Self::ISZERO
            | Self::AND | Self::OR | Self::XOR | Self::BYTE
            | Self::SHL | Self::SHR | Self::SAR | Self::POP
            | Self::PUSH0 => 3,
            Self::MUL | Self::DIV | Self::SDIV | Self::MOD | Self::SMOD
            | Self::SIGNEXTEND => 5,
            Self::ADDMOD | Self::MULMOD => 8,
            Self::EXP => 10,   // +50 per byte
            Self::KECCAK256 => 30,
            Self::ADDRESS | Self::ORIGIN | Self::CALLER | Self::CALLVALUE
            | Self::CALLDATASIZE | Self::CODESIZE | Self::GASPRICE
            | Self::COINBASE | Self::TIMESTAMP | Self::NUMBER
            | Self::GASLIMIT | Self::CHAINID | Self::SELFBALANCE
            | Self::RETURNDATASIZE | Self::DIFFICULTY | Self::BASEFEE
            | Self::MSIZE | Self::GAS | Self::PC | Self::JUMPDEST => 2,
            Self::CALLDATALOAD | Self::MLOAD | Self::MSTORE | Self::MSTORE8 => 3,
            Self::BALANCE | Self::EXTCODESIZE | Self::EXTCODEHASH => 100, // warm; cold = 2600
            Self::SLOAD => 100, // warm; cold = 2100 (EIP-2929)
            Self::SSTORE => 100, // dynamic (EIP-2929 + EIP-3529)
            Self::BLOCKHASH => 20,
            Self::CREATE | Self::CREATE2 => 32_000,
            Self::CALL | Self::CALLCODE | Self::DELEGATECALL | Self::STATICCALL => 100,
            Self::SELFDESTRUCT => 5_000,
            Self::LOG0 => 375,
            Self::LOG1 => 750,
            Self::LOG2 => 1125,
            Self::LOG3 => 1500,
            Self::LOG4 => 1875,
            Self::JUMP => 8,
            Self::JUMPI => 10,
            Self::CALLDATACOPY | Self::CODECOPY | Self::RETURNDATACOPY => 3,
            Self::EXTCODECOPY => 100,
            // PUSH1..=PUSH32 (0x60..=0x7f), DUP1..=DUP16 (0x80..=0x8f),
            // SWAP1..=SWAP16 (0x90..=0x9f). Range patterns on enum variants
            // are not supported in stable Rust, so we match on the discriminant.
            op if matches!(op as u8, 0x60..=0x7f | 0x80..=0x8f | 0x90..=0x9f) => 3,
            Self::MCOPY => 3,
            Self::TLOAD | Self::TSTORE => 100,
            Self::BLOBHASH | Self::BLOBBASEFEE => 3,
            _ => 0,
        }
    }

    /// Is this opcode a PUSHn (1..=32) or PUSH0?
    pub fn is_push(self) -> bool {
        let b = self as u8;
        b == 0x5f || (0x60..=0x7f).contains(&b)
    }

    /// Is this opcode a DUP1..=DUP16?
    pub fn is_dup(self) -> bool {
        (0x80..=0x8f).contains(&(self as u8))
    }

    /// Is this opcode a SWAP1..=SWAP16?
    pub fn is_swap(self) -> bool {
        (0x90..=0x9f).contains(&(self as u8))
    }

    pub fn push_size(self) -> usize {
        match self {
            Self::PUSH0 => 0,
            Self::PUSH1 => 1,
            op if op as u8 >= 0x60 && op as u8 <= 0x7f => (op as u8 - 0x5f) as usize,
            _ => 0,
        }
    }
}

impl std::fmt::Display for OpCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}