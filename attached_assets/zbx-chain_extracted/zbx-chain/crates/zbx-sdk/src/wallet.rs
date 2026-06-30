//! Wallet: private key management, message signing, and transaction signing.

use crate::{
    error::SdkError,
    signer::{KeyPair, keccak256},
    transaction::{TransactionRequest, SignedTransaction},
};
use zbx_types::{Address, H256, U256};
use std::fmt;

/// A Zebvix wallet backed by a secp256k1 private key.
pub struct Wallet {
    keypair:  KeyPair,
    chain_id: u64,
}

impl Wallet {
    // ── Constructors ─────────────────────────────────────────────────────────

    /// Create a wallet from a hex-encoded private key (with or without 0x prefix).
    pub fn from_private_key(hex: &str) -> Result<Self, SdkError> {
        let clean = hex.trim_start_matches("0x");
        let bytes  = hex::decode(clean).map_err(SdkError::Hex)?;
        if bytes.len() != 32 {
            return Err(SdkError::InvalidKey("key must be exactly 32 bytes".into()));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Self { keypair: KeyPair::from_bytes(&arr)?, chain_id: zbx_types::CHAIN_ID_MAINNET })
    }

    /// Create a wallet from raw bytes. Defaults to mainnet (8989); use
    /// `with_chain_id()` to target testnet (8990).
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, SdkError> {
        Ok(Self { keypair: KeyPair::from_bytes(bytes)?, chain_id: zbx_types::CHAIN_ID_MAINNET })
    }

    /// Generate a random wallet (mainnet by default).
    pub fn random() -> Self {
        Self { keypair: KeyPair::random(), chain_id: zbx_types::CHAIN_ID_MAINNET }
    }

    /// Set the chain ID used for EIP-155 transaction signing.
    pub fn with_chain_id(mut self, id: u64) -> Self {
        self.chain_id = id; self
    }

    // ── Getters ──────────────────────────────────────────────────────────────

    pub fn address(&self)  -> Address { self.keypair.address }
    pub fn chain_id(&self) -> u64     { self.chain_id }

    /// Export the private key as a 0x-prefixed hex string.
    pub fn private_key_hex(&self) -> String {
        format!("0x{}", hex::encode(self.keypair.signing.to_bytes()))
    }

    // ── Signing ──────────────────────────────────────────────────────────────

    /// Sign a raw 32-byte hash.
    pub fn sign_hash(&self, hash: H256) -> Result<Signature, SdkError> {
        let raw = self.keypair.sign_hash(&hash)?;
        Ok(Signature::from_bytes(raw))
    }

    /// Sign a message with Ethereum personal sign prefix (EIP-191).
    pub fn personal_sign(&self, message: &[u8]) -> Result<Signature, SdkError> {
        let hash = keccak256(message);
        let raw  = self.keypair.personal_sign(&hash)?;
        Ok(Signature::from_bytes(raw))
    }

    /// Sign EIP-712 typed data.
    pub fn sign_typed_data(&self, domain_sep: H256, struct_hash: H256) -> Result<Signature, SdkError> {
        let raw = self.keypair.sign_typed_data(&domain_sep, &struct_hash)?;
        Ok(Signature::from_bytes(raw))
    }

    /// Sign a `TransactionRequest` with EIP-155 replay protection.
    pub fn sign_transaction(&self, mut tx: TransactionRequest) -> Result<SignedTransaction, SdkError> {
        tx.chain_id.get_or_insert(self.chain_id);
        let hash = tx.sighash();
        let sig  = self.keypair.sign_hash(&hash)?;
        SignedTransaction::from_request_and_sig(tx, sig)
    }

    // ── Keystore ─────────────────────────────────────────────────────────────

    /// Encrypt the wallet's private key into an Ethereum v3 keystore JSON string.
    ///
    /// Uses scrypt KDF + AES-128-CTR + keccak256 MAC.
    /// Compatible with MetaMask, geth, Ledger Live, and zbx-cli.
    pub fn to_keystore_json(&self, password: &str) -> String {
        use zbx_keystore::KeystoreWallet;
        let privkey_bytes = self.keypair.signing.to_bytes();
        let mut key = [0u8; 32];
        key.copy_from_slice(&privkey_bytes);
        let ks_wallet = match KeystoreWallet::from_private_key(&key) {
            Ok(w)  => w,
            Err(_) => return "{}".to_string(),
        };
        // Use N=8192 for the SDK (faster interactive use); CLI uses N=262144.
        let kf = match ks_wallet.to_keyfile(password, 8_192) {
            Ok(f)  => f,
            Err(_) => return "{}".to_string(),
        };
        serde_json::to_string(&kf).unwrap_or_else(|_| "{}".to_string())
    }

    /// Decrypt an Ethereum v3 keystore JSON string and reconstruct the wallet.
    ///
    /// Returns `SdkError::InvalidKey` if the password is wrong or the keystore
    /// is malformed. Returns `SdkError::Other` if the JSON cannot be parsed.
    pub fn from_keystore_json(json: &str, password: &str) -> Result<Self, SdkError> {
        use zbx_keystore::{KeyFile, KeystoreWallet};
        let kf: KeyFile = serde_json::from_str(json)
            .map_err(|e| SdkError::Other(format!("invalid keystore JSON: {}", e)))?;
        let ks_wallet = KeystoreWallet::from_keyfile(&kf, password)
            .map_err(|_| SdkError::InvalidKey(
                "wrong password or corrupt keystore".into()
            ))?;
        Self::from_bytes(ks_wallet.expose_private_key_unsafe())
    }
}

impl fmt::Debug for Wallet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Wallet {{ address: {:?}, chain_id: {} }}", self.address(), self.chain_id)
    }
}

/// A secp256k1 ECDSA signature (r, s, v).
#[derive(Debug, Clone, Copy)]
pub struct Signature {
    pub r: [u8; 32],
    pub s: [u8; 32],
    pub v: u8,
}

impl Signature {
    pub fn from_bytes(raw: [u8; 65]) -> Self {
        let mut r = [0u8; 32];
        let mut s = [0u8; 32];
        r.copy_from_slice(&raw[0..32]);
        s.copy_from_slice(&raw[32..64]);
        Self { r, s, v: raw[64] }
    }

    pub fn to_bytes(&self) -> [u8; 65] {
        let mut out = [0u8; 65];
        out[0..32].copy_from_slice(&self.r);
        out[32..64].copy_from_slice(&self.s);
        out[64] = self.v;
        out
    }

    pub fn to_hex(&self) -> String {
        format!("0x{}", hex::encode(self.to_bytes()))
    }

    /// EIP-2098 compact signature (64 bytes): r || s with v encoded in high bit of s.
    pub fn to_compact(&self) -> [u8; 64] {
        let mut out = [0u8; 64];
        out[0..32].copy_from_slice(&self.r);
        out[32..64].copy_from_slice(&self.s);
        if self.v == 28 { out[32] |= 0x80; }
        out
    }
}
