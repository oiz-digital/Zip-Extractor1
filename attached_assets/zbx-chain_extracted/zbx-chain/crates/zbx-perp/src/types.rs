//! Core types for the perpetuals engine — mirrors ZbxPerpetuals.sol structs exactly.

use zbx_types::address::Address;

// ─── Market ──────────────────────────────────────────────────────────────────

/// A trading pair registered in the perpetuals engine.
/// Mirrors the `Market` struct in ZbxPerpetuals.sol.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Market {
    /// Human-readable ticker (e.g. "BTC", "ETH", "ZBX").
    pub symbol: String,
    /// Chainlink-compatible oracle address for this asset.
    pub oracle: Address,
    /// Whether the market can accept new positions.
    pub active: bool,
    /// Per-market leverage cap (1–MAX_LEVERAGE).
    pub max_leverage: u64,
    /// Total notional long open interest (18-decimal wei).
    pub total_long_oi: u128,
    /// Total notional short open interest (18-decimal wei).
    pub total_short_oi: u128,
    /// Cumulative 8-hour funding index (signed, 1e10 scale).
    pub cumulative_funding: i128,
    /// Unix timestamp of the last 8-hour funding settlement.
    pub last_funding_update: u64,
}

impl Market {
    pub fn new(symbol: String, oracle: Address, max_leverage: u64, now: u64) -> Self {
        Self {
            symbol,
            oracle,
            active: true,
            max_leverage,
            total_long_oi: 0,
            total_short_oi: 0,
            cumulative_funding: 0,
            last_funding_update: now,
        }
    }
}

// ─── Position ─────────────────────────────────────────────────────────────────

/// A perpetual position.
/// Mirrors the `Position` struct in ZbxPerpetuals.sol (all 14 fields).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Position {
    /// Position owner.
    pub trader: Address,
    /// Which market (0-indexed).
    pub market_id: u64,
    /// true = long, false = short.
    pub is_long: bool,
    /// true = cross-margin account, false = isolated.
    pub is_cross: bool,
    /// Isolated collateral (0 for cross positions).
    pub collateral: u128,
    /// Notional size = (col − fee) × leverage.
    pub size: u128,
    /// Entry oracle price (18-decimal wei).
    pub entry_price: u128,
    /// Cumulative funding rate captured at open (signed).
    pub funding_entry_rate: i128,
    /// Stop-loss price (0 = none).
    pub stop_loss: u128,
    /// Take-profit price (0 = none).
    pub take_profit: u128,
    /// Trailing-stop width in basis points (0 = disabled).
    pub trail_bps: u64,
    /// Highest (long) or lowest (short) mark price seen for trailing SL.
    pub trail_peak: u128,
    /// Whether the position has been closed or liquidated.
    pub closed: bool,
    /// Per-position initial margin share (for cross IM accounting).
    pub initial_margin: u128,
}

// ─── Cross account ────────────────────────────────────────────────────────────

/// Shared margin account for a single trader across all markets.
/// Mirrors the `CrossAccount` struct in ZbxPerpetuals.sol.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CrossAccount {
    /// Deposited collateral balance.
    pub balance: u128,
    /// Sum of initial margin locked by all open cross positions.
    pub initial_margin: u128,
    /// IDs of all open cross positions for this trader.
    pub pos_ids: Vec<u64>,
}

// ─── Engine snapshots (read-only views) ──────────────────────────────────────

/// Enriched market snapshot returned by view queries.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MarketView {
    pub market_id:         u64,
    pub symbol:            String,
    pub oracle:            Address,
    pub active:            bool,
    pub max_leverage:      u64,
    pub total_long_oi:     u128,
    pub total_short_oi:    u128,
    pub oi_imbalance:      i128,      // long_oi − short_oi (signed)
    pub cumulative_funding: i128,
    pub current_funding:   i128,      // current 8-hour rate
    pub next_funding_in:   u64,       // seconds until next settlement (0 if overdue)
    pub mark_price:        u128,
}

/// Full position snapshot including live health metrics.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PositionView {
    pub position_id:       u64,
    pub trader:            Address,
    pub market_id:         u64,
    pub is_long:           bool,
    pub is_cross:          bool,
    pub collateral:        u128,
    pub size:              u128,
    pub entry_price:       u128,
    pub funding_entry_rate: i128,
    pub stop_loss:         u128,
    pub take_profit:       u128,
    pub trail_bps:         u64,
    pub trail_peak:        u128,
    pub closed:            bool,
    pub initial_margin:    u128,
    // Live fields
    pub unrealised_pnl:    i128,
    pub health_bps:        u64,       // 0 = liquidatable, 10000 = fully margined
    pub liquidation_price: u128,      // 0 for cross positions
    pub is_sl_triggered:   bool,
    pub is_tp_triggered:   bool,
    pub is_liquidatable:   bool,
}

/// Cross-margin account snapshot.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CrossAccountView {
    pub trader:        Address,
    pub balance:       u128,
    pub equity:        i128,          // balance + sum(unrealised PnL)
    pub maint_margin:  u128,          // sum(size_i × 10%)
    pub free_margin:   u128,          // max(0, equity − maint_margin)
    pub liq_threshold: u128,          // = maint_margin
    pub liquidatable:  bool,
    pub position_ids:  Vec<u64>,
}

// ─── Tx input types ──────────────────────────────────────────────────────────

/// Parameters for `open_position`.
#[derive(Debug, Clone)]
pub struct OpenPositionParams {
    pub market_id:  u64,
    pub is_long:    bool,
    /// Collateral (wei). For isolated: transferred from sender. For cross: deducted from balance.
    pub collateral: u128,
    /// Leverage multiplier (1–max_leverage). Size = (col − fee) × leverage.
    pub leverage:   u64,
    pub is_cross:   bool,
    /// Stop-loss price (0 = none).
    pub sl_price:   u128,
    /// Take-profit price (0 = none).
    pub tp_price:   u128,
}

/// Result of opening a position.
#[derive(Debug, Clone)]
pub struct OpenPositionResult {
    pub position_id: u64,
    pub size:        u128,
    pub entry_price: u128,
    pub fee_charged: u128,
}

/// Result of closing a position (full or partial).
#[derive(Debug, Clone)]
pub struct CloseResult {
    pub exit_price: u128,
    pub net_pnl:    i128,
    /// Amount paid out to the trader (0 if fully wiped by loss).
    pub payout:     u128,
    /// Protocol fee deducted on close.
    pub fee:        u128,
}

/// Result of a liquidation.
#[derive(Debug, Clone)]
pub struct LiquidationResult {
    pub exit_price:    u128,
    /// Bounty paid to the keeper (1% of collateral).
    pub keeper_bounty: u128,
    /// Remaining collateral credited to protocol fee balance.
    pub protocol_fee:  u128,
}
