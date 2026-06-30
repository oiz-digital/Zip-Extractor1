//! zbx-perp: On-chain perpetual futures engine for ZBX Chain.
//!
//! Implements ZEP-034 rev5 — Multi-Market, 200× leverage, Isolated + Cross margin,
//! SL/TP, Trailing-stop, Liquidation price view, 8-hour funding rate.
//!
//! ## Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────────────┐
//! │  Block Executor                                                       │
//! │    tx.to == PERP_CONTRACT_ADDR → tx_handler::dispatch_perp_call()   │
//! └───────────────────────────────────┬──────────────────────────────────┘
//!                                     │
//!                             ┌───────▼───────┐
//!                             │  PerpEngine   │  ← engine.rs
//!                             └──┬──────┬─────┘
//!          ┌─────────────────────┘      └───────────────────┐
//!   ┌──────▼──────┐                               ┌─────────▼────────┐
//!   │ MarketRegistry│  ← market.rs                │ PositionStore    │  ← position.rs
//!   └──────┬──────┘                               └─────────┬────────┘
//!          │  funding.rs                     order.rs ───────┘
//!          │  (8-hour settlement)            liquidation.rs ─┘
//!          └─────────────────────────────────────────────────┘
//! ```
//!
//! ## Key constants (ZbxPerpetuals.sol v5)
//!
//! | Constant                  | Value  | Meaning                                    |
//! |---------------------------|--------|--------------------------------------------|
//! | `MAX_LEVERAGE`            | 200    | Global upper bound; per-market cap ≤ this  |
//! | `MAINTENANCE_MARGIN_BPS`  | 1000   | 10% of position size                       |
//! | `PROTOCOL_FEE_BPS`        | 10     | 0.10% on open and close                    |
//! | `KEEPER_BOUNTY_BPS`       | 5      | 0.05% of collateral for trigger callers    |
//! | `LIQUIDATION_BOUNTY_BPS`  | 100    | 1.00% of collateral for liquidators        |
//! | `FUNDING_INTERVAL`        | 28800s | 8 hours                                    |
//! | `MAX_TRAIL_BPS`           | 5000   | 50% max trailing-stop width                |

pub mod engine;
pub mod error;
pub mod funding;
pub mod liquidation;
pub mod market;
pub mod order;
pub mod position;
pub mod tx_handler;
pub mod types;

// ── Re-exports ────────────────────────────────────────────────────────────────

pub use engine::{OracleProvider, PerpEngine};
pub use error::PerpError;
pub use funding::{
    current_funding_rate, funding_cost_for_position, next_funding_in, settle_funding,
    FUNDING_RATE_SCALE,
};
pub use liquidation::{is_cross_liquidatable, is_isolated_liquidatable, liquidate, liquidate_cross};
pub use market::MarketRegistry;
pub use order::{
    set_stop_loss, set_take_profit, set_trailing_stop,
    sl_hit, tp_hit,
    trigger_order, trigger_stop_loss, trigger_take_profit,
    update_trailing_stop, validate_sl, validate_tp,
};
pub use position::{
    keeper_bounty_for, liquidation_bounty_for, pnl_for, PositionStore,
};
pub use tx_handler::{
    decode_perp_call, dispatch_perp_call, is_perp_destination, PerpCall,
    PERP_CONTRACT_ADDR,
    GAS_OPEN_POSITION, GAS_CLOSE_POSITION, GAS_PARTIAL_CLOSE, GAS_ADD_COLLATERAL,
    GAS_SET_STOP_LOSS, GAS_SET_TAKE_PROFIT, GAS_SET_TRAILING_STOP,
    GAS_UPDATE_TRAILING_STOP, GAS_TRIGGER_ORDER, GAS_TRIGGER_SL, GAS_TRIGGER_TP,
    GAS_LIQUIDATE, GAS_LIQUIDATE_CROSS, GAS_DEPOSIT_CROSS, GAS_WITHDRAW_CROSS,
    GAS_UPDATE_FUNDING,
    SEL_OPEN_POSITION, SEL_CLOSE_POSITION, SEL_PARTIAL_CLOSE, SEL_ADD_COLLATERAL,
    SEL_SET_STOP_LOSS, SEL_SET_TAKE_PROFIT, SEL_SET_TRAILING_STOP,
    SEL_UPDATE_TRAILING_STOP, SEL_TRIGGER_ORDER, SEL_TRIGGER_SL, SEL_TRIGGER_TP,
    SEL_LIQUIDATE, SEL_LIQUIDATE_CROSS, SEL_DEPOSIT_CROSS, SEL_WITHDRAW_CROSS,
    SEL_UPDATE_FUNDING,
};
pub use types::{
    CloseResult, CrossAccount, CrossAccountView, LiquidationResult,
    Market, MarketView, OpenPositionParams, OpenPositionResult,
    Position, PositionView,
};

// ── Protocol constants (single source of truth — match ZbxPerpetuals.sol v5) ─

/// Global maximum leverage allowed by any market (ZbxPerpetuals.sol: MAX_LEVERAGE).
pub const MAX_LEVERAGE: u64 = 200;
/// Maintenance margin as basis points of position size (10% = 1000 bps).
pub const MAINTENANCE_MARGIN_BPS: u16 = 1_000;
/// Protocol fee charged on open and close (0.10% = 10 bps).
pub const PROTOCOL_FEE_BPS: u16 = 10;
/// Bounty paid to the keeper that triggers an SL/TP order (0.05% = 5 bps).
pub const KEEPER_BOUNTY_BPS: u16 = 5;
/// Bounty paid to the keeper that liquidates an isolated position (1% = 100 bps).
pub const LIQUIDATION_BOUNTY_BPS: u16 = 100;
/// Funding settlement interval in seconds (8 hours = 28 800 s).
pub const FUNDING_INTERVAL: u64 = 8 * 60 * 60; // 28 800
/// Maximum trailing-stop width in basis points (50% = 5000 bps).
pub const MAX_TRAIL_BPS: u64 = 5_000;
