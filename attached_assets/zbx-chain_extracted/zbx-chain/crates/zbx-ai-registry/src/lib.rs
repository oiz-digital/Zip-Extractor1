//! ZBX Chain AI Model Registry — Session 42.
//!
//! Provides the on-chain infrastructure for deploying, billing, proving, and
//! governing AI models that run via the 0xCA AIINFER precompile (ZEP-009).
//!
//! # Architecture
//!
//! ```text
//!                  ZBX DAO Governance
//!                        │
//!              ┌─────────▼──────────┐
//!              │  GovernanceSystem  │
//!              │  (vote + veto)     │
//!              └─────────┬──────────┘
//!                        │ approve/suspend
//!              ┌─────────▼──────────┐
//!              │   ModelRegistry    │◄── Publisher submits model
//!              │  (12 models, v2+)  │
//!              └────┬──────────┬────┘
//!                   │          │
//!          ┌────────▼──┐  ┌───▼──────────┐
//!          │  Billing  │  │ InferenceProof│
//!          │  System   │  │ (Merkle ZK)   │
//!          │ (ZBX fee) │  └──────────────┘
//!          └───────────┘
//! ```
//!
//! # Model Lifecycle
//!
//! ```text
//! Pending → Active → Deprecated → Removed
//!         ↘ Suspended (security emergency)
//! ```
//!
//! # Fee Split
//!
//! Every inference call splits the ZBX token fee:
//! - 60% → model publisher
//! - 25% → ZBX DAO treasury
//! - 15% → validator reward pool
//!
//! # Proof System
//!
//! Every inference produces a Merkle commitment:
//! leaf = SHA3(input_hash || weights_hash || output_hash || block || model_id)
//! Batches of leaves form a Merkle tree; the root is committed to the chain.

pub mod registry;
pub mod payment;
pub mod proof;
pub mod governance;
pub mod error;

pub use registry::{ModelRegistry, ModelEntry, ModelStatus, ModelTier};
pub use payment::{BillingSystem, FeeSchedule, FeeSplit, InferenceBilling, AccountBalance};
pub use proof::{InferenceProof, InferenceCommitment, ProofBatch};
pub use governance::{GovernanceSystem, Proposal, GovernanceAction, ProposalStatus, Vote};
pub use error::RegistryError;

/// Registry version.
pub const REGISTRY_VERSION: &str = "1.0.0-session42";

/// Maximum models in the registry.
pub const MAX_REGISTRY_MODELS: usize = 256;

/// Revenue split constants (basis points).
pub const PUBLISHER_SHARE_BPS: u32  = 6_000;
pub const TREASURY_SHARE_BPS: u32   = 2_500;
pub const VALIDATOR_SHARE_BPS: u32  = 1_500;
