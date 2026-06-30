//! zbx-wallet — Wallet creation, import, and signing for ZBX Chain.
//!
//! ## Modules
//!
//! | Module        | Responsibility                                          |
//! |---------------|---------------------------------------------------------|
//! | `mnemonic`    | BIP-39: generate/validate/seed-derive mnemonics         |
//! | `hd`          | BIP-32/44: HD key derivation with full chain-code       |
//! | `signer`      | secp256k1 ECDSA: EIP-155/191/712 signing, EIP-55 addr  |
//! | `keystore`    | Ethereum v3 keystore: scrypt + AES-128-CTR              |
//! | `multisig`    | M-of-N threshold multisig wallet                        |
//! | `watch`       | Watch-only wallet (no private key)                      |
//! | `pq_wallet`   | Post-quantum hybrid (ECDSA + Dilithium-3, ZEP-015)      |
//! | `eip712`      | EIP-712 typed data hashing                              |
//! | `create_import` | High-level wallet create/import API                   |
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use zbx_wallet::{create_wallet, import_wallet_from_mnemonic, Wallet};
//!
//! // Create a new 24-word wallet
//! let wallet = create_wallet(24).expect("wallet generation failed");
//! println!("Address: {}", wallet.checksum_address);
//! println!("Mnemonic: {}", wallet.mnemonic.as_ref().unwrap());
//!
//! // Import from mnemonic
//! let imported = import_wallet_from_mnemonic(
//!     "abandon abandon abandon abandon abandon abandon \
//!      abandon abandon abandon abandon abandon about",
//!     0, // account index
//! ).expect("import failed");
//! ```

pub mod create_import;
pub mod eip712;
pub mod hd;
pub mod keystore;
pub mod mnemonic;
pub mod multisig;
pub mod pq_wallet;
pub mod signer;
pub mod watch;

// ── Primary re-exports ────────────────────────────────────────────────────────

pub use create_import::{
    ZbxWallet as Wallet,
    WalletError,
    KeystoreFile,
    KeystoreCrypto,
    ScryptParams,
    create_wallet,
    import_wallet_from_mnemonic,
    import_wallet_from_key,
    import_wallet_from_keystore,
    export_keystore,
    ZBX_DERIVATION_PATH,
    ZBX_COIN_TYPE,
    MNEMONIC_WORDS_12,
    MNEMONIC_WORDS_24,
};

pub use multisig::{MultiSigWallet, Proposal, MultiSigError};
pub use watch::{WatchWallet, UnsignedTx, WatchError};
pub use pq_wallet::{PqWallet, HybridSignedTx, PqWalletError};
pub use eip712::{TypedData, SolidityValue, zbx_domain_separator, hash_struct};
