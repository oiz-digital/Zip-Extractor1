use thiserror::Error;

#[derive(Debug, Error)]
pub enum SdkError {
    #[error("oracle: no providers configured")]
    OracleNoProviders,

    #[error("oracle: insufficient sources — got {got}, required {required}")]
    OracleInsuffientSources { got: usize, required: usize },

    #[error("oracle: price stale for pair {pair} — staleness {staleness_secs}s")]
    OracleStalePrices { pair: String, staleness_secs: u64 },

    #[error("oracle: pair {pair} not found")]
    OraclePairNotFound { pair: String },

    #[error("session key expired — expires at block {expires_at}, current block {current_block}")]
    SessionKeyExpired { expires_at: u64, current_block: u64 },

    #[error("session key value exceeded — requested {requested} wei, max {max} wei")]
    SessionKeyValueExceeded { requested: u128, max: u128 },

    #[error("session key: contract {target} not in allowed list")]
    SessionKeyContractNotAllowed { target: String },

    #[error("session key revoked")]
    SessionKeyRevoked,

    #[error("strategy: no rules defined")]
    StrategyEmpty,

    #[error("risk: level {level:?} exceeds maximum allowed")]
    RiskLevelExceeded { level: String },

    #[error("agent paused (emergency stop)")]
    AgentPaused,

    #[error("inference error: {0}")]
    Inference(String),

    #[error("serialization error: {0}")]
    Serialization(String),
}
