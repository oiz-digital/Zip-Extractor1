//! Typed errors for the zbx-perp engine.

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PerpError {
    // ── Market ─────────────────────────────────────────────────────────────
    #[error("market {0} not found")]
    MarketNotFound(u64),
    #[error("market {0} is inactive")]
    MarketInactive(u64),
    #[error("leverage {got} exceeds market maximum {max}")]
    LeverageTooHigh { got: u64, max: u64 },
    #[error("leverage must be at least 1")]
    ZeroLeverage,
    #[error("oracle address is zero")]
    InvalidOracle,
    #[error("oracle price is stale (updated {age_secs}s ago, max {max_secs}s)")]
    StaleOracle { age_secs: u64, max_secs: u64 },

    // ── Position ───────────────────────────────────────────────────────────
    #[error("position {0} not found")]
    PositionNotFound(u64),
    #[error("sender is not the owner of position {0}")]
    NotPositionOwner(u64),
    #[error("position {0} is already closed / liquidated")]
    AlreadyClosed(u64),
    #[error("collateral must be > 0")]
    ZeroCollateral,
    #[error("amount must be > 0")]
    ZeroAmount,

    // ── Order validation ──────────────────────────────────────────────────
    #[error("stop-loss price is invalid for this direction (long: SL < entry, short: SL > entry)")]
    InvalidStopLoss,
    #[error("take-profit price is invalid for this direction (long: TP > entry, short: TP < entry)")]
    InvalidTakeProfit,
    #[error("basis-points value must be 1–10000")]
    InvalidBps,
    #[error("trailing-stop basis-points must be 1–{}", crate::MAX_TRAIL_BPS)]
    InvalidTrailBps,
    #[error("trailing stop did not move favourably — mark price has not improved past the peak")]
    TrailNotFavourable,
    #[error("neither SL nor TP is triggered for position {0}")]
    NeitherTriggered(u64),
    #[error("stop-loss is not triggered for position {0}")]
    SLNotTriggered(u64),
    #[error("take-profit is not triggered for position {0}")]
    TPNotTriggered(u64),

    // ── Liquidation ────────────────────────────────────────────────────────
    #[error("position {0} is not liquidatable (health above maintenance margin)")]
    NotLiquidatable(u64),

    // ── Cross margin ───────────────────────────────────────────────────────
    #[error("insufficient cross-margin balance (have {have}, need {need})")]
    InsufficientCrossMargin { have: u128, need: u128 },
    #[error("cross withdraw too large (free margin: {free}, requested: {requested})")]
    CrossWithdrawTooLarge { free: u128, requested: u128 },
    #[error("addCollateral is only valid on isolated positions")]
    NotIsolatedPosition,

    // ── Access ──────────────────────────────────────────────────────────────
    #[error("caller is not the contract owner")]
    NotOwner,

    // ── Internal ─────────────────────────────────────────────────────────
    #[error("arithmetic overflow in perp engine")]
    Overflow,
    #[error("oracle returned zero price for market {0}")]
    ZeroOraclePrice(u64),
}
