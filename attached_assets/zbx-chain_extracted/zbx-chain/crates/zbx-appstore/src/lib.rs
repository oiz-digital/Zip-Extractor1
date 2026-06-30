//! zbx-appstore — Zebvix On-Chain App Store Registry
//!
//! Provides a production-ready decentralized application registry where
//! developers can publish, version, and manage dApps on Zebvix Chain.
//!
//! # Architecture
//!
//! ```text
//! Developer ──publish()──► AppRegistry ──► RocksDB store
//!                               │
//!                          version mgmt
//!                          category index
//!                          rating system
//!                          install tracking
//! ```
//!
//! # Supported App Categories
//!
//! - Wallet, DeFi, NFT, AI Tools, Games, Utilities
//!
//! # Storage layout
//!
//! ```text
//! apps/{slug}              → AppMetadata (JSON)
//! apps/{slug}/versions/{v} → VersionRecord (JSON)
//! categories/{cat}/{slug}  → "" (index)
//! ratings/{slug}/{addr}    → Rating (JSON)
//! installs/{slug}          → u64 (little-endian count)
//! ```

pub mod category;
pub mod error;
pub mod install;
pub mod metadata;
pub mod rating;
pub mod registry;
pub mod version;

pub use category::AppCategory;
pub use error::AppStoreError;
pub use install::InstallTracker;
pub use metadata::{AppMetadata, AppStatus, ContactInfo};
pub use rating::{RatingRecord, RatingSummary};
pub use registry::AppRegistry;
pub use version::{VersionRecord, VersionStatus};
