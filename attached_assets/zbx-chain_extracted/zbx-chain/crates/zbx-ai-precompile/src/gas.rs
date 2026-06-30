//! Gas schedule for all 12 AI models.
//!
//! Gas cost is proportional to: input_size × hidden_size × num_layers.
//! All costs are in ZBX gas units (same scale as EVM opcodes).
//!
//! Governance can update gas costs via ZEP governance vote.

use crate::model::ModelId;

/// Maximum calls per block per contract (rate limit to prevent abuse).
pub const MAX_AI_CALLS_PER_BLOCK: u32 = 10;

pub struct GasSchedule {
    pub spam_classifier:      u64,  // 0x01
    pub risk_scorer:          u64,  // 0x02
    pub nft_tagger:           u64,  // 0x03
    pub zusd_risk_model:      u64,  // 0x04
    pub price_prediction:     u64,  // 0x05
    pub oracle_anomaly_guard: u64,  // 0x06
    pub mev_detector:         u64,  // 0x07
    pub fraud_detector:       u64,  // 0x08
    pub liquidity_analyzer:   u64,  // 0x09
    pub sentiment_classifier: u64,  // 0x0A
    pub gas_optimizer:        u64,  // 0x0B
    pub market_maker:         u64,  // 0x0C
}

impl Default for GasSchedule {
    fn default() -> Self {
        Self {
            // Original 4 (unchanged for backward compat)
            spam_classifier:       500_000,
            risk_scorer:           750_000,
            nft_tagger:          2_000_000,
            zusd_risk_model:       600_000,
            // New 8 (Session 42)
            price_prediction:      800_000,  // OHLCV window inference
            oracle_anomaly_guard:  650_000,  // real-time anomaly detection
            mev_detector:          550_000,  // fast path (time-sensitive)
            fraud_detector:        700_000,  // pattern matching
            liquidity_analyzer:    500_000,  // simple regression
            sentiment_classifier:  450_000,  // lightweight classifier
            gas_optimizer:         400_000,  // very fast model
            market_maker:          900_000,  // multi-output prediction
        }
    }
}

impl GasSchedule {
    pub fn for_model(&self, model: &ModelId) -> u64 {
        match model {
            ModelId::SpamClassifier      => self.spam_classifier,
            ModelId::RiskScorer          => self.risk_scorer,
            ModelId::NftTagger           => self.nft_tagger,
            ModelId::ZusdRiskModel       => self.zusd_risk_model,
            ModelId::PricePrediction     => self.price_prediction,
            ModelId::OracleAnomalyGuard  => self.oracle_anomaly_guard,
            ModelId::MevDetector         => self.mev_detector,
            ModelId::FraudDetector       => self.fraud_detector,
            ModelId::LiquidityAnalyzer   => self.liquidity_analyzer,
            ModelId::SentimentClassifier => self.sentiment_classifier,
            ModelId::GasOptimizer        => self.gas_optimizer,
            ModelId::MarketMaker         => self.market_maker,
        }
    }

    /// Gas cost for ABI encoding overhead (flat).
    pub fn abi_overhead() -> u64 { 5_000 }

    /// Gas cost for DA hash verification (per model).
    pub fn da_verify_cost() -> u64 { 25_000 }

    /// Total cost including overhead.
    pub fn total_cost(&self, model: &ModelId) -> u64 {
        self.for_model(model) + Self::abi_overhead() + Self::da_verify_cost()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_models_have_nonzero_gas() {
        let sched = GasSchedule::default();
        for &id in crate::model::ModelId::all() {
            assert!(sched.for_model(&id) > 0, "model {id:?} has zero gas");
        }
    }

    #[test]
    fn total_cost_exceeds_base() {
        let sched = GasSchedule::default();
        let base = sched.for_model(&ModelId::SpamClassifier);
        let total = sched.total_cost(&ModelId::SpamClassifier);
        assert!(total > base);
    }
}
