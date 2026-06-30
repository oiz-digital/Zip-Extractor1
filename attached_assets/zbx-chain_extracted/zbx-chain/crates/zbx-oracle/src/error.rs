use thiserror::Error;
use crate::feed::FeedId;

#[derive(Debug, Error)]
pub enum OracleError {
    #[error("no reports submitted")]
    NoReports,

    #[error("insufficient reporters: required {required}, got {got}")]
    InsufficientReporters { required: usize, got: usize },

    #[error("too many outliers: {outliers}/{total} removed")]
    TooManyOutliers { total: usize, outliers: usize },

    #[error("round {0} is not open")]
    RoundNotOpen(u64),

    #[error("no open round for this feed")]
    NoOpenRound,

    #[error("reporter {0:?} not in whitelist")]
    UnauthorizedReporter([u8; 20]),

    #[error("duplicate report from reporter {0:?}")]
    DuplicateReport([u8; 20]),

    #[error("invalid signature on report")]
    InvalidSignature,

    #[error("report expired (too old)")]
    ReportExpired,

    #[error("invalid price: {0}")]
    InvalidPrice(i128),

    #[error("unknown feed: {0}")]
    UnknownFeed(FeedId),

    #[error("all external price sources failed for {0}")]
    AllSourcesFailed(FeedId),

    /// All live sources failed but a cached price was returned.
    /// This is NOT an error — callers receive `Ok(price)`.
    /// Use `usd_inr_cache_age_secs()` to check staleness.
    /// Cache is valid up to 30 days (`MAX_CACHE_AGE_SECS`).
    #[error("stale cached price used for {feed}: {age_hours}h old (max 720h)")]
    StalePriceUsed { feed: String, age_hours: u64 },

    /// All live sources failed AND the cache is expired (>30 days) or empty.
    #[error("all sources failed and no valid cache for {0} (cache expired or empty)")]
    AllSourcesFailedNoCache(FeedId),

    #[error("HTTP error fetching price: {0}")]
    Http(String),

    #[error("on-chain submission failed: {0}")]
    OnChainSubmit(String),

    /// Circuit breaker: reported price below configured minimum.
    /// Prevents near-zero price manipulation attacks.
    #[error("price below min_answer for feed {feed}: got {reported}, min is {min_answer}")]
    BelowMinAnswer {
        feed:       String,
        reported:   i128,
        min_answer: i128,
    },

    /// Circuit breaker: reported price above configured maximum.
    /// Prevents astronomical price manipulation attacks.
    #[error("price above max_answer for feed {feed}: got {reported}, max is {max_answer}")]
    AboveMaxAnswer {
        feed:       String,
        reported:   i128,
        max_answer: i128,
    },
}