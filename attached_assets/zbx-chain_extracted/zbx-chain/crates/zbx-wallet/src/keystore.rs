//! Ethereum v3 keystore (Web3 Secret Storage Definition) implementation.
//!
//! Encrypts and decrypts secp256k1 private keys using:
//!   - Key derivation:  scrypt(password, salt, N, r=8, p=1, dkLen=32)
//!   - Encryption:      AES-128-CTR(dk[0..16], iv, private_key)
//!   - Authentication:  MAC = keccak256(dk[16..32] || ciphertext)
//!
//! Compatible with: MetaMask, geth, zbx-cli, Ledger Live, MyEtherWallet, Rainbow.
//!
//! ## Security parameters
//!   Production: N = 262144 (2^18, ~1 second on modern hardware)
//!   Testing:    N = 4096   (2^12, ~1 ms — NEVER use in production)

use aes::Aes128;
use aes::cipher::generic_array::GenericArray;
use ctr::{Ctr128BE, cipher::{KeyIvInit, StreamCipher}};
use scrypt::{Params as ScryptParams, scrypt as scrypt_kdf};
use sha3::{Keccak256, Digest};
use rand::rngs::OsRng;
use rand::RngCore;
use uuid::Uuid;
use std::collections::HashMap;
use crate::create_import::{
    KeystoreFile, KeystoreCrypto,
    ScryptParams as ZbxScryptParams,
    WalletError,
};

/// Production-grade scrypt N (2^18 = 262144 iterations, ≈ 1s per unlock).
pub const SCRYPT_N_HIGH: u32 = 262_144;
/// Test-only scrypt N (2^12 = 4096 iterations, ≈ 1ms). NEVER use in production.
pub const SCRYPT_N_TEST: u32 = 4_096;
/// SEC-2026-05-09 Pass-12 (S5/C3): minimum scrypt N enforced on DECRYPT.
/// EIP-2335 baseline for production keystores. A keystore that was
/// encrypted with N below this floor is rejected outright — without
/// this gate, a keystore crafted with N=2 (or even N=1) is brute-forceable
/// in milliseconds even with a strong password.
pub const SCRYPT_N_MIN_DECRYPT: u32 = 1 << 15; // 2^15 = 32768

/// Encrypt a private key into an Ethereum v3 keystore structure.
///
/// # Parameters
/// - `private_key` – 32-byte secp256k1 private key
/// - `address`     – 20-byte EVM address (stored as metadata in the keystore)
/// - `password`    – passphrase (UTF-8, any length)
/// - `n`           – scrypt N parameter (`SCRYPT_N_HIGH` for production)
///
/// # Errors
/// Returns `WalletError::InvalidKeystore` if scrypt params are invalid.
pub fn encrypt(
    private_key: &[u8; 32],
    address:     &[u8; 20],
    password:    &str,
    n:           u32,
) -> Result<KeystoreFile, WalletError> {
    // 1. Generate fresh 32-byte salt and 16-byte IV from OS RNG
    let mut salt = vec![0u8; 32];
    let mut iv   = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut iv);

    // 2. Derive 32-byte key via scrypt
    let log_n = log2_u32(n);
    let params = ScryptParams::new(log_n, 8, 1, 32)
        .map_err(|_| WalletError::InvalidKeystore)?;
    let mut dk = [0u8; 32];
    scrypt_kdf(password.as_bytes(), &salt, &params, &mut dk)
        .map_err(|_| WalletError::InvalidKeystore)?;

    // 3. Encrypt private key with AES-128-CTR using dk[0..16] as key
    let mut ciphertext = private_key.to_vec();
    aes128ctr_xor(&dk[..16], &iv, &mut ciphertext);

    // 4. MAC = keccak256(dk[16..32] || ciphertext)
    let mac: [u8; 32] = {
        let mut h = Keccak256::new();
        h.update(&dk[16..32]);
        h.update(&ciphertext);
        h.finalize().into()
    };

    // 5. Assemble keystore
    let mut cipherparams = HashMap::new();
    cipherparams.insert("iv".to_string(), hex::encode(iv));

    Ok(KeystoreFile {
        version: 3,
        id:      Uuid::new_v4().to_string(),
        address: hex::encode(address),
        crypto: KeystoreCrypto {
            cipher:       "aes-128-ctr".to_string(),
            ciphertext,
            cipherparams,
            kdf:          "scrypt".to_string(),
            kdfparams: ZbxScryptParams {
                n, r: 8, p: 1, dklen: 32, salt,
            },
            mac,
        },
    })
}

/// Decrypt an Ethereum v3 keystore with the given password.
///
/// Verifies the MAC before decryption to prevent padding oracle attacks.
///
/// # Errors
/// - `WalletError::InvalidPassphrase` if the MAC does not match (wrong password)
/// - `WalletError::InvalidKeystore` if the keystore structure is malformed
pub fn decrypt(keystore: &KeystoreFile, password: &str) -> Result<[u8; 32], WalletError> {
    let c = &keystore.crypto;

    // SEC-2026-05-09 Pass-12 (S5/C3): refuse weak KDF parameters before
    // spending any work. A malicious or corrupted keystore with N < 2^15
    // (or non-power-of-two N) is brute-forceable; reject up front.
    if c.kdfparams.n < SCRYPT_N_MIN_DECRYPT
        || c.kdfparams.n & (c.kdfparams.n - 1) != 0
        || c.kdfparams.r == 0 || c.kdfparams.p == 0
        || c.kdfparams.dklen != 32
        || c.kdfparams.salt.len() < 16
    {
        return Err(WalletError::InvalidKeystore);
    }

    // 1. Re-derive encryption key using stored scrypt parameters
    let log_n = log2_u32(c.kdfparams.n);
    let params = ScryptParams::new(
        log_n,
        c.kdfparams.r,
        c.kdfparams.p,
        c.kdfparams.dklen as usize,
    ).map_err(|_| WalletError::InvalidKeystore)?;

    let mut dk = [0u8; 32];
    scrypt_kdf(password.as_bytes(), &c.kdfparams.salt, &params, &mut dk)
        .map_err(|_| WalletError::InvalidKeystore)?;

    // 2. Verify MAC — must happen BEFORE decryption
    let computed_mac: [u8; 32] = {
        let mut h = Keccak256::new();
        h.update(&dk[16..32]);
        h.update(&c.ciphertext);
        h.finalize().into()
    };
    // SEC-2026-05-09 Pass-12 (S5/C3): constant-time MAC comparison.
    // Wrong-password handling MUST NOT short-circuit on the first mismatched
    // byte — a timing oracle leaks the prefix length of the correct MAC
    // and reduces the search space dramatically when an attacker can
    // probe the wallet repeatedly (CLI, daemon, hardware token interface).
    use subtle::ConstantTimeEq;
    if computed_mac.ct_eq(&c.mac).unwrap_u8() != 1 {
        return Err(WalletError::InvalidPassphrase);
    }

    // 3. Decode IV from cipherparams
    let iv_hex = c.cipherparams.get("iv").ok_or(WalletError::InvalidKeystore)?;
    let iv_bytes = hex::decode(iv_hex).map_err(|_| WalletError::InvalidKeystore)?;
    if iv_bytes.len() != 16 { return Err(WalletError::InvalidKeystore); }
    let iv: [u8; 16] = iv_bytes.try_into().map_err(|_| WalletError::InvalidKeystore)?;

    // 4. Decrypt (CTR mode is symmetric: same function for encrypt and decrypt)
    let mut plaintext = c.ciphertext.clone();
    aes128ctr_xor(&dk[..16], &iv, &mut plaintext);

    // 5. Extract private key
    if plaintext.len() != 32 { return Err(WalletError::InvalidKeystore); }
    let mut key = [0u8; 32];
    key.copy_from_slice(&plaintext);
    Ok(key)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// AES-128-CTR keystream XOR. CTR mode is symmetric; same function for en/decrypt.
fn aes128ctr_xor(key: &[u8], iv: &[u8; 16], data: &mut [u8]) {
    let key_ga = GenericArray::from_slice(&key[..16]);
    let iv_ga  = GenericArray::from_slice(iv.as_ref());
    let mut cipher = Ctr128BE::<Aes128>::new(key_ga, iv_ga);
    cipher.apply_keystream(data);
}

/// Integer log2 for powers of 2 (used for scrypt log_n parameter).
/// For N=262144=2^18: returns 18. For N=4096=2^12: returns 12.
fn log2_u32(n: u32) -> u8 {
    (31u32.saturating_sub(n.leading_zeros())) as u8
}
