//! ZVM error types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ZvmError {
    #[error("stack overflow (max 1024 items)")]
    StackOverflow,

    #[error("stack underflow")]
    StackUnderflow,

    #[error("out of gas")]
    OutOfGas,

    #[error("invalid jump destination: {0}")]
    InvalidJump(usize),

    #[error("invalid opcode: 0x{0:02X}")]
    InvalidOpcode(u8),

    #[error("state change in static context")]
    StaticStateChange,

    #[error("unexpected end of bytecode")]
    UnexpectedEnd,

    #[error("invalid UTF-8 in memory")]
    InvalidUtf8,

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("insufficient balance for ZBXBURN")]
    InsufficientBalance,

    #[error("precompile not found at address {0:?}")]
    PrecompileNotFound([u8; 20]),

    #[error("Pay ID not found: {0}")]
    PayIdNotFound(String),

    #[error("ZVM internal error: {0}")]
    Internal(String),
}