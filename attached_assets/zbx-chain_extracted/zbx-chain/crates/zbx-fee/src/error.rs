use thiserror::Error;

#[derive(Debug, Error)]
pub enum FeeError {
    #[error("base fee overflow")]
    BaseFeeOverflow,
    #[error("block gas limit exceeded: used={used}, limit={limit}")]
    GasLimitExceeded { used: u64, limit: u64 },
    #[error("fee history range too large: requested={req}, max={max}")]
    FeeHistoryRangeTooLarge { req: u64, max: u64 },
    #[error("insufficient tip: min={min}, got={got}")]
    InsufficientTip { min: u64, got: u64 },
    #[error("max fee below base fee: max_fee={max_fee}, base_fee={base_fee}")]
    MaxFeeBelowBaseFee { max_fee: u64, base_fee: u64 },
}