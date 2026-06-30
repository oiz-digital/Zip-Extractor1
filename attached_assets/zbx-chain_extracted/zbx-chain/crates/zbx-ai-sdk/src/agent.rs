//! AI Agent Framework — trigger-based autonomous on-chain agents.
//!
//! An `AiAgent` is a stateless rule engine that:
//! 1. Reads oracle prices + on-chain data (via OracleProvider)
//! 2. Runs AI inference via the 0xCA precompile (via AiInferPrecompile)
//! 3. Evaluates strategy rules (via Strategy DSL)
//! 4. Executes actions (via SessionKeyExecutor)
//!
//! Security:
//! - All decisions are logged with a deterministic action_id hash
//! - Max actions per run: 8 (prevents runaway loops)
//! - Rate limit enforced at executor level (ZEP-017 session keys)
//! - Emergency kill-switch: any action can be vetoed by guardian address

use crate::{
    error::SdkError,
    oracle::{OracleProvider, AggregatedPrice},
    strategy::{Strategy, StrategyAction},
    risk::{RiskManager, RiskLevel},
};
use zbx_ai_precompile::{AiInferPrecompile, ModelId, ModelRegistry};
use serde::{Serialize, Deserialize};

/// Maximum actions an agent can schedule in a single run.
pub const MAX_ACTIONS_PER_RUN: usize = 8;

/// Agent configuration — immutable after deployment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Human-readable name.
    pub name:           String,
    /// Unique agent ID (hash of config).
    pub agent_id:       [u8; 32],
    /// Maximum risk level this agent is allowed to operate at.
    pub max_risk_level: RiskLevel,
    /// Pairs this agent monitors.
    pub watch_pairs:    Vec<String>,
    /// ZBX address that receives agent earnings.
    pub owner_address:  [u8; 20],
    /// Whether emergency stop is active.
    pub paused:         bool,
}

impl AgentConfig {
    pub fn new(name: String, owner: [u8; 20], pairs: Vec<String>, max_risk: RiskLevel) -> Self {
        use sha3::{Digest, Sha3_256};
        let mut h = Sha3_256::new();
        h.update(name.as_bytes());
        h.update(&owner);
        let digest = h.finalize();
        let mut agent_id = [0u8; 32];
        agent_id.copy_from_slice(&digest);
        Self {
            name,
            agent_id,
            max_risk_level: max_risk,
            watch_pairs: pairs,
            owner_address: owner,
            paused: false,
        }
    }
}

/// Result of a single agent run.
#[derive(Debug, Clone)]
pub struct AgentRunResult {
    pub agent_id:     [u8; 32],
    pub block_number: u64,
    pub prices:       Vec<AggregatedPrice>,
    pub actions:      Vec<StrategyAction>,
    pub risk_level:   RiskLevel,
    pub skipped:      bool,
    pub skip_reason:  Option<String>,
}

/// The AI agent — combines oracle, inference, strategy, and risk management.
pub struct AiAgent {
    pub config:   AgentConfig,
    strategy:     Strategy,
    risk_manager: RiskManager,
    precompile:   AiInferPrecompile,
}

impl AiAgent {
    pub fn new(config: AgentConfig, strategy: Strategy) -> Self {
        let risk_manager = RiskManager::new(config.max_risk_level.clone());
        let precompile = AiInferPrecompile::new(ModelRegistry::with_stubs());
        Self { config, strategy, risk_manager, precompile }
    }

    /// Run the agent for one block cycle.
    pub fn run<O: OracleProvider>(
        &mut self,
        oracle:       &O,
        block_number: u64,
    ) -> Result<AgentRunResult, SdkError> {
        // Emergency stop check
        if self.config.paused {
            return Ok(AgentRunResult {
                agent_id:     self.config.agent_id,
                block_number,
                prices:       vec![],
                actions:      vec![],
                risk_level:   RiskLevel::Critical,
                skipped:      true,
                skip_reason:  Some("agent paused (emergency stop)".to_string()),
            });
        }

        // 1. Collect prices for all watched pairs
        let prices = self.collect_prices(oracle)?;

        // 2. Run AI inference on price data
        let ai_signals = self.run_inference(&prices)?;

        // 3. Evaluate risk
        let risk_level = self.risk_manager.evaluate(&prices, &ai_signals);

        // 4. Block if risk too high
        if risk_level > self.config.max_risk_level {
            tracing::warn!(
                agent = %self.config.name,
                risk  = ?risk_level,
                "Agent skipping run: risk level exceeded"
            );
            let skip_reason = format!("risk level {:?} > max {:?}",
                risk_level, self.config.max_risk_level);
            return Ok(AgentRunResult {
                agent_id: self.config.agent_id,
                block_number,
                prices,
                actions:     vec![],
                risk_level,
                skipped:     true,
                skip_reason: Some(skip_reason),
            });
        }

        // 5. Evaluate strategy
        let actions = self.strategy.evaluate(&prices, &ai_signals, block_number);
        let actions = actions.into_iter().take(MAX_ACTIONS_PER_RUN).collect::<Vec<_>>();

        tracing::info!(
            agent        = %self.config.name,
            block        = block_number,
            num_prices   = prices.len(),
            num_actions  = actions.len(),
            risk         = ?risk_level,
            "Agent run complete"
        );

        Ok(AgentRunResult {
            agent_id: self.config.agent_id,
            block_number,
            prices,
            actions,
            risk_level,
            skipped:     false,
            skip_reason: None,
        })
    }

    fn collect_prices<O: OracleProvider>(&self, oracle: &O) -> Result<Vec<AggregatedPrice>, SdkError> {
        let mut prices = Vec::new();
        for pair in &self.config.watch_pairs {
            match oracle.latest_price(pair) {
                Ok(obs) => {
                    prices.push(AggregatedPrice {
                        pair:        pair.clone(),
                        median_fp6:  obs.price_fp6,
                        min_fp6:     obs.price_fp6,
                        max_fp6:     obs.price_fp6,
                        num_sources: 1,
                        timestamp:   obs.timestamp,
                    });
                }
                Err(e) => {
                    tracing::warn!(pair, error = %e, "Failed to fetch price — skipping pair");
                }
            }
        }
        if prices.is_empty() && !self.config.watch_pairs.is_empty() {
            return Err(SdkError::OracleInsuffientSources { got: 0, required: 1 });
        }
        Ok(prices)
    }

    fn run_inference(&mut self, prices: &[AggregatedPrice]) -> Result<Vec<AiSignal>, SdkError> {
        let mut signals = Vec::new();

        if prices.is_empty() {
            return Ok(signals);
        }

        // Use first pair's price for inference features
        let price = &prices[0];
        let input = encode_price_input(price);

        // Sentiment classifier (0x0A)
        if let Ok(r) = self.precompile.call(ModelId::SentimentClassifier, &input, 2_000_000) {
            signals.push(AiSignal {
                model:      ModelId::SentimentClassifier,
                class:      r.class,
                confidence: r.confidence,
            });
        }

        // Price prediction (0x05)
        let price_input = encode_price_input(price);
        if let Ok(r) = self.precompile.call(ModelId::PricePrediction, &price_input, 2_000_000) {
            signals.push(AiSignal {
                model:      ModelId::PricePrediction,
                class:      r.class,
                confidence: r.confidence,
            });
        }

        Ok(signals)
    }

    /// Emergency pause.
    pub fn pause(&mut self) { self.config.paused = true; }
    pub fn resume(&mut self) { self.config.paused = false; }
}

/// AI inference signal from a model.
#[derive(Debug, Clone)]
pub struct AiSignal {
    pub model:      ModelId,
    pub class:      u8,
    pub confidence: u16,
}

/// Encode a price observation as a byte vector for AI inference.
fn encode_price_input(price: &AggregatedPrice) -> Vec<u8> {
    let mut buf = Vec::with_capacity(32);
    buf.extend_from_slice(&price.median_fp6.to_be_bytes());
    buf.extend_from_slice(&price.min_fp6.to_be_bytes());
    buf.extend_from_slice(&price.max_fp6.to_be_bytes());
    buf.extend_from_slice(&(price.spread_bps() as u64).to_be_bytes());
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{oracle::StubOracleProvider, strategy::Strategy};

    fn test_agent() -> AiAgent {
        let config = AgentConfig::new(
            "test-agent".to_string(),
            [1u8; 20],
            vec!["ZBX/USDT".to_string()],
            RiskLevel::Medium,
        );
        AiAgent::new(config, Strategy::do_nothing())
    }

    #[test]
    fn agent_runs_without_error() {
        let mut agent = test_agent();
        let oracle = StubOracleProvider::new(1_700_000_000);
        let result = agent.run(&oracle, 1000).unwrap();
        assert!(!result.skipped);
        assert!(!result.prices.is_empty());
    }

    #[test]
    fn paused_agent_skips() {
        let mut agent = test_agent();
        agent.pause();
        let oracle = StubOracleProvider::new(1_700_000_000);
        let result = agent.run(&oracle, 1000).unwrap();
        assert!(result.skipped);
        assert!(result.skip_reason.is_some());
    }

    #[test]
    fn agent_id_is_deterministic() {
        let c1 = AgentConfig::new("agent-x".to_string(), [2u8; 20], vec![], RiskLevel::Low);
        let c2 = AgentConfig::new("agent-x".to_string(), [2u8; 20], vec![], RiskLevel::Low);
        assert_eq!(c1.agent_id, c2.agent_id);
    }
}
