//! zbx-gaming — Gaming framework for Zebvix Chain.
//!
//! Provides Rust-native representations and precompile glue for:
//!   - VRF (Verifiable Random Function) commit-reveal scheme
//!   - Game session escrow (stake → play → payout)
//!   - On-chain leaderboard with ERC-20 reward distribution
//!   - ERC-1155 game item state management

pub mod vrf;
pub mod escrow;
pub mod leaderboard;
pub mod items;
