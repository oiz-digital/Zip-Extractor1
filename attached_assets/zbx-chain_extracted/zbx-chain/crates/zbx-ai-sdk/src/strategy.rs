//! Strategy DSL — rule-based AI trading strategy engine.
//!
//! A Strategy is a list of Rules. Each Rule has:
//!   - A Condition (price threshold, AI signal, block time, etc.)
//!   - An Action (swap, stake, rebalance, alert, etc.)
//!
//! Rules are evaluated in priority order. Once MAX_ACTIONS_PER_RULE actions
//! are scheduled, evaluation stops.
//!
//! This is intentionally simple (no Turing-complete scripting) to prevent
//! unbounded computation. All conditions are O(1) checks.

use crate::{oracle::AggregatedPrice, agent::AiSignal};
use zbx_ai_precompile::ModelId;
use serde::{Serialize, Deserialize};

/// Maximum actions a single strategy evaluation can produce.
pub const MAX_STRATEGY_ACTIONS: usize = 8;

/// A scheduled action from the strategy engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyAction {
    pub kind:       ActionKind,
    pub pair:       String,
    pub amount_fp6: u64,
    pub reason:     String,
    pub priority:   u8,
}

/// Types of actions an agent can take.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionKind {
    /// Swap token A → token B on ZBX DEX.
    Swap { from_token: String, to_token: String },
    /// Add liquidity to a pool.
    AddLiquidity { pool: String },
    /// Remove liquidity from a pool.
    RemoveLiquidity { pool: String },
    /// Stake ZBX tokens.
    Stake,
    /// Unstake ZBX tokens.
    Unstake,
    /// Send alert (no on-chain tx, just log).
    Alert { message: String },
    /// Do nothing (null action).
    NoOp,
}

/// A condition that must be true for a rule to fire.
#[derive(Debug, Clone)]
pub enum Condition {
    /// Price is above threshold (in fp6).
    PriceAbove { pair: String, threshold_fp6: u64 },
    /// Price is below threshold (in fp6).
    PriceBelow { pair: String, threshold_fp6: u64 },
    /// AI model output class matches.
    AiClass { model: ModelId, class: u8, min_confidence: u16 },
    /// AI model confidence exceeds threshold.
    AiConfident { model: ModelId, min_confidence: u16 },
    /// Block number is a multiple of N (periodic).
    EveryNBlocks { n: u64 },
    /// Spread is above threshold.
    SpreadAbove { pair: String, bps: u16 },
    /// Always true.
    Always,
    /// AND of two conditions.
    And(Box<Condition>, Box<Condition>),
    /// OR of two conditions.
    Or(Box<Condition>, Box<Condition>),
    /// NOT of a condition.
    Not(Box<Condition>),
}

impl Condition {
    pub fn evaluate(
        &self,
        prices:       &[AggregatedPrice],
        signals:      &[AiSignal],
        block_number: u64,
    ) -> bool {
        match self {
            Self::Always => true,

            Self::PriceAbove { pair, threshold_fp6 } => {
                prices.iter()
                    .find(|p| p.pair == *pair)
                    .map(|p| p.median_fp6 > *threshold_fp6)
                    .unwrap_or(false)
            }

            Self::PriceBelow { pair, threshold_fp6 } => {
                prices.iter()
                    .find(|p| p.pair == *pair)
                    .map(|p| p.median_fp6 < *threshold_fp6)
                    .unwrap_or(false)
            }

            Self::AiClass { model, class, min_confidence } => {
                signals.iter()
                    .find(|s| s.model == *model)
                    .map(|s| s.class == *class && s.confidence >= *min_confidence)
                    .unwrap_or(false)
            }

            Self::AiConfident { model, min_confidence } => {
                signals.iter()
                    .find(|s| s.model == *model)
                    .map(|s| s.confidence >= *min_confidence)
                    .unwrap_or(false)
            }

            Self::EveryNBlocks { n } => {
                *n > 0 && block_number % n == 0
            }

            Self::SpreadAbove { pair, bps } => {
                prices.iter()
                    .find(|p| p.pair == *pair)
                    .map(|p| p.spread_bps() > *bps)
                    .unwrap_or(false)
            }

            Self::And(a, b) => {
                a.evaluate(prices, signals, block_number)
                    && b.evaluate(prices, signals, block_number)
            }

            Self::Or(a, b) => {
                a.evaluate(prices, signals, block_number)
                    || b.evaluate(prices, signals, block_number)
            }

            Self::Not(inner) => {
                !inner.evaluate(prices, signals, block_number)
            }
        }
    }
}

/// A single strategy rule: if condition → produce action.
#[derive(Debug)]
pub struct Rule {
    pub name:      String,
    pub condition: Condition,
    pub action:    ActionTemplate,
    pub priority:  u8,
}

/// Template for generating an action (filled in at runtime).
#[derive(Debug, Clone)]
pub struct ActionTemplate {
    pub kind:       ActionKind,
    pub pair:       String,
    pub amount_fp6: u64,
    pub reason:     String,
}

impl ActionTemplate {
    pub fn to_action(&self, priority: u8) -> StrategyAction {
        StrategyAction {
            kind:       self.kind.clone(),
            pair:       self.pair.clone(),
            amount_fp6: self.amount_fp6,
            reason:     self.reason.clone(),
            priority,
        }
    }
}

/// A complete strategy — ordered list of rules.
pub struct Strategy {
    rules: Vec<Rule>,
}

impl Strategy {
    pub fn new(mut rules: Vec<Rule>) -> Self {
        // Sort by priority descending (highest priority first)
        rules.sort_by(|a, b| b.priority.cmp(&a.priority));
        Self { rules }
    }

    /// Strategy with no rules (useful for testing).
    pub fn do_nothing() -> Self { Self { rules: vec![] } }

    /// Evaluate all rules and collect actions.
    pub fn evaluate(
        &self,
        prices:       &[AggregatedPrice],
        signals:      &[AiSignal],
        block_number: u64,
    ) -> Vec<StrategyAction> {
        let mut actions = Vec::new();
        for rule in &self.rules {
            if actions.len() >= MAX_STRATEGY_ACTIONS { break; }
            if rule.condition.evaluate(prices, signals, block_number) {
                actions.push(rule.action.to_action(rule.priority));
                tracing::debug!(
                    rule  = %rule.name,
                    block = block_number,
                    "Strategy rule fired"
                );
            }
        }
        actions
    }

    /// Build a standard DeFi strategy: buy low, sell high, alert on anomaly.
    pub fn standard_defi(pair: &str, buy_below: u64, sell_above: u64) -> Self {
        let rules = vec![
            Rule {
                name:      format!("{pair} buy-the-dip"),
                priority:  10,
                condition: Condition::PriceBelow {
                    pair:          pair.to_string(),
                    threshold_fp6: buy_below,
                },
                action: ActionTemplate {
                    kind:       ActionKind::Swap {
                        from_token: "USDT".to_string(),
                        to_token:   pair.split('/').next().unwrap_or("ZBX").to_string(),
                    },
                    pair:       pair.to_string(),
                    amount_fp6: 100_000_000, // 100 USDT
                    reason:     format!("price below buy threshold {buy_below}"),
                },
            },
            Rule {
                name:      format!("{pair} take-profit"),
                priority:  10,
                condition: Condition::PriceAbove {
                    pair:          pair.to_string(),
                    threshold_fp6: sell_above,
                },
                action: ActionTemplate {
                    kind:       ActionKind::Swap {
                        from_token: pair.split('/').next().unwrap_or("ZBX").to_string(),
                        to_token:   "USDT".to_string(),
                    },
                    pair:       pair.to_string(),
                    amount_fp6: 100_000_000,
                    reason:     format!("price above sell threshold {sell_above}"),
                },
            },
            Rule {
                name:      "oracle anomaly alert".to_string(),
                priority:  100,
                condition: Condition::AiClass {
                    model:          ModelId::OracleAnomalyGuard,
                    class:          2,
                    min_confidence: 7000,
                },
                action: ActionTemplate {
                    kind:       ActionKind::Alert {
                        message: "Oracle anomaly detected — possible manipulation".to_string(),
                    },
                    pair:       pair.to_string(),
                    amount_fp6: 0,
                    reason:     "AI anomaly guard triggered".to_string(),
                },
            },
        ];
        Self::new(rules)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oracle::AggregatedPrice;

    fn price_at(pair: &str, fp6: u64) -> AggregatedPrice {
        AggregatedPrice {
            pair:        pair.to_string(),
            median_fp6:  fp6,
            min_fp6:     fp6,
            max_fp6:     fp6,
            num_sources: 1,
            timestamp:   1_700_000_000,
        }
    }

    #[test]
    fn price_above_condition() {
        let cond = Condition::PriceAbove { pair: "ZBX/USDT".to_string(), threshold_fp6: 500_000 };
        assert!(cond.evaluate(&[price_at("ZBX/USDT", 600_000)], &[], 1));
        assert!(!cond.evaluate(&[price_at("ZBX/USDT", 400_000)], &[], 1));
    }

    #[test]
    fn every_n_blocks_fires_on_multiple() {
        let cond = Condition::EveryNBlocks { n: 10 };
        assert!(cond.evaluate(&[], &[], 10));
        assert!(cond.evaluate(&[], &[], 100));
        assert!(!cond.evaluate(&[], &[], 11));
    }

    #[test]
    fn and_condition() {
        let a = Condition::PriceAbove { pair: "ZBX/USDT".to_string(), threshold_fp6: 100 };
        let b = Condition::PriceBelow { pair: "ZBX/USDT".to_string(), threshold_fp6: 900_000 };
        let and = Condition::And(Box::new(a), Box::new(b));
        assert!(and.evaluate(&[price_at("ZBX/USDT", 500_000)], &[], 1));
    }

    #[test]
    fn strategy_fires_correct_rules() {
        let strat = Strategy::standard_defi("ZBX/USDT", 500_000, 2_000_000);
        // Price below buy threshold → buy action
        let actions = strat.evaluate(&[price_at("ZBX/USDT", 400_000)], &[], 1);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0].kind, ActionKind::Swap { to_token, .. } if to_token == "ZBX"));
    }

    #[test]
    fn do_nothing_produces_no_actions() {
        let strat = Strategy::do_nothing();
        let actions = strat.evaluate(&[price_at("ZBX/USDT", 1_000_000)], &[], 1);
        assert!(actions.is_empty());
    }
}
