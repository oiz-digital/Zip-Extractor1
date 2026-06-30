//! zbx-genesis — Zebvix Chain genesis block creation and validation.
//!
//! Responsibilities:
//!   1. Parse `genesis.json` / `mainnet.toml` configuration.
//!   2. Build the genesis state trie from allocations.
//!   3. Produce the genesis block header (block #0).
//!   4. Verify genesis integrity on node startup.

pub mod allocations;
pub mod builder;
pub mod error;
pub mod spec;
pub mod validator;

pub use builder::GenesisBuilder;
pub use error::GenesisError;
pub use spec::{GenesisSpec, ChainConfig, Allocation, TokenPremint};

/// **N-07 fix (S54) — single canonical genesis state-root function.**
///
/// Previously `zbx-genesis::builder`, `zbx-storage::genesis_init`, and
/// `zbx-tools::genesis_dump` each independently computed the genesis
/// `state_root`. Any divergence in encoding logic between the three impls
/// would produce a silent chain-split on startup because the node compares
/// the genesis hash in the DB against the freshly computed one.
///
/// This function is now THE one authoritative implementation; all other call
/// sites must delegate here.  The underlying algorithm is the S30 injective
/// hash (`GenesisBuilder::state_root_bytes`) which commits all account fields
/// (address, balance, nonce, code, storage) with length-prefixed framing.
///
/// Returns the 32-byte genesis state root for `spec`.
///
/// # Errors
/// Propagates `GenesisError` if `spec` fails to parse or contains invalid
/// allocations.
pub fn genesis_state_root(spec: &GenesisSpec) -> Result<zbx_types::H256, GenesisError> {
    GenesisBuilder::new(spec.clone()).state_root_bytes()
}