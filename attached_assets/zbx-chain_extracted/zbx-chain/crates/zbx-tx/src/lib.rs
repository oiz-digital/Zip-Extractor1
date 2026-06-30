//! zbx-tx — Zebvix Chain transaction types.
//!
//! Supported transaction types:
//!   Type 0: Legacy (pre-EIP-1559)
//!   Type 1: EIP-2930 (access lists)
//!   Type 2: EIP-1559 (max fee + priority fee)  ← default
//!
//! Native gas tokens: ZBX (default), ZUSD.
//! Set `transaction.gas_token` to pay fees in either token.

pub mod types;
pub mod validation;
pub mod signer;
pub mod signing;
pub mod error;
pub mod gas;

pub use types::{Transaction, TxType, SignedTx, GasToken};
pub use validation::TxValidator;
pub use signer::TxSigner;
pub use signing::{
    SigningContext, BatchSignRequest, BatchSignResult,
    recover_signer, verify_sender,
    CHAIN_ID_MAINNET, CHAIN_ID_TESTNET,
};
pub use error::TxError;
pub use gas::{GasFeeInfo, FeeDeduction, ZUSD_GENESIS_ADDR};