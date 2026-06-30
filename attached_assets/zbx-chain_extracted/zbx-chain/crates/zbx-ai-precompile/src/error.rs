use thiserror::Error;
use crate::model::ModelId;

#[derive(Debug, Error)]
pub enum AiError {
    #[error("out of gas: required {required}, available {available}")]
    OutOfGas { required: u64, available: u64 },

    #[error("input too large: {0} bytes (max 1024)")]
    InputTooLarge(usize),

    #[error("input size mismatch: expected {expected} bytes, got {got}")]
    InputSizeMismatch { expected: usize, got: usize },

    #[error("model not found: {0:?}")]
    ModelNotFound(ModelId),

    #[error("inference runtime not available in this build")]
    RuntimeNotAvailable,

    #[error("model weights not available on DA layer")]
    ModelWeightsUnavailable,

    #[error("weight hash mismatch: expected {expected}, actual {actual}")]
    WeightHashMismatch { expected: String, actual: String },

    #[error("invalid model weights: {0}")]
    InvalidModelWeights(String),

    #[error("determinism violation: validators disagree on inference output")]
    DeterminismViolation,

    #[error("ABI decode error: {0}")]
    AbiDecodeError(String),

    #[error("ABI encode error: {0}")]
    AbiEncodeError(String),

    #[error("inference error: {0}")]
    Inference(String),

    #[error("model version mismatch: expected {expected}, got {got}")]
    VersionMismatch { expected: u32, got: u32 },

    #[error("circuit breaker open: model {model:?} is suspended")]
    ModelCircuitOpen { model: ModelId },

    #[error("rate limit exceeded: {calls_per_block} calls per block (max {max})")]
    RateLimitExceeded { calls_per_block: u32, max: u32 },

    #[error("weight file not found: {0} — {1}")]
    WeightFileNotFound(String, String),
}
