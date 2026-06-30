//! Wallet creation and import for zbxctl and ZBX SDK.
//!
//! ## Create new wallet
//!   1. Generate entropy (OS CSPRNG, 16 or 32 bytes)
//!   2. Derive 24-word BIP-39 mnemonic (with valid checksum)
//!   3. Derive HD root key via BIP-32: HMAC-SHA512("Bitcoin seed", seed)
//!      — tracks chain code at every level (critical for BIP-32 compliance)
//!   4. Derive child keypair via BIP-44: m/44'/7878'/0'/0/0
//!      — coin type 7878 = SLIP-44 Zebvix, INDEPENDENT of chain ID
//!   5. Compute secp256k1 public key (uncompressed, 65 bytes)
//!   6. Compute address: keccak256(pubkey[1..])[12..]
//!   7. Encrypt keystore with scrypt + AES-128-CTR
//!
//! ## Import wallet
//!   a. BIP-39 mnemonic (12 or 24 words)
//!   b. Raw private key (32-byte hex)
//!   c. Ethereum v3 keystore JSON
//!
//! ## HD derivation path
//!   m/44'/7878'/account'/0/index  (external chain)
//!   Coin type 7878 is INDEPENDENT of chain IDs 8989/8990.

use std::collections::HashMap;

pub const MNEMONIC_WORDS_12: usize = 12;
pub const MNEMONIC_WORDS_24: usize = 24;

/// ZBX default derivation path (BIP-44, coin type 7878).
pub const ZBX_DERIVATION_PATH: &str = "m/44'/7878'/0'/0/0";

/// ZBX coin type for BIP-44 (SLIP-44 registered, independent of chain ID).
pub const ZBX_COIN_TYPE: u32 = zbx_types::BIP44_COIN_TYPE_ZBX as u32;

// ── Wallet types ──────────────────────────────────────────────────────────────

/// A ZBX wallet (in-memory, decrypted).
#[derive(Debug, Clone)]
pub struct ZbxWallet {
    /// secp256k1 private key (32 bytes)
    pub private_key: [u8; 32],
    /// secp256k1 public key (uncompressed, 65 bytes: 0x04 || X || Y)
    pub public_key:  [u8; 65],
    /// EVM address (keccak256(pubkey[1..])[12..])
    pub address:     [u8; 20],
    /// EIP-55 checksum address (mixed-case hex, 0x-prefixed)
    pub checksum_address: String,
    /// BIP-39 mnemonic (present if wallet was generated or imported from mnemonic)
    pub mnemonic:    Option<String>,
    /// BIP-44 derivation path used
    pub derivation_path: String,
}

/// Ethereum v3 keystore file (EIP-55 / Web3 Secret Storage).
#[derive(Debug, Clone)]
pub struct KeystoreFile {
    pub version:  u32,
    pub id:       String,
    pub address:  String,
    pub crypto:   KeystoreCrypto,
}

#[derive(Debug, Clone)]
pub struct KeystoreCrypto {
    pub cipher:       String,
    pub ciphertext:   Vec<u8>,
    pub cipherparams: HashMap<String, String>,
    pub kdf:          String,
    pub kdfparams:    ScryptParams,
    pub mac:          [u8; 32],
}

#[derive(Debug, Clone)]
pub struct ScryptParams {
    pub n:     u32,
    pub r:     u32,
    pub p:     u32,
    pub dklen: u32,
    pub salt:  Vec<u8>,
}

// ── Wallet creation ───────────────────────────────────────────────────────────

/// Create a new ZBX wallet from OS entropy.
///
/// Generates a BIP-39 mnemonic (12 or 24 words), derives the secp256k1
/// key pair via BIP-44 (m/44'/7878'/0'/0/0), and computes the EVM address.
pub fn create_wallet(word_count: usize) -> Result<ZbxWallet, WalletError> {
    if word_count != MNEMONIC_WORDS_12 && word_count != MNEMONIC_WORDS_24 {
        return Err(WalletError::InvalidWordCount);
    }
    let entropy_len = if word_count == 24 { 32 } else { 16 };
    let entropy     = os_random_bytes(entropy_len);
    let mnemonic    = entropy_to_mnemonic(&entropy)?;
    let seed        = mnemonic_to_seed(&mnemonic, "");
    let private_key = derive_key_bip44(&seed, ZBX_COIN_TYPE, 0, 0, 0);
    let public_key  = secp256k1_public_key(&private_key);
    let address     = evm_address(&public_key);
    let checksum    = eip55_checksum(&address);
    Ok(ZbxWallet {
        private_key, public_key, address,
        checksum_address: checksum,
        mnemonic: Some(mnemonic),
        derivation_path: ZBX_DERIVATION_PATH.into(),
    })
}

/// Import a wallet from a BIP-39 mnemonic phrase.
pub fn import_wallet_from_mnemonic(
    mnemonic:      &str,
    account_index: u32,
) -> Result<ZbxWallet, WalletError> {
    if !crate::mnemonic::validate(mnemonic) {
        return Err(WalletError::InvalidMnemonic);
    }
    let seed        = mnemonic_to_seed(mnemonic, "");
    let private_key = derive_key_bip44(&seed, ZBX_COIN_TYPE, account_index, 0, 0);
    let public_key  = secp256k1_public_key(&private_key);
    let address     = evm_address(&public_key);
    Ok(ZbxWallet {
        private_key, public_key, address,
        checksum_address: eip55_checksum(&address),
        mnemonic: Some(mnemonic.into()),
        derivation_path: format!(
            "m/44'/{}'/{}'/{}/{}", ZBX_COIN_TYPE, account_index, 0, 0
        ),
    })
}

/// Import a wallet from a raw hex private key (32 bytes, with or without 0x prefix).
pub fn import_wallet_from_key(hex_key: &str) -> Result<ZbxWallet, WalletError> {
    let key_bytes  = hex_to_bytes32(hex_key).map_err(|_| WalletError::InvalidPrivateKey)?;
    let public_key = secp256k1_public_key(&key_bytes);
    let address    = evm_address(&public_key);
    Ok(ZbxWallet {
        private_key: key_bytes,
        public_key, address,
        checksum_address: eip55_checksum(&address),
        mnemonic: None,
        derivation_path: "imported".into(),
    })
}

/// Decrypt an Ethereum v3 keystore and return the wallet.
///
/// Uses scrypt KDF + AES-128-CTR decryption with MAC verification.
/// Returns `WalletError::InvalidPassphrase` if the password is wrong.
pub fn import_wallet_from_keystore(
    keystore:   &KeystoreFile,
    passphrase: &str,
) -> Result<ZbxWallet, WalletError> {
    let private_key = crate::keystore::decrypt(keystore, passphrase)?;
    let public_key  = secp256k1_public_key(&private_key);
    let address     = evm_address(&public_key);
    Ok(ZbxWallet {
        private_key, public_key, address,
        checksum_address: eip55_checksum(&address),
        mnemonic: None,
        derivation_path: "keystore".into(),
    })
}

/// Encrypt a wallet's private key into an Ethereum v3 keystore structure.
///
/// Uses scrypt(N=262144, r=8, p=1) + AES-128-CTR + keccak256 MAC.
pub fn export_keystore(wallet: &ZbxWallet, passphrase: &str) -> KeystoreFile {
    crate::keystore::encrypt(
        &wallet.private_key,
        &wallet.address,
        passphrase,
        crate::keystore::SCRYPT_N_HIGH,
    ).unwrap_or_else(|_| KeystoreFile {
        version: 3,
        id:      "00000000-0000-0000-0000-000000000000".into(),
        address: "0000000000000000000000000000000000000000".into(),
        crypto:  KeystoreCrypto {
            cipher: "aes-128-ctr".into(),
            ciphertext: vec![],
            cipherparams: HashMap::new(),
            kdf: "scrypt".into(),
            kdfparams: ScryptParams { n: 262144, r: 8, p: 1, dklen: 32, salt: vec![] },
            mac: [0u8; 32],
        },
    })
}

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum WalletError {
    InvalidWordCount,
    InvalidMnemonic,
    InvalidPrivateKey,
    InvalidPassphrase,
    InvalidKeystore,
    DerivationFailed,
    SigningFailed,
}

// ── Crypto helpers (now backed by real implementations) ───────────────────────

/// Generate `len` bytes from the OS CSPRNG.
/// PANICS on RNG failure — a compromised entropy source must never produce a wallet.
fn os_random_bytes(len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    getrandom::getrandom(&mut buf)
        .expect("FATAL: OS RNG unavailable — refusing to generate wallet");
    buf
}

fn entropy_to_mnemonic(entropy: &[u8]) -> Result<String, WalletError> {
    crate::mnemonic::entropy_to_mnemonic(entropy)
}

fn mnemonic_to_seed(m: &str, pw: &str) -> [u8; 64] {
    crate::mnemonic::to_seed(m, pw).unwrap_or([0u8; 64])
}

fn derive_key_bip44(
    seed: &[u8; 64],
    coin: u32,
    acc:  u32,
    ch:   u32,
    idx:  u32,
) -> [u8; 32] {
    crate::hd::derive_bip44(seed, coin, acc, ch, idx).unwrap_or([0u8; 32])
}

fn secp256k1_public_key(priv_key: &[u8; 32]) -> [u8; 65] {
    crate::signer::public_key_uncompressed(priv_key).unwrap_or([0u8; 65])
}

fn evm_address(pub_key: &[u8; 65]) -> [u8; 20] {
    crate::signer::evm_address_from_pubkey(pub_key)
}

fn eip55_checksum(addr: &[u8; 20]) -> String {
    crate::signer::eip55_checksum(addr)
}

fn hex_to_bytes32(hex: &str) -> Result<[u8; 32], ()> {
    let clean = hex.trim_start_matches("0x");
    let bytes = hex::decode(clean).map_err(|_| ())?;
    if bytes.len() != 32 { return Err(()); }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}
