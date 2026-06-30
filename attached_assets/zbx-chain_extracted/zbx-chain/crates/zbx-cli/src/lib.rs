//! zbx-cli — command-line interface library for Zebvix Chain.
//!
//! Re-exports the modules the `zbxctl` binary builds against. Kept narrow on
//! purpose: only modules that actually exist as files on disk are listed
//! here so the library and binary halves of the crate cannot drift.

pub mod config;
pub mod output;
pub mod rpc;
pub mod safety;

pub mod wallet;
pub mod contract;
pub mod defi;
pub mod governance;
pub mod stake;
