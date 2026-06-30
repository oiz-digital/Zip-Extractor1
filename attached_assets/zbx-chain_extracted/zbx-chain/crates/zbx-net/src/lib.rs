//! zbx-net — low-level networking primitives for Zebvix Chain.
//!
//! # ⚠ DEPRECATION NOTICE (M53-02)
//!
//! **This crate (`zbx-net`) is superseded by `zbx-network`.**
//!
//! `zbx-network` provides a unified, production-wired networking stack that
//! consolidates the functionality of `zbx-net`, `zbx-p2p`, and `zbx-gossip`
//! into a single crate with a stable, audited API surface. The node binary
//! (`zbx-node`) has been updated to depend on `zbx-network` only.
//!
//! This crate is **not actively maintained**. It remains compiled for binary
//! compatibility during the transition period and will be removed in the next
//! major release. New code must not depend on it directly.
//!
//! **Migrate to:** `zbx-network` — see `docs/networking/MIGRATION.md`.
//!
//! ---
//!
//! Modules (legacy):
//! - [`discv5`]:        node discovery (UDP).
//! - [`gossip`]:        block/transaction gossip layer.
//! - [`hello`]:         protocol handshake (caps + chain id + head).
//! - [`nat`]:           NAT traversal helpers.
//! - [`peer_manager`]:  peer scoring, ban list, slot allocation.
//! - [`rlpx`]:          RLPx-style framed transport (TCP + Noise).

pub mod discv5;
pub mod gossip;
pub mod hello;
pub mod nat;
pub mod peer_manager;
pub mod rlpx;
