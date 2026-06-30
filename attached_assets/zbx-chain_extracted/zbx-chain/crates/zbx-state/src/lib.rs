//! zbx-state: World-state management for Zebvix.
//!
//! Provides a cached view over the persistent ZbxDb that tracks dirty
//! accounts, caches code, and computes the state trie root after block execution.

pub mod account;
pub mod error;
pub mod mpt;
pub mod host_zvm;
pub mod snapshot;
pub mod state_db;
pub mod trie;
pub mod trie_adapter;

pub use account::{AccountInfo, AccountDiff, GenesisAccount, account_trie_key, encode_account_rlp};
pub use error::StateError;
pub use state_db::StateDB;
pub use trie_adapter::ZbxDbTrieAdapter;