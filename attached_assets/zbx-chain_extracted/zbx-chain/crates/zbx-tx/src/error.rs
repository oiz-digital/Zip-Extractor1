use thiserror::Error;

#[derive(Debug, Error)]
pub enum TxError {
    #[error("wrong chain ID: expected {expected}, got {got}")]
    WrongChainId { expected: u64, got: u64 },
    #[error("gas limit too low: minimum intrinsic gas {min}, got {got}")]
    GasLimitTooLow { min: u64, got: u64 },
    #[error("gas limit too high: max {limit}, got {got}")]
    GasLimitTooHigh { limit: u64, got: u64 },
    #[error("fee too low: min {min}, got {got}")]
    FeeTooLow { min: u128, got: u128 },
    #[error("priority fee exceeds max fee")]
    PriorityFeeExceedsMaxFee,
    #[error("invalid signature")]
    InvalidSignature,
    /// TX-SEC-01 (EIP-2): high-S signature rejected — malleable signature.
    #[error("high-S signature rejected (EIP-2): s must be ≤ secp256k1 half-curve order")]
    HighSSignature,
    /// TX-VAL-01: calldata payload exceeds the per-transaction size cap.
    #[error("calldata too large: max {max} bytes, got {got}")]
    CalldataTooLarge { max: usize, got: usize },
    /// TX-VAL-02 (EIP-3860): contract creation initcode exceeds the protocol limit.
    #[error("initcode too large (EIP-3860): max {max} bytes, got {got}")]
    InitcodeTooLarge { max: usize, got: usize },
    #[error("RLP decode error: {0}")]
    Rlp(String),
}