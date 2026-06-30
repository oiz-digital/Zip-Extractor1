//! Keystore file format — Ethereum v3 JSON keystore.

use serde::{Deserialize, Serialize};
use crate::KeystoreError;

/// Ethereum-compatible v3 keystore file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyFile {
    pub version: u8,
    pub id:      String,       // UUID
    pub address: String,       // 0x... (checksummed)
    pub crypto:  CryptoParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CryptoParams {
    pub cipher:       String,   // "aes-128-ctr"
    pub cipherparams: CipherParams,
    pub ciphertext:   String,   // hex-encoded encrypted private key
    pub kdf:          String,   // "scrypt" or "pbkdf2"
    pub kdfparams:    KdfParams,
    pub mac:          String,   // hex-encoded MAC for integrity check
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CipherParams {
    pub iv: String,   // hex-encoded AES-CTR IV (16 bytes)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KdfParams {
    pub dklen: u32,   // derived key length (32 bytes for AES-256)
    pub salt:  String, // hex-encoded random salt
    // Scrypt params
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,  // CPU/memory cost (262144 for mainnet, 8192 for fast)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r: Option<u32>,  // block size (8)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p: Option<u32>,  // parallelisation (1)
    // PBKDF2 params
    #[serde(skip_serializing_if = "Option::is_none")]
    pub c: Option<u32>,  // iterations (262144)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prf: Option<String>, // "hmac-sha256"
}

impl KeyFile {
    /// Minimum acceptable scrypt N for production keystores (128K = 2^17).
    /// Below this, brute-force attacks take <1 second per attempt on modern hardware.
    /// Test keystores may use lower N (n=2 or n=8192) but are rejected in production.
    pub const MIN_SCRYPT_N: u32 = 131_072; // 128K

    /// SEC-2026-05-09 (N1): minimum acceptable PBKDF2 iteration count.
    /// Geth and the original Web3 Secret Storage spec both default to 262144;
    /// anything below ~100K is brute-forceable in seconds on modern GPUs.
    pub const MIN_PBKDF2_ITERS: u32 = 100_000;

    /// Parse from JSON bytes.
    pub fn from_json(json: &[u8]) -> Result<Self, KeystoreError> {
        let kf: Self = serde_json::from_slice(json)?;
        if kf.version != 3 {
            return Err(KeystoreError::InvalidFormat(
                format!("expected version 3, got {}", kf.version)
            ));
        }
        // M-06 fix: enforce minimum KDF work factor at parse time.
        // Rejects keystores with weak scrypt N that would be brute-forceable.
        if kf.crypto.kdf == "scrypt" {
            if let Some(n) = kf.crypto.kdfparams.n {
                if n < Self::MIN_SCRYPT_N {
                    return Err(KeystoreError::InvalidFormat(
                        format!(
                            "scrypt N={} is below the minimum safe value {}; \
                             regenerate with N >= {} for production keystores",
                            n, Self::MIN_SCRYPT_N, Self::MIN_SCRYPT_N
                        )
                    ));
                }
            }
        }
        // SEC-2026-05-09 (N1): mirror the scrypt floor for the PBKDF2 KDF —
        // previously a keystore with `c=1000` would parse and decrypt fine,
        // making the password trivially crackable.
        if kf.crypto.kdf == "pbkdf2" {
            if let Some(c) = kf.crypto.kdfparams.c {
                if c < Self::MIN_PBKDF2_ITERS {
                    return Err(KeystoreError::InvalidFormat(
                        format!(
                            "pbkdf2 c={} is below the minimum safe value {}; \
                             regenerate with c >= {} for production keystores",
                            c, Self::MIN_PBKDF2_ITERS, Self::MIN_PBKDF2_ITERS
                        )
                    ));
                }
            }
        }
        Ok(kf)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, KeystoreError> {
        Ok(serde_json::to_vec_pretty(self)?)
    }

    pub fn address_bytes(&self) -> Result<[u8; 20], KeystoreError> {
        let hex = self.address.trim_start_matches("0x");
        let bytes = hex::decode(hex)
            .map_err(|_| KeystoreError::InvalidFormat("bad address hex".into()))?;
        bytes.try_into().map_err(|_| KeystoreError::InvalidFormat("address not 20 bytes".into()))
    }
}