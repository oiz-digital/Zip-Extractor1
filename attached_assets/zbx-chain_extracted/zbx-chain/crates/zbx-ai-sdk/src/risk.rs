//! Portfolio Risk Manager for AI Agents.
//!
//! Evaluates portfolio and market risk using both rule-based checks and
//! AI model signals. Returns a RiskLevel that agents use to gate actions.
//!
//! Risk Levels (ascending severity):
//!   Low → Medium → High → Critical
//!
//! When risk = Critical, all agent actions are blocked automatically.

use crate::{
    oracle::AggregatedPrice,
    agent::AiSignal,
};
use zbx_ai_precompile::ModelId;
use serde::{Serialize, Deserialize};

/// Risk level enum — ordered from safest to most dangerous.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl RiskLevel {
    pub fn as_u8(&self) -> u8 {
        match self {
            Self::Low      => 0,
            Self::Medium   => 1,
            Self::High     => 2,
            Self::Critical => 3,
        }
    }

    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Low,
            1 => Self::Medium,
            2 => Self::High,
            _ => Self::Critical,
        }
    }
}

/// Risk evaluation parameters (tunable by governance).
#[derive(Debug, Clone)]
pub struct RiskParams {
    /// Maximum spread in bps before escalating to High risk.
    pub max_spread_bps: u16,
    /// Maximum price deviation from 24h average (bps) → High risk.
    pub max_deviation_bps: u16,
    /// Minimum AI confidence to trust a prediction (bps).
    pub min_ai_confidence: u16,
    /// Oracle anomaly guard class that signals attack (0=normal, 1+=alert).
    pub anomaly_alert_class: u8,
}

impl Default for RiskParams {
    fn default() -> Self {
        Self {
            max_spread_bps:      500,  // 5% max spread
            max_deviation_bps:   1000, // 10% max deviation
            min_ai_confidence:   6000, // 60% minimum confidence
            anomaly_alert_class: 2,    // class 2+ = attack detected
        }
    }
}

/// Portfolio risk position.
#[derive(Debug, Clone, Default)]
pub struct PortfolioPosition {
    pub pair:          String,
    pub size_fp6:      u64,  // position size in fixed-point
    pub entry_fp6:     u64,  // entry price
    pub unrealized_pnl: i64, // unrealized P&L in fp6 (signed)
}

impl PortfolioPosition {
    pub fn update_pnl(&mut self, current_price: u64) {
        if self.entry_fp6 == 0 { return; }
        let current = current_price as i64;
        let entry   = self.entry_fp6 as i64;
        let size    = self.size_fp6 as i64;
        // PnL = size × (current - entry) / entry (integer)
        self.unrealized_pnl = (size * (current - entry)) / entry;
    }

    pub fn pnl_pct_bps(&self) -> i32 {
        if self.entry_fp6 == 0 { return 0; }
        let pnl  = self.unrealized_pnl as i128;
        let size = self.size_fp6 as i128;
        if size == 0 { return 0; }
        ((pnl * 10_000) / size) as i32
    }
}

/// The risk manager — evaluates current market + AI signals.
pub struct RiskManager {
    pub max_risk:   RiskLevel,
    pub params:     RiskParams,
    pub positions:  Vec<PortfolioPosition>,
    /// Running risk history (last 16 readings).
    history:        [RiskLevel; 16],
    history_idx:    usize,
}

impl RiskManager {
    pub fn new(max_risk: RiskLevel) -> Self {
        const INIT: RiskLevel = RiskLevel::Low;
        Self {
            max_risk,
            params: RiskParams::default(),
            positions: Vec::new(),
            history: [INIT, INIT, INIT, INIT, INIT, INIT, INIT, INIT,
                      INIT, INIT, INIT, INIT, INIT, INIT, INIT, INIT],
            history_idx: 0,
        }
    }

    /// Evaluate current risk from prices and AI signals.
    pub fn evaluate(&mut self, prices: &[AggregatedPrice], signals: &[AiSignal]) -> RiskLevel {
        let mut risk = RiskLevel::Low;

        // 1. Spread check
        for price in prices {
            if price.spread_bps() > self.params.max_spread_bps {
                risk = risk.max(RiskLevel::High);
                tracing::warn!(
                    pair = %price.pair,
                    spread = price.spread_bps(),
                    "High spread detected — escalating risk"
                );
            }
        }

        // 2. AI signal analysis
        for signal in signals {
            match signal.model {
                ModelId::OracleAnomalyGuard => {
                    if signal.class >= self.params.anomaly_alert_class
                        && signal.confidence >= self.params.min_ai_confidence
                    {
                        risk = risk.max(RiskLevel::Critical);
                        tracing::error!(
                            class = signal.class,
                            confidence = signal.confidence,
                            "Oracle anomaly detected — CRITICAL risk"
                        );
                    }
                }
                ModelId::FraudDetector => {
                    if signal.class >= 2 && signal.confidence >= self.params.min_ai_confidence {
                        risk = risk.max(RiskLevel::High);
                    }
                }
                ModelId::MevDetector => {
                    if signal.class >= 2 && signal.confidence >= self.params.min_ai_confidence {
                        risk = risk.max(RiskLevel::Medium);
                    }
                }
                ModelId::SentimentClassifier => {
                    // class 2 = bearish with high confidence → medium risk
                    if signal.class == 2 && signal.confidence >= 8000 {
                        risk = risk.max(RiskLevel::Medium);
                    }
                }
                _ => {}
            }
        }

        // 3. Portfolio P&L check
        for pos in &self.positions {
            let pnl_bps = pos.pnl_pct_bps();
            if pnl_bps < -2000 { // -20% loss
                risk = risk.max(RiskLevel::High);
            }
            if pnl_bps < -5000 { // -50% loss
                risk = risk.max(RiskLevel::Critical);
            }
        }

        // Record in history
        self.history[self.history_idx % 16] = risk.clone();
        self.history_idx = self.history_idx.wrapping_add(1);

        risk
    }

    /// Check if the last N readings were all High or Critical (sustained risk).
    pub fn is_sustained_high_risk(&self, n: usize) -> bool {
        let n = n.min(16);
        let start = self.history_idx.saturating_sub(n);
        (start..self.history_idx).all(|i| {
            self.history[i % 16] >= RiskLevel::High
        })
    }

    pub fn add_position(&mut self, pos: PortfolioPosition) {
        self.positions.push(pos);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oracle::AggregatedPrice;

    fn price(pair: &str, spread_bps: u16) -> AggregatedPrice {
        let median = 1_000_000u64;
        let half_spread = (median * spread_bps as u64) / 20_000;
        AggregatedPrice {
            pair:        pair.to_string(),
            median_fp6:  median,
            min_fp6:     median - half_spread,
            max_fp6:     median + half_spread,
            num_sources: 3,
            timestamp:   1_700_000_000,
        }
    }

    #[test]
    fn low_spread_gives_low_risk() {
        let mut rm = RiskManager::new(RiskLevel::High);
        let level = rm.evaluate(&[price("ZBX/USDT", 100)], &[]);
        assert_eq!(level, RiskLevel::Low);
    }

    #[test]
    fn high_spread_gives_high_risk() {
        let mut rm = RiskManager::new(RiskLevel::High);
        let level = rm.evaluate(&[price("ZBX/USDT", 600)], &[]); // 600 bps > 500 threshold
        assert_eq!(level, RiskLevel::High);
    }

    #[test]
    fn oracle_anomaly_critical() {
        let mut rm = RiskManager::new(RiskLevel::Critical);
        let signal = AiSignal {
            model:      ModelId::OracleAnomalyGuard,
            class:      3, // emergency
            confidence: 9000,
        };
        let level = rm.evaluate(&[price("ZBX/USDT", 50)], &[signal]);
        assert_eq!(level, RiskLevel::Critical);
    }

    #[test]
    fn position_pnl_bps() {
        let mut pos = PortfolioPosition {
            pair:           "ZBX/USDT".to_string(),
            size_fp6:       1_000_000,
            entry_fp6:      1_000_000,
            unrealized_pnl: 0,
        };
        pos.update_pnl(1_100_000); // +10%
        assert_eq!(pos.pnl_pct_bps(), 1000); // 10% = 1000 bps
    }

    #[test]
    fn risk_level_ordering() {
        assert!(RiskLevel::Low < RiskLevel::Medium);
        assert!(RiskLevel::Medium < RiskLevel::High);
        assert!(RiskLevel::High < RiskLevel::Critical);
    }
}
