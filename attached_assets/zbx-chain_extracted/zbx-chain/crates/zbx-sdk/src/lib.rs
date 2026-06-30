#![recursion_limit = "256"]
//! # zbx-sdk — Zebvix Chain Developer SDK
//!
//! Full-featured Rust client library for building applications on Zebvix Chain.
//!
//! ## Features
//!
//! | Feature   | Description                               |
//! |-----------|-------------------------------------------|
//! | provider  | HTTP JSON-RPC client with middleware       |
//! | wallet    | secp256k1 signing, address derivation     |
//! | contract  | ABI encoding, deploy, call, send          |
//! | ws        | WebSocket event subscriptions             |
//! | hd        | BIP32/BIP44 HD wallet key derivation      |
//!
//! ## Quick Example
//!
//! ```rust,no_run
//! use zbx_sdk::prelude::*;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let provider = Provider::http("https://rpc.zebvix.com").await?;
//!     let wallet   = Wallet::from_private_key(
//!         "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
//!     )?;
//!
//!     let balance = provider.get_balance(wallet.address()).await?;
//!     println!("Balance: {}", format_zbx(balance));
//!
//!     let receipt = provider
//!         .send(
//!             TransactionRequest::pay("0xRecipient", parse_zbx("1.5").unwrap()),
//!             &wallet,
//!         )
//!         .await?
//!         .wait_confirmations(3)
//!         .await?;
//!
//!     println!("Confirmed in block #{}", receipt.block_number.unwrap());
//!     Ok(())
//! }
//! ```

#![warn(missing_docs, unreachable_pub)]
#![deny(unsafe_code)]

pub mod error;
pub mod provider;
pub mod signer;
pub mod wallet;
pub mod transaction;
pub mod contract;
pub mod abi;
pub mod events;
pub mod batch;
pub mod multicall;
pub mod filter;
pub mod gas;
pub mod middleware;
pub mod types;
pub mod utils;

#[cfg(feature = "hd")]
pub mod hd_wallet;

#[cfg(feature = "ws")]
pub mod ws;

/// Convenience re-exports for common SDK use.
pub mod prelude {
    pub use crate::provider::Provider;
    pub use crate::wallet::Wallet;
    pub use crate::contract::Contract;
    pub use crate::transaction::TransactionRequest;
    pub use crate::filter::FilterBuilder;
    pub use crate::gas::GasOracle;
    pub use crate::multicall::Multicall;
    pub use crate::utils::{format_zbx, parse_zbx, to_checksum};
    pub use crate::error::SdkError;
    pub use crate::types::mainnet;
    pub use zbx_types::{Address, U256, H256};
}

pub use prelude::*;