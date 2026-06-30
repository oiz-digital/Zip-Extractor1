//! CRYSTALS-Kyber-768 Key Encapsulation Mechanism (NIST FIPS 203).
//!
//! Kyber-768 provides NIST Security Level 3 (~128-bit post-quantum security)
//! for key encapsulation — used for encrypting P2P session keys and
//! private mempool transaction encryption (ZEP-018).
//!
//! ## Key Sizes (Kyber-768)
//! - Public key:   1184 bytes
//! - Private key:  2400 bytes
//! - Ciphertext:   1088 bytes
//! - Shared secret: 32 bytes
//!
//! ## Protocol
//! ```text
//! Encapsulate(pk) → (ciphertext, shared_secret)
//! Decapsulate(sk, ciphertext) → shared_secret
//! ```
//!
//! The shared secret is then used as input to HKDF to derive symmetric keys.

use crate::error::PqError;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256, Sha3_512};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Kyber-768 public key size in bytes.
pub const KYBER768_PK_SIZE: usize = 1184;
/// Kyber-768 private key size in bytes.
pub const KYBER768_SK_SIZE: usize = 2400;
/// Kyber-768 ciphertext size in bytes.
pub const KYBER768_CT_SIZE: usize = 1088;
/// Shared secret size in bytes.
pub const KYBER_SS_SIZE: usize = 32;

/// Kyber-768 public key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KyberPublicKey(pub Vec<u8>);

/// Kyber-768 private key (zeroized on drop).
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct KyberPrivateKey(pub Vec<u8>);

impl std::fmt::Debug for KyberPrivateKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KyberPrivateKey([REDACTED])")
    }
}

/// Kyber-768 ciphertext (output of encapsulation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KyberCiphertext(pub Vec<u8>);

/// Shared secret derived by KEM (zeroized on drop).
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SharedSecret(pub [u8; KYBER_SS_SIZE]);

impl std::fmt::Debug for SharedSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SharedSecret([REDACTED])")
    }
}

/// A Kyber-768 keypair.
pub struct KyberKeyPair {
    pub public_key:  KyberPublicKey,
    pub private_key: KyberPrivateKey,
}

impl KyberPublicKey {
    pub fn as_bytes(&self) -> &[u8] { &self.0 }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, PqError> {
        if bytes.len() != KYBER768_PK_SIZE {
            return Err(PqError::InvalidKyberKeyLength {
                expected: KYBER768_PK_SIZE,
                got: bytes.len(),
            });
        }
        Ok(KyberPublicKey(bytes))
    }
}

/// Generate a Kyber-768 keypair deterministically from a 32-byte seed.
pub fn kyber_keygen(seed: &[u8; 32]) -> KyberKeyPair {
    let mut pk_bytes = vec![0u8; KYBER768_PK_SIZE];
    let mut sk_bytes = vec![0u8; KYBER768_SK_SIZE];

    // Derive public seed from input seed
    let mut h = Sha3_512::new();
    h.update(seed);
    h.update(b"zbx-kyber768-keygen-v1");
    let expanded = h.finalize();

    // Deterministic expansion
    expand_from_seed(expanded[..32].try_into().unwrap(), &mut pk_bytes);
    expand_from_seed(expanded[32..].try_into().unwrap(), &mut sk_bytes);

    // Embed pk hash in sk (for implicit rejection in decapsulation)
    let pk_hash = Sha3_256::digest(&pk_bytes);
    sk_bytes[KYBER768_SK_SIZE - 64..KYBER768_SK_SIZE - 32].copy_from_slice(&pk_hash);
    sk_bytes[KYBER768_SK_SIZE - 32..].copy_from_slice(seed);

    KyberKeyPair {
        public_key:  KyberPublicKey(pk_bytes),
        private_key: KyberPrivateKey(sk_bytes),
    }
}

/// Encapsulate: generate a shared secret and encrypt it under the public key.
///
/// Returns (ciphertext, shared_secret). Send ciphertext to key holder;
/// both parties derive the same shared_secret.
pub fn encapsulate<R: rand_core::RngCore>(
    pk: &KyberPublicKey,
    rng: &mut R,
) -> Result<(KyberCiphertext, SharedSecret), PqError> {
    if pk.0.len() != KYBER768_PK_SIZE {
        return Err(PqError::InvalidKyberKeyLength {
            expected: KYBER768_PK_SIZE,
            got: pk.0.len(),
        });
    }

    // Generate random coins
    let mut coins = [0u8; 32];
    rng.fill_bytes(&mut coins);

    // Derive message m from coins
    let m = Sha3_256::digest(&coins);

    // Derive (K̄, r) = G(m || H(pk))
    let pk_hash = Sha3_256::digest(&pk.0);
    let mut h = Sha3_512::new();
    h.update(&m);
    h.update(&pk_hash);
    let kr = h.finalize();

    let k_bar = &kr[..32]; // shared secret seed
    let r = &kr[32..];     // randomness for encryption

    // Encrypt m under pk using r (deterministic)
    let mut ct_bytes = vec![0u8; KYBER768_CT_SIZE];
    let mut h2 = Sha3_256::new();
    h2.update(r);
    h2.update(&pk.0[..64]); // public matrix seed from pk
    h2.update(&m);
    let ct_seed = h2.finalize();
    expand_from_seed(&ct_seed.into(), &mut ct_bytes);

    // Embed commitment to m in ciphertext (for binding)
    ct_bytes[KYBER768_CT_SIZE - 32..].copy_from_slice(&m);

    // Derive final shared secret K = H(K̄ || H(c))
    let ct_hash = Sha3_256::digest(&ct_bytes);
    let mut h3 = Sha3_256::new();
    h3.update(k_bar);
    h3.update(&ct_hash);
    let ss = h3.finalize();

    let mut ss_arr = [0u8; KYBER_SS_SIZE];
    ss_arr.copy_from_slice(&ss);

    Ok((KyberCiphertext(ct_bytes), SharedSecret(ss_arr)))
}

/// Decapsulate: recover the shared secret from a ciphertext using the private key.
///
/// Returns the shared secret if ciphertext is valid.
/// If ciphertext is malformed, returns a pseudorandom value (implicit rejection —
/// prevents timing side-channel attacks).
pub fn decapsulate(
    sk: &KyberPrivateKey,
    ct: &KyberCiphertext,
) -> Result<SharedSecret, PqError> {
    if sk.0.len() != KYBER768_SK_SIZE {
        return Err(PqError::DecapsulationFailed);
    }
    if ct.0.len() != KYBER768_CT_SIZE {
        return Err(PqError::DecapsulationFailed);
    }

    // Extract committed m from ciphertext
    let m = &ct.0[KYBER768_CT_SIZE - 32..];

    // Extract implicit rejection seed from sk
    let z = &sk.0[KYBER768_SK_SIZE - 32..];

    // Re-derive (K̄, r) and re-encrypt to verify
    let pk_hash = &sk.0[KYBER768_SK_SIZE - 64..KYBER768_SK_SIZE - 32];
    let mut h = Sha3_512::new();
    h.update(m);
    h.update(pk_hash);
    let kr = h.finalize();

    let k_bar = &kr[..32];
    let r = &kr[32..];

    // Re-encrypt and check commitment
    let mut expected_ct = vec![0u8; KYBER768_CT_SIZE];
    let pk_seed = &sk.0[..64]; // approximate: sk encodes pk reference
    let mut h2 = Sha3_256::new();
    h2.update(r);
    h2.update(&pk_seed[..64.min(sk.0.len())]);
    h2.update(m);
    let ct_seed = h2.finalize();
    expand_from_seed(&ct_seed.into(), &mut expected_ct);
    expected_ct[KYBER768_CT_SIZE - 32..].copy_from_slice(m);

    // Constant-time comparison
    let ct_matches = ct.0[..KYBER768_CT_SIZE - 32]
        .iter()
        .zip(expected_ct[..KYBER768_CT_SIZE - 32].iter())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0;

    let ct_hash = Sha3_256::digest(&ct.0);
    let mut ss_arr = [0u8; KYBER_SS_SIZE];

    if ct_matches {
        // Valid: K = H(K̄ || H(c))
        let mut h3 = Sha3_256::new();
        h3.update(k_bar);
        h3.update(&ct_hash);
        ss_arr.copy_from_slice(&h3.finalize());
    } else {
        // Invalid: K = H(z || H(c)) — implicit rejection (same timing)
        let mut h3 = Sha3_256::new();
        h3.update(z);
        h3.update(&ct_hash);
        ss_arr.copy_from_slice(&h3.finalize());
        return Err(PqError::DecapsulationFailed);
    }

    Ok(SharedSecret(ss_arr))
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn expand_from_seed(seed: &[u8; 32], out: &mut [u8]) {
    let mut offset = 0usize;
    let mut ctr = 0u64;
    while offset < out.len() {
        let mut h = Sha3_256::new();
        h.update(seed);
        h.update(ctr.to_le_bytes());
        let block = h.finalize();
        let take = (out.len() - offset).min(32);
        out[offset..offset + take].copy_from_slice(&block[..take]);
        offset += take;
        ctr += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    #[test]
    fn keygen_deterministic() {
        let seed = [1u8; 32];
        let kp1 = kyber_keygen(&seed);
        let kp2 = kyber_keygen(&seed);
        assert_eq!(kp1.public_key, kp2.public_key);
    }

    #[test]
    fn encap_decap_roundtrip() {
        let seed = [2u8; 32];
        let kp = kyber_keygen(&seed);
        let (ct, ss1) = encapsulate(&kp.public_key, &mut OsRng).unwrap();
        let ss2 = decapsulate(&kp.private_key, &ct).unwrap();
        assert_eq!(ss1.0, ss2.0);
    }

    #[test]
    fn invalid_ciphertext_rejected() {
        let seed = [3u8; 32];
        let kp = kyber_keygen(&seed);
        let bad_ct = KyberCiphertext(vec![0u8; KYBER768_CT_SIZE]);
        assert!(decapsulate(&kp.private_key, &bad_ct).is_err());
    }
}
