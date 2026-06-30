//! zbx-payment — Crypto payment gateway for Zebvix Chain.
//!
//! Off-chain Rust layer for the ZbxPaymentGateway.sol contract:
//!   - Merchant registry and state caching
//!   - Invoice lifecycle management
//!   - Multi-token price conversion via oracle
//!   - Webhook event builder for off-chain integrations

pub mod merchant;
pub mod invoice;
pub mod webhook;
pub mod converter;
