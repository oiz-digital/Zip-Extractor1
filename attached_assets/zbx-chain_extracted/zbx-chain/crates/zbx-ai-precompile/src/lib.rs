//! AI Inference Precompile for ZBX Chain — Session 42 Full Upgrade.
//!
//! ZVM Native Opcode: `0xCA` — AIINFER
//!
//! Enables ZVM smart contracts to request deterministic ML model inference.
//! All 12 models run as INT8 quantized networks — identical output on every
//! validator (no floating point → consensus safe).
//!
//! # Model Suite (12 models)
//!
//! | ID   | Model                  | Gas       | Use case |
//! |:-----|:-----------------------|:----------|:---------|
//! | 0x01 | SpamClassifier         | 525,000   | Token rug-pull / spam detection |
//! | 0x02 | RiskScorer             | 775,000   | DeFi collateral risk (0-100) |
//! | 0x03 | NftTagger              | 2,025,000 | NFT trait generation |
//! | 0x04 | ZusdRiskModel          | 625,000   | ZUSD stability risk |
//! | 0x05 | PricePrediction        | 825,000   | Short-term price direction |
//! | 0x06 | OracleAnomalyGuard     | 675,000   | Oracle manipulation detection |
//! | 0x07 | MevDetector            | 575,000   | MEV sandwich detection |
//! | 0x08 | FraudDetector          | 725,000   | On-chain fraud detection |
//! | 0x09 | LiquidityAnalyzer      | 525,000   | Pool liquidity health |
//! | 0x0A | SentimentClassifier    | 475,000   | On-chain sentiment |
//! | 0x0B | GasOptimizer           | 425,000   | Optimal gas price |
//! | 0x0C | MarketMaker            | 925,000   | Market-making parameters |
//!
//! # Contract ABI (Solidity)
//!
//! ```solidity
//! contract AntiRug {
//!     function isRug(address token) external view returns (bool, uint16) {
//!         bytes memory inp = abi.encode(token);
//!         (bool ok, bytes memory ret) = address(0xCA).staticcall(
//!             abi.encode(uint8(1), inp)
//!         );
//!         require(ok, "AI inference failed");
//!         (bytes memory out, uint16 conf) = abi.decode(ret, (bytes, uint16));
//!         return (out[0] > 128, conf);
//!     }
//! }
//! ```
//!
//! # Security
//!
//! - Weights verified against SHA3-256 DA hash before use
//! - Circuit breaker: model suspended after 5 consecutive errors
//! - Rate limit: max 10 AI calls per block per contract
//! - Input capped at 1024 bytes
//! - Gas metered per model
//! - Post-quantum safe weight updates via Dilithium-3 (ZEP-015)

pub mod model;
pub mod precompile;
pub mod gas;
pub mod error;
pub mod engine;
pub mod abi;
pub mod da;
pub mod weights;

pub use precompile::{AiInferPrecompile, InferResult};
pub use model::{ModelId, ModelRegistry, ModelMeta};
pub use error::AiError;
pub use engine::Int8Network;
pub use da::{DaRef, WeightEntry, ModelHeader};

/// ZVM opcode for AI inference.
pub const OPCODE_AIINFER: u8 = 0xCA;

/// EVM precompile address for AI inference.
pub const PRECOMPILE_ADDRESS: [u8; 20] = [
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0xCA,
];

/// Model weight DA content-addressed prefix.
pub const MODEL_DA_PREFIX: &[u8] = b"zbx:ai:model:v1:";

/// Total number of supported AI models.
pub const MODEL_COUNT: usize = 12;

/// Current AI platform version.
pub const AI_PLATFORM_VERSION: &str = "2.0.0-session42";
