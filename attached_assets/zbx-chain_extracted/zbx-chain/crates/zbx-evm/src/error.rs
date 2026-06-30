use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum EvmError {
    #[error("out of gas")]
    OutOfGas,
    #[error("stack underflow")]
    StackUnderflow,
    #[error("stack overflow (max 1024)")]
    StackOverflow,
    #[error("invalid opcode: 0x{0:02x}")]
    InvalidOpcode(u8),
    #[error("invalid jump destination: {0}")]
    InvalidJump(usize),
    #[error("memory out of bounds: offset {offset}, size {size}")]
    MemoryOutOfBounds { offset: usize, size: usize },
    #[error("write to static context")]
    WriteProtection,
    #[error("revert: {0}")]
    Revert(String),
    #[error("precompile error: {0}")]
    Precompile(String),
    #[error("call depth exceeded (max 1024)")]
    CallDepthExceeded,
    /// S32 — raised by SSTORE/SELFDESTRUCT/CREATE/LOG/CALL-with-value when
    /// inside a STATICCALL frame.
    #[error("static-context state change forbidden")]
    StaticStateChange,
    /// S32 — raised when a balance debit would underflow the sender's account.
    #[error("insufficient balance for value transfer")]
    InsufficientBalance,
    /// S32 — raised by `Host::inc_nonce` when nonce would exceed `u64::MAX`.
    /// Ethereum mainnet treats this as a transaction-failure condition rather
    /// than wrapping silently (which would collide CREATE addresses).
    #[error("account nonce would overflow u64::MAX")]
    NonceOverflow,
    /// S32 — raised by CREATE/CREATE2 when initcode exceeds EIP-3860
    /// `MAX_INIT_CODE_SIZE = 49152` bytes.
    #[error("initcode exceeds EIP-3860 size limit ({0} > 49152)")]
    InitcodeOversize(usize),
    /// S32 — raised by CREATE/CREATE2 when the deployed runtime code starts
    /// with `0xEF` (EIP-3541 reservation for EOF / future formats).
    #[error("deployed code starts with 0xEF (EIP-3541 reserved)")]
    InvalidDeployedCodePrefix,
    /// S32 — raised by CREATE/CREATE2 when the target address already has
    /// code or a non-zero nonce (collision).
    #[error("CREATE collision at target address")]
    CreateCollision,
}