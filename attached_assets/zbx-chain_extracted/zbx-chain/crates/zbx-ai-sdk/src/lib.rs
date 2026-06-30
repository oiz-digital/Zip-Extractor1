//! ZBX Chain AI Agent SDK — Session 42.
//!
//! Enables developers to build autonomous AI agents that run on-chain logic
//! powered by the ZBX AI Inference Precompile (0xCA).
//!
//! # Architecture
//!
//! ```text
//! OracleProvider  ──┐
//!                   ▼
//!              AiAgent.run()
//!                   │
//!         ┌─────────┼─────────┐
//!         ▼         ▼         ▼
//!   AiInferPrecompile  RiskManager  Strategy
//!   (0xCA INT8 models)  (risk gates)  (rule DSL)
//!         │                           │
//!         └──────────┬────────────────┘
//!                    ▼
//!           SessionKeyExecutor  (ZEP-017)
//!                    │
//!                    ▼
//!           On-chain transactions
//! ```
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use zbx_ai_sdk::{
//!     agent::{AiAgent, AgentConfig},
//!     oracle::StubOracleProvider,
//!     risk::RiskLevel,
//!     strategy::Strategy,
//! };
//!
//! let config = AgentConfig::new(
//!     "my-defi-agent".to_string(),
//!     [0u8; 20],
//!     vec!["ZBX/USDT".to_string()],
//!     RiskLevel::Medium,
//! );
//! let strategy = Strategy::standard_defi("ZBX/USDT", 800_000, 1_500_000);
//! let mut agent = AiAgent::new(config, strategy);
//! let oracle = StubOracleProvider::new(1_700_000_000);
//! let result = agent.run(&oracle, 100_000).unwrap();
//! println!("Actions: {}", result.actions.len());
//! ```
//!
//! # Security
//!
//! - All AI inference is deterministic (INT8 quantized, no floats)
//! - Session keys are scope-limited + value-capped (ZEP-017)
//! - Risk manager gates all actions — Critical risk = full stop
//! - Oracle aggregation uses median across N sources (manipulation resistant)
//! - Emergency pause available via `agent.pause()`

pub mod agent;
pub mod oracle;
pub mod executor;
pub mod strategy;
pub mod risk;
pub mod error;

pub use agent::{AiAgent, AgentConfig, AgentRunResult, AiSignal};
pub use oracle::{OracleProvider, StubOracleProvider, AggregatedPrice, PriceObservation};
pub use executor::{SessionKeyExecutor, SessionKey, ActionRequest, ActionReceipt};
pub use strategy::{Strategy, Rule, Condition, StrategyAction, ActionKind};
pub use risk::{RiskManager, RiskLevel, PortfolioPosition, RiskParams};
pub use error::SdkError;

/// SDK version.
pub const SDK_VERSION: &str = "1.0.0-session42";
