//! TLSNotary-style attestation — proves TLS response is real.
//!
//! A "Notary" is a trusted third party that co-signs TLS sessions.
//! The reporter and Notary together create an MPC-TLS session:
//!   - Reporter holds: client_random, master_secret (half)
//!   - Notary holds:   master_secret (other half)
//!   - Neither alone can forge TLS traffic
//!   - Notary signs the session transcript
//!
//! This proves the HTTP response came from a real TLS server
//! without the notary seeing the actual content (privacy-preserving).

use serde_big_array::BigArray;
use serde::{Serialize, Deserialize};

/// Notary's attestation of a TLS session.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NotaryAttestation {
    /// Notary's compressed secp256k1 public key
    #[serde(with = "BigArray")]
    pub notary_pubkey:   [u8; 33],
    /// SHA-256 of the TLS transcript (proves session integrity)
    pub session_hash:    [u8; 32],
    /// Notary's signature over (session_hash || timestamp)
    #[serde(with = "BigArray")]
    pub notary_sig:      [u8; 64],
    /// Timestamp when notary signed
    pub timestamp:       u64,
    /// SNI (server name) of the TLS server — proves which CEX was queried
    pub server_name:     String,
}

impl NotaryAttestation {
    /// Verify the notary's secp256k1 signature over SHA-256(session_hash || timestamp).
    ///
    /// The notary signs the SHA-256 digest of `session_hash || timestamp_le`.
    /// We verify using the 33-byte compressed public key stored in `notary_pubkey`.
    pub fn verify(&self) -> bool {
        use k256::ecdsa::{signature::DigestVerifier, Signature as KSig, VerifyingKey};
        use sha2::{Digest, Sha256};

        // Build the verifying key from the compressed 33-byte SEC1 pubkey.
        let vk = match VerifyingKey::from_sec1_bytes(&self.notary_pubkey) {
            Ok(vk) => vk,
            Err(_) => return false,
        };

        // Parse the 64-byte compact (r || s) signature.
        let sig = match KSig::from_slice(&self.notary_sig) {
            Ok(s) => s,
            Err(_) => return false,
        };

        // Build the digest the notary signed over: SHA-256(session_hash || timestamp_le).
        let mut digest = Sha256::new();
        digest.update(&self.session_hash);
        digest.update(&self.timestamp.to_le_bytes());

        // Verify using DigestVerifier — passes the Sha256 digest directly to k256
        // without finalizing it ourselves (avoids double-hash).
        vk.verify_digest(digest, &sig).is_ok()
    }

    /// Check if this attestation is for an approved CEX.
    pub fn is_approved_source(&self) -> bool {
        matches!(
            self.server_name.as_str(),
            "api.binance.com" | "api.coinbase.com" | "api.kraken.com" | "www.okx.com"
        )
    }
}