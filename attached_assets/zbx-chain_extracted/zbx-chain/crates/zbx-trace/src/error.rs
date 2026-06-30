use thiserror::Error;

#[derive(Debug, Error)]
pub enum TraceError {
    #[error("tx not found: {0}")]
    TxNotFound(String),
    #[error("block not found: {0}")]
    BlockNotFound(u64),
    #[error("tracer config invalid: {0}")]
    InvalidConfig(String),
    #[error("trace replay failed: {0}")]
    ReplayFailed(String),
    #[error("output too large: {size} bytes (max {max})")]
    OutputTooLarge { size: usize, max: usize },
}