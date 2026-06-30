//! zbx-explorer — HTTP API backend for the Zebvix block explorer.
//!
//! Serves block, transaction, address, and token data consumed by
//! the frontend explorer UI at <https://explorer.zebvix.io>.

pub mod api;
pub mod indexer;
pub mod search;
pub mod ws;