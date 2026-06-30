//! zbx-yield — yield farming, gauge-weighted reward allocation,
//! and merkle-based reward distribution for Zebvix Chain.
//!
//! ## Modules
//! * `farm`        — LP-token staking with per-block ZBX emission
//! * `gauge`       — vote-weighted gauge controller (ve-token style, epoch-based)
//! * `distributor` — off-chain merkle reward roots + linear vesting on-chain claims

pub mod farm;
pub mod gauge;
pub mod distributor;
