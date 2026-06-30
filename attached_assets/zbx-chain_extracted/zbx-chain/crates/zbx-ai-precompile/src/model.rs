//! Model registry — 12 production AI models for ZBX Chain.

use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use crate::error::AiError;

/// All 12 supported AI models (ZEP-009, Session 42).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ModelId {
    // --- Phase 1: Original 4 ---
    SpamClassifier     = 0x01, // Token spam / rug-pull detection
    RiskScorer         = 0x02, // DeFi collateral risk score (0–100)
    NftTagger          = 0x03, // NFT trait tagging / metadata
    ZusdRiskModel      = 0x04, // ZUSD stability risk scoring
    // --- Phase 1 Extension: New 8 ---
    PricePrediction    = 0x05, // Short-term price direction (up/down/neutral)
    OracleAnomalyGuard = 0x06, // Oracle manipulation / anomaly detection
    MevDetector        = 0x07, // MEV sandwich / frontrun detection
    FraudDetector      = 0x08, // On-chain fraud / wash-trading detection
    LiquidityAnalyzer  = 0x09, // Pool liquidity health scoring
    SentimentClassifier= 0x0A, // On-chain sentiment (bullish/bearish/neutral)
    GasOptimizer       = 0x0B, // Optimal gas price prediction
    MarketMaker        = 0x0C, // Automated market-making parameter suggestion
}

impl ModelId {
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x01 => Some(Self::SpamClassifier),
            0x02 => Some(Self::RiskScorer),
            0x03 => Some(Self::NftTagger),
            0x04 => Some(Self::ZusdRiskModel),
            0x05 => Some(Self::PricePrediction),
            0x06 => Some(Self::OracleAnomalyGuard),
            0x07 => Some(Self::MevDetector),
            0x08 => Some(Self::FraudDetector),
            0x09 => Some(Self::LiquidityAnalyzer),
            0x0A => Some(Self::SentimentClassifier),
            0x0B => Some(Self::GasOptimizer),
            0x0C => Some(Self::MarketMaker),
            _    => None,
        }
    }

    /// All 12 model IDs.
    pub fn all() -> &'static [ModelId] {
        &[
            Self::SpamClassifier, Self::RiskScorer, Self::NftTagger,
            Self::ZusdRiskModel, Self::PricePrediction, Self::OracleAnomalyGuard,
            Self::MevDetector, Self::FraudDetector, Self::LiquidityAnalyzer,
            Self::SentimentClassifier, Self::GasOptimizer, Self::MarketMaker,
        ]
    }

    /// Human-readable name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::SpamClassifier      => "spam-classifier-v2",
            Self::RiskScorer          => "risk-scorer-v2",
            Self::NftTagger           => "nft-tagger-v1",
            Self::ZusdRiskModel       => "zusd-risk-v2",
            Self::PricePrediction     => "price-prediction-v1",
            Self::OracleAnomalyGuard  => "oracle-anomaly-guard-v1",
            Self::MevDetector         => "mev-detector-v1",
            Self::FraudDetector       => "fraud-detector-v1",
            Self::LiquidityAnalyzer   => "liquidity-analyzer-v1",
            Self::SentimentClassifier => "sentiment-classifier-v1",
            Self::GasOptimizer        => "gas-optimizer-v1",
            Self::MarketMaker         => "market-maker-v1",
        }
    }

    /// Expected input size in bytes.
    pub fn input_size(&self) -> usize {
        match self {
            Self::SpamClassifier      => 32,   // token address hash
            Self::RiskScorer          => 64,   // collateral + loan data
            Self::NftTagger           => 128,  // metadata blob
            Self::ZusdRiskModel       => 64,   // oracle prices + supply stats
            Self::PricePrediction     => 96,   // OHLCV window (8 × 12 bytes)
            Self::OracleAnomalyGuard  => 80,   // price + volatility + sources
            Self::MevDetector         => 48,   // tx data + mempool snapshot
            Self::FraudDetector       => 64,   // trading pattern features
            Self::LiquidityAnalyzer   => 48,   // reserves + volume + fees
            Self::SentimentClassifier => 32,   // on-chain signal vector
            Self::GasOptimizer        => 32,   // recent gas prices + block load
            Self::MarketMaker         => 64,   // spread + depth + volatility
        }
    }

    /// Expected output size in bytes.
    pub fn output_size(&self) -> usize {
        match self {
            Self::NftTagger    => 32,
            Self::MarketMaker  => 8,
            _                  => 4,
        }
    }

    /// Number of output classes.
    pub fn num_classes(&self) -> usize {
        match self {
            Self::SpamClassifier      => 2,  // clean / spam
            Self::RiskScorer          => 5,  // 0-20/20-40/40-60/60-80/80-100
            Self::NftTagger           => 16, // 16 trait categories
            Self::ZusdRiskModel       => 4,  // safe/caution/alert/critical
            Self::PricePrediction     => 3,  // up/neutral/down
            Self::OracleAnomalyGuard  => 4,  // normal/suspicious/attack/emergency
            Self::MevDetector         => 3,  // clean/possible-mev/mev
            Self::FraudDetector       => 3,  // clean/suspicious/fraud
            Self::LiquidityAnalyzer   => 4,  // healthy/thin/critical/empty
            Self::SentimentClassifier => 3,  // bullish/neutral/bearish
            Self::GasOptimizer        => 5,  // slow/standard/fast/rapid/instant
            Self::MarketMaker         => 4,  // tight/normal/wide/halt
        }
    }

    /// Hidden layer size for stub network.
    pub fn hidden_size(&self) -> usize {
        match self {
            Self::NftTagger    => 64,
            Self::MarketMaker  => 32,
            _                  => 16,
        }
    }
}

/// Metadata for a registered model.
#[derive(Debug, Clone)]
pub struct ModelMeta {
    pub id:          ModelId,
    pub name:        &'static str,
    pub input_size:  usize,
    pub output_size: usize,
    pub num_classes: usize,
    pub hidden_size: usize,
    /// SHA3-256 of model weights on DA layer (zeros = stub).
    pub da_hash:     [u8; 32],
    /// Minimum validators required to run this model.
    pub min_validators: u8,
}

impl ModelMeta {
    pub fn stub(id: ModelId) -> Self {
        Self {
            name:           id.name(),
            input_size:     id.input_size(),
            output_size:    id.output_size(),
            num_classes:    id.num_classes(),
            hidden_size:    id.hidden_size(),
            da_hash:        [0u8; 32],
            min_validators: 1,
            id,
        }
    }
}

/// Registry of all available models.
pub struct ModelRegistry {
    models: HashMap<ModelId, ModelMeta>,
}

impl ModelRegistry {
    pub fn new() -> Self { Self { models: HashMap::new() } }

    /// Register all 12 models with stub weights.
    pub fn with_stubs() -> Self {
        let mut r = Self::new();
        for &id in ModelId::all() {
            r.register(ModelMeta::stub(id));
        }
        r
    }

    pub fn register(&mut self, meta: ModelMeta) {
        self.models.insert(meta.id, meta);
    }

    pub fn has(&self, id: &ModelId) -> bool { self.models.contains_key(id) }

    pub fn get(&self, id: &ModelId) -> Option<&ModelMeta> { self.models.get(id) }

    pub fn count(&self) -> usize { self.models.len() }

    pub fn validate_input(&self, id: &ModelId, input: &[u8]) -> Result<(), AiError> {
        if let Some(meta) = self.models.get(id) {
            if input.len() > 1024 {
                return Err(AiError::InputTooLarge(input.len()));
            }
            if !input.is_empty() && input.len() < meta.input_size / 2 {
                return Err(AiError::InputSizeMismatch {
                    expected: meta.input_size,
                    got: input.len(),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_12_models_registered() {
        let reg = ModelRegistry::with_stubs();
        assert_eq!(reg.count(), 12);
    }

    #[test]
    fn all_model_ids_round_trip() {
        for &id in ModelId::all() {
            let byte = id as u8;
            assert_eq!(ModelId::from_byte(byte), Some(id));
        }
    }

    #[test]
    fn unknown_byte_returns_none() {
        assert!(ModelId::from_byte(0xFF).is_none());
        assert!(ModelId::from_byte(0x00).is_none());
        assert!(ModelId::from_byte(0x0D).is_none());
    }

    #[test]
    fn all_models_have_valid_sizes() {
        for &id in ModelId::all() {
            assert!(id.input_size() > 0);
            assert!(id.output_size() > 0);
            assert!(id.num_classes() >= 2);
            assert!(id.hidden_size() >= 8);
        }
    }
}
