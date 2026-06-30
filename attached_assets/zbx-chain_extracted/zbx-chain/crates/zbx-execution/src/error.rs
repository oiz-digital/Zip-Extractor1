use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("out of gas: used {used}, limit {limit}")]
    OutOfGas { used: u64, limit: u64 },

    #[error("invalid nonce: expected {expected}, got {got}")]
    InvalidNonce { expected: u64, got: u64 },

    #[error("insufficient balance: have {balance} wei, need {cost} wei")]
    InsufficientBalance { balance: u128, cost: u128 },

    #[error("intrinsic gas too low: required {required}, provided {provided}")]
    IntrinsicGasTooLow { required: u64, provided: u64 },

    #[error("contract creation failed: {0}")]
    CreateFailed(String),

    #[error("revert: {0}")]
    Revert(String),

    #[error("stack overflow")]
    StackOverflow,

    #[error("invalid jump destination")]
    InvalidJump,

    #[error("state error: {0}")]
    State(String),

    #[error("block validation failed: {0}")]
    Validation(String),
}