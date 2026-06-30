//! zbx-keystore — Ethereum-compatible encrypted keystore.
//!
//! Stores private keys in the Ethereum Keystore v3 (EIP-55) format:
//! ```json
//! {
//!   "version": 3,
//!   "id": "UUID",
//!   "address": "0xABCD...",
//!   "crypto": {
//!     "cipher": "aes-128-ctr",
//!     "cipherparams": { "iv": "..." },
//!     "ciphertext": "...",
//!     "kdf": "scrypt",
//!     "kdfparams": { "n": 262144, "r": 8, "p": 1, ... },
//!     "mac": "..."
//!   }
//! }
//! ```
//!
//! Compatible with MetaMask, geth, zbx CLI, and hardware wallets.

pub mod error;
pub mod keyfile;
pub mod manager;
pub mod secure_write;
pub mod wallet;

pub use error::KeystoreError;
pub use keyfile::{KeyFile, CryptoParams};
pub use manager::KeystoreManager;
pub use secure_write::{
    ensure_strict_perms, secure_write, secure_write_follow_symlinks, tighten_dir,
    KEYFILE_PERM_MASK,
};
pub use wallet::KeystoreWallet;