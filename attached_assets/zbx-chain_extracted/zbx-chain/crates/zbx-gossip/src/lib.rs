//! zbx-gossip — GossipSub protocol for efficient transaction propagation.
//!
//! # ⚠ DEPRECATION NOTICE (M53-02)
//!
//! **This crate (`zbx-gossip`) is superseded by `zbx-network`.**
//!
//! `zbx-network` provides a unified networking stack that consolidates
//! `zbx-net`, `zbx-p2p`, and `zbx-gossip`. The node binary (`zbx-node`)
//! has been updated to depend only on `zbx-network`.
//!
//! This crate is **not actively maintained** and will be removed in the next
//! major release. New code must not depend on it directly.
//!
//! **Migrate to:** `zbx-network` — see `docs/networking/MIGRATION.md`.
//!
//! ---
//!
//! Implements a simplified GossipSub v1.1 protocol (legacy):
//! - D (degree): 8 outbound mesh peers
//! - D_low: 6, D_high: 12
//! - Heartbeat: 1 second
//! - Fanout TTL: 60 seconds
//! - Gossip factor: 0.25
//!
//! Message deduplication via a 128-entry sliding LRU cache (xxHash).
//! Peer scoring: penalizes peers that send duplicate/invalid messages.

pub mod config;
pub mod topic;
pub mod peer_score;
pub mod message_cache;
pub mod manager;

pub use config::GossipConfig;
pub use topic::Topic;
pub use manager::{GossipManager, GossipEvent};
