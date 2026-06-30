//! CRYSTALS-Dilithium-3 post-quantum digital signatures (NIST FIPS 204 / ML-DSA-65).
//!
//! Dilithium-3 / ML-DSA-65 provides NIST Security Level 3 (~128-bit post-quantum security).
//!
//! ## Key Sizes (ML-DSA-65 = Dilithium-3 FIPS 204)
//! - Public key:  1952 bytes
//! - Private key: 4032 bytes
//! - Signature:   3309 bytes
//!
//! ## Implementation
//!
//! Uses the `fips204` crate — pure-Rust, constant-time NIST FIPS 204 ML-DSA-65.
//! All polynomial arithmetic (NTT, rejection sampling, hint generation) is real.
//!
//! ## Algorithm Overview
//!
//! Dilithium is a lattice-based Fiat-Shamir-with-aborts scheme:
//!
//! ```text
//! KeyGen(seed):
//!   A = ExpandA(ρ)                      -- public matrix
//!   (s₁, s₂) = sample_error_vectors()  -- private key
//!   t = A·s₁ + s₂                      -- public key hint
//!   pk = (ρ, t₁)  sk = (ρ, K, tr, s₁, s₂, t₀)
//!
//! Sign(sk, M):
//!   y = sample_masking_vector()
//!   w₁ = HighBits(A·y)
//!   c = SampleInBall(H(μ || w₁))
//!   z = y + c·s₁
//!   Repeat if rejection conditions hold
//!   σ = (c̃, z, h)
//!
//! Verify(pk, M, σ):
//!   w₁' = UseHint(h, A·z − c·t)
//!   Accept iff H(μ || w₁') == c̃
//! ```

use crate::error::PqError;
use fips204::ml_dsa_65;
use fips204::traits::{KeyGen, SerDes, Signer, Verifier};
use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;
use sha3::{Digest, Keccak256};
use zeroize::Zeroize;

/// ML-DSA-65 (= Dilithium-3 FIPS 204) public key size in bytes.
pub const DILITHIUM3_PK_SIZE: usize = 1952;
/// ML-DSA-65 private key size in bytes.
pub const DILITHIUM3_SK_SIZE: usize = 4032;
/// ML-DSA-65 signature size in bytes.
pub const DILITHIUM3_SIG_SIZE: usize = 3309;

/// Dilithium-3 public key (1952 bytes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DilithiumPublicKey(#[serde(with = "BigArray")] pub [u8; DILITHIUM3_PK_SIZE]);

/// Dilithium-3 private key (4032 bytes, zeroized on drop).
#[derive(Clone, Zeroize)]
#[zeroize(drop)]
pub struct DilithiumPrivateKey(pub Box<[u8; DILITHIUM3_SK_SIZE]>);

impl std::fmt::Debug for DilithiumPrivateKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DilithiumPrivateKey([REDACTED])")
    }
}

/// Dilithium-3 signature (3309 bytes, returned as Vec for API compatibility).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DilithiumSignature(pub Vec<u8>);

/// A Dilithium-3 keypair.
pub struct DilithiumKeyPair {
    pub public_key:  DilithiumPublicKey,
    pub private_key: DilithiumPrivateKey,
}

impl DilithiumPublicKey {
    /// Derive a ZBX address (20 bytes) from this public key.
    ///
    /// Address = keccak256(pubkey)[12..32]  — same derivation as ECDSA.
    pub fn to_address(&self) -> [u8; 20] {
        let hash = Keccak256::digest(&self.0);
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&hash[12..32]);
        addr
    }

    /// Serialize to hex string.
    pub fn to_hex(&self) -> String {
        hex::encode(&self.0)
    }

    /// Deserialize from hex string.
    pub fn from_hex(s: &str) -> Result<Self, PqError> {
        let bytes = hex::decode(s)
            .map_err(|e| PqError::Serialization(e.to_string()))?;
        if bytes.len() != DILITHIUM3_PK_SIZE {
            return Err(PqError::InvalidPublicKeyLength {
                expected: DILITHIUM3_PK_SIZE,
                got: bytes.len(),
            });
        }
        let mut arr = [0u8; DILITHIUM3_PK_SIZE];
        arr.copy_from_slice(&bytes);
        Ok(DilithiumPublicKey(arr))
    }
}

impl DilithiumSignature {
    pub fn as_bytes(&self) -> &[u8] { &self.0 }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, PqError> {
        if bytes.len() != DILITHIUM3_SIG_SIZE {
            return Err(PqError::InvalidSignatureLength {
                expected: DILITHIUM3_SIG_SIZE,
                got: bytes.len(),
            });
        }
        Ok(DilithiumSignature(bytes))
    }
}

/// Generate a Dilithium-3 keypair deterministically from a 32-byte seed.
///
/// Uses `StdRng::from_seed(seed)` (ChaCha12 CSPRNG) to feed the ML-DSA-65
/// key generation, which then performs the full FIPS 204 lattice computation:
///
/// 1. `(ρ, ρ', K) = H(internal_seed)` via SHAKE-256
/// 2. `A = ExpandA(ρ)` — expand public matrix from ρ
/// 3. `(s₁, s₂) = ExpandS(ρ', η)` — sample bounded error vectors
/// 4. `t = NTT⁻¹(NTT(A) ∘ NTT(s₁)) + s₂`
///
/// In production: `seed = HKDF-SHA256(master_seed, "zbx-dilithium-v1")`.
pub fn keygen_from_seed(seed: &[u8; 32]) -> DilithiumKeyPair {
    use rand::SeedableRng;
    let mut rng = rand::rngs::StdRng::from_seed(*seed);
    let (pk_obj, sk_obj) = ml_dsa_65::KG::try_keygen_with_rng(&mut rng)
        .expect("ML-DSA-65 keygen from seeded RNG");

    let pk_bytes: [u8; DILITHIUM3_PK_SIZE] = pk_obj.into_bytes();
    let sk_bytes: [u8; DILITHIUM3_SK_SIZE] = sk_obj.into_bytes();

    DilithiumKeyPair {
        public_key:  DilithiumPublicKey(pk_bytes),
        private_key: DilithiumPrivateKey(Box::new(sk_bytes)),
    }
}

/// Generate a Dilithium-3 keypair from an OS-entropy RNG.
///
/// Preferred for new key generation in production validators.
pub fn keygen_random<R: rand_core::RngCore + rand_core::CryptoRng>(rng: &mut R) -> DilithiumKeyPair {
    let (pk_obj, sk_obj) = ml_dsa_65::KG::try_keygen_with_rng(rng)
        .expect("ML-DSA-65 keygen");

    let pk_bytes: [u8; DILITHIUM3_PK_SIZE] = pk_obj.into_bytes();
    let sk_bytes: [u8; DILITHIUM3_SK_SIZE] = sk_obj.into_bytes();

    DilithiumKeyPair {
        public_key:  DilithiumPublicKey(pk_bytes),
        private_key: DilithiumPrivateKey(Box::new(sk_bytes)),
    }
}

/// Sign a message with a Dilithium-3 private key.
///
/// Uses the FIPS 204 ML-DSA-65 deterministic signing (`try_sign`):
/// `σ = Sign(sk, M, ctx)` where ctx = b"" (empty ZBX context label).
///
/// The signature encodes commitment hash c̃, response vector z, and hint h,
/// all computed via real NTT polynomial operations over the module lattice.
pub fn sign(sk: &DilithiumPrivateKey, message: &[u8]) -> DilithiumSignature {
    let sk_obj = ml_dsa_65::PrivateKey::try_from_bytes(*sk.0)
        .expect("ML-DSA-65: valid private key bytes");

    // try_sign: deterministic signing (FIPS 204 §5.2)
    // Context string is empty; ZBX callers embed domain separation in `message`.
    let sig_arr: [u8; DILITHIUM3_SIG_SIZE] = sk_obj
        .try_sign(message, b"")
        .expect("ML-DSA-65 signing");

    DilithiumSignature(sig_arr.to_vec())
}

/// Sign with an OS-random hedge (FIPS 204 randomized signing §5.2).
///
/// Provides resilience against side-channel attacks that exploit determinism.
/// In contexts without side-channel exposure, `sign()` is sufficient.
pub fn sign_hedged<R: rand_core::RngCore + rand_core::CryptoRng>(
    sk:      &DilithiumPrivateKey,
    message: &[u8],
    rng:     &mut R,
) -> DilithiumSignature {
    let sk_obj = ml_dsa_65::PrivateKey::try_from_bytes(*sk.0)
        .expect("ML-DSA-65: valid private key bytes");

    let sig_arr: [u8; DILITHIUM3_SIG_SIZE] = sk_obj
        .try_sign_with_rng(rng, message, b"")
        .expect("ML-DSA-65 hedged signing");

    DilithiumSignature(sig_arr.to_vec())
}

/// Verify a Dilithium-3 signature.
///
/// Performs the full ML-DSA-65 verification:
/// 1. Decode c̃, z, h from σ
/// 2. Recompute μ = H(H(pk) || M)
/// 3. w₁' = UseHint(h, NTT⁻¹(NTT(A)·NTT(z) − c·NTT(t)))
/// 4. Accept iff H(μ || w₁') == c̃ and norm bounds hold
///
/// Returns `Ok(())` if valid, `Err(PqError::SignatureVerificationFailed)` if not.
pub fn verify(
    pk:      &DilithiumPublicKey,
    message: &[u8],
    sig:     &DilithiumSignature,
) -> Result<(), PqError> {
    if sig.0.len() != DILITHIUM3_SIG_SIZE {
        return Err(PqError::InvalidSignatureLength {
            expected: DILITHIUM3_SIG_SIZE,
            got: sig.0.len(),
        });
    }

    let pk_obj = ml_dsa_65::PublicKey::try_from_bytes(pk.0)
        .map_err(|_| PqError::SignatureVerificationFailed)?;

    // Convert Vec<u8> to fixed-size array required by FIPS 204 API.
    let sig_arr: &[u8; DILITHIUM3_SIG_SIZE] = sig.0.as_slice().try_into()
        .map_err(|_| PqError::InvalidSignatureLength {
            expected: DILITHIUM3_SIG_SIZE,
            got: sig.0.len(),
        })?;

    // Full polynomial-based verify: NTT(A·z − c·t) + hint check.
    // Returns bool in fips204 v0.4.x.
    let valid = pk_obj.verify(message, sig_arr, b"");

    if valid { Ok(()) } else { Err(PqError::SignatureVerificationFailed) }
}

/// Proof of possession: sign a well-known domain-tagged message to prove
/// ownership of the private key during validator registration.
///
/// The domain tag `b"zbx-pop-v1"` prevents cross-protocol signature reuse.
pub fn proof_of_possession(sk: &DilithiumPrivateKey, validator_addr: &[u8; 20]) -> DilithiumSignature {
    let mut msg = b"zbx-pop-v1".to_vec();
    msg.extend_from_slice(validator_addr);
    sign(sk, &msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    #[test]
    fn keygen_from_seed_is_deterministic() {
        let seed = [42u8; 32];
        let kp1 = keygen_from_seed(&seed);
        let kp2 = keygen_from_seed(&seed);
        assert_eq!(kp1.public_key.0, kp2.public_key.0, "same seed → same pk");
    }

    #[test]
    fn different_seeds_produce_different_keys() {
        let kp1 = keygen_from_seed(&[1u8; 32]);
        let kp2 = keygen_from_seed(&[2u8; 32]);
        assert_ne!(kp1.public_key.0, kp2.public_key.0);
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let kp = keygen_from_seed(&[7u8; 32]);
        let msg = b"zbx-dilithium3-test-message";
        let sig = sign(&kp.private_key, msg);
        assert!(verify(&kp.public_key, msg, &sig).is_ok());
    }

    #[test]
    fn verify_rejects_wrong_message() {
        let kp = keygen_from_seed(&[8u8; 32]);
        let sig = sign(&kp.private_key, b"message-a");
        assert!(verify(&kp.public_key, b"message-b", &sig).is_err());
    }

    #[test]
    fn verify_rejects_wrong_pubkey() {
        let kp1 = keygen_from_seed(&[9u8; 32]);
        let kp2 = keygen_from_seed(&[10u8; 32]);
        let sig = sign(&kp1.private_key, b"hello");
        assert!(verify(&kp2.public_key, b"hello", &sig).is_err());
    }

    #[test]
    fn verify_rejects_tampered_signature() {
        let kp = keygen_from_seed(&[11u8; 32]);
        let msg = b"tamper-test";
        let mut sig = sign(&kp.private_key, msg);
        sig.0[100] ^= 0xFF;
        assert!(verify(&kp.public_key, msg, &sig).is_err());
    }

    #[test]
    fn hedged_sign_also_verifies() {
        let kp = keygen_from_seed(&[12u8; 32]);
        let msg = b"hedged-signing-test";
        let sig = sign_hedged(&kp.private_key, msg, &mut OsRng);
        assert!(verify(&kp.public_key, msg, &sig).is_ok());
    }

    #[test]
    fn key_sizes_correct() {
        let kp = keygen_from_seed(&[0u8; 32]);
        assert_eq!(kp.public_key.0.len(), DILITHIUM3_PK_SIZE);
        assert_eq!(kp.private_key.0.len(), DILITHIUM3_SK_SIZE);
    }

    #[test]
    fn signature_size_correct() {
        let kp = keygen_from_seed(&[5u8; 32]);
        let sig = sign(&kp.private_key, b"size-check");
        assert_eq!(sig.0.len(), DILITHIUM3_SIG_SIZE);
    }

    #[test]
    fn proof_of_possession_verifies() {
        let kp = keygen_from_seed(&[13u8; 32]);
        let addr = [3u8; 20];
        let pop = proof_of_possession(&kp.private_key, &addr);
        let mut msg = b"zbx-pop-v1".to_vec();
        msg.extend_from_slice(&addr);
        assert!(verify(&kp.public_key, &msg, &pop).is_ok());
    }
}
