//! secp256k1 key management, ECDSA signing, and address derivation.
//!
//! This module is a **thin compatibility shim** over [`zbx_crypto::secp256k1`].
//! All cryptographic work — RFC 6979 nonce generation, low-S normalisation
//! during signing, low-S enforcement during recovery, and v-byte handling —
//! lives in `zbx-crypto` so there is exactly one signer implementation in the
//! workspace.
//!
//! Audit-2026-05-01:
//!  * S7-SDK1 (HIGH): the previous `recover_address` used
//!    `RecoveryId::from_byte(v.saturating_sub(27))`, which silently mapped
//!    `v ∈ {0..=27}` to recovery id `0` and `v=1` to `0` instead of `1`.
//!    Half of all valid EIP-2098 raw signatures recovered to the wrong
//!    address. Now delegated to `zbx_crypto::normalize_v_eip155` +
//!    `zbx_crypto::recover_signer`, which handles {0,1}, {27,28},
//!    EIP-155 chain-encoded v, and rejects everything else.
//!  * S7-SDK2 (HIGH): the SDK signer never enforced low-S; a malicious
//!    relayer could flip s → n-s and forge a "different" signature that
//!    recovers to the same address. `zbx_crypto::recover_signer` now
//!    rejects high-S, and `zbx_crypto::PrivKey::sign` produces low-S
//!    output by construction (k256's `sign_prehash_recoverable`).
//!  * S7-CR1b (HIGH): EIP-191 personal-sign prefix was a literal
//!    `\\x19...\\n32` string instead of the byte 0x19 + LF + "32".
//!    Fixed below in `personal_sign_hash`.
//!
//! The public API surface (`KeyPair`, `sign_hash` returning v∈{27,28},
//! `personal_sign`, `sign_typed_data`, `recover_address`, `derive_address`,
//! `keccak256`) is preserved for existing callers.

use k256::{
    ecdsa::{SigningKey, VerifyingKey},
    elliptic_curve::sec1::ToEncodedPoint,
    SecretKey,
};
use sha3::{Digest, Keccak256};
use crate::error::SdkError;
use zbx_types::{Address, H256};
use zbx_crypto::secp256k1 as zsec;

/// A secp256k1 key pair.
pub struct KeyPair {
    pub signing:   SigningKey,
    pub verifying: VerifyingKey,
    pub address:   Address,
}

impl KeyPair {
    /// Create from a raw 32-byte private key scalar.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, SdkError> {
        let secret = SecretKey::from_bytes(bytes.into())
            .map_err(|e| SdkError::InvalidKey(e.to_string()))?;
        let signing   = SigningKey::from(&secret);
        let verifying = VerifyingKey::from(&signing);
        let address   = derive_address(&verifying);
        Ok(Self { signing, verifying, address })
    }

    /// Generate a random key pair using OS entropy.
    pub fn random() -> Self {
        let secret  = SecretKey::random(&mut rand::thread_rng());
        let signing   = SigningKey::from(&secret);
        let verifying = VerifyingKey::from(&signing);
        let address   = derive_address(&verifying);
        Self { signing, verifying, address }
    }

    /// Sign a 32-byte message hash.  Returns the 65-byte signature
    /// `[r(32) | s(32) | v(1)]`. `v` is 27 or 28 (legacy Ethereum convention).
    ///
    /// Internally delegates to `zbx_crypto::PrivKey::sign`, which produces a
    /// low-S signature with v ∈ {0,1}; we add +27 here to preserve the legacy
    /// 65-byte wire format the SDK has always returned.
    pub fn sign_hash(&self, hash: &H256) -> Result<[u8; 65], SdkError> {
        let priv_bytes = self.signing.to_bytes();
        let sk = zsec::PrivKey::from_bytes(priv_bytes.as_slice())
            .map_err(|e| SdkError::Signing(e.to_string()))?;
        let sig = sk.sign(hash);
        // sig.v is 0 or 1 (canonical). Convert to legacy 27/28.
        let mut out = [0u8; 65];
        out[..32].copy_from_slice(sig.r.as_bytes());
        out[32..64].copy_from_slice(sig.s.as_bytes());
        out[64] = sig.v + 27;
        Ok(out)
    }

    /// Sign a message with the Ethereum personal sign prefix.
    /// `eth_sign` message: keccak256(0x19 || "Ethereum Signed Message:\n32" || hash)
    pub fn personal_sign(&self, hash: &H256) -> Result<[u8; 65], SdkError> {
        let prefixed = personal_sign_hash(hash);
        self.sign_hash(&prefixed)
    }

    /// EIP-712 structured data signing.
    pub fn sign_typed_data(&self, domain_sep: &H256, struct_hash: &H256) -> Result<[u8; 65], SdkError> {
        let final_hash = eip712_hash(domain_sep, struct_hash);
        self.sign_hash(&final_hash)
    }
}

/// Derive an Ethereum-compatible address from a secp256k1 public key.
/// `address = keccak256(uncompressed_pubkey[1..])[12..]`
pub fn derive_address(verifying: &VerifyingKey) -> Address {
    let point      = verifying.to_encoded_point(false);
    let pubkey_bytes = &point.as_bytes()[1..]; // strip 0x04 prefix
    let hash       = Keccak256::digest(pubkey_bytes);
    let mut addr   = [0u8; 20];
    addr.copy_from_slice(&hash[12..]);
    Address(addr)
}

/// Recover the signer address from a message hash and 65-byte signature.
///
/// Audit-2026-05-01 S7-SDK1 + S7-SDK2:
///   * Accepts v ∈ {0, 1} (raw EIP-2098), {27, 28} (legacy), and
///     `chain_id*2 + 35 + parity` (EIP-155) — via
///     [`zbx_crypto::normalize_v_eip155`].
///   * Enforces low-S (rejects malleable high-S signatures) — via
///     [`zbx_crypto::recover_signer`].
///   * Rejects every other v value with `SdkError::InvalidSignature`.
pub fn recover_address(hash: &H256, sig: &[u8; 65]) -> Result<Address, SdkError> {
    let (parity, _chain_id) = zsec::normalize_v_eip155(sig[64] as u64)
        .map_err(|e| SdkError::InvalidSignature(format!("bad v value: {e}")))?;
    let zsig = zsec::Signature {
        v: parity,
        r: H256::from_slice(&sig[..32]),
        s: H256::from_slice(&sig[32..64]),
    };
    zsec::recover_signer(hash, &zsig)
        .map_err(|e| SdkError::InvalidSignature(e.to_string()))
}

/// Keccak256 hash of arbitrary bytes.
pub fn keccak256(data: &[u8]) -> H256 {
    let hash = Keccak256::digest(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(&hash);
    H256(out)
}

/// EIP-191 personal sign hash.
///
/// Audit-2026-05-01 S7-CR1b: previous `format!("\\x19...\\n32")` was the
/// literal 5+3-char string `\x19...\n32` instead of the byte 0x19 + LF + "32"
/// digits. Every signature produced or recovered through this helper was
/// incompatible with `eth_sign` and any standard wallet.
fn personal_sign_hash(hash: &H256) -> H256 {
    let mut data = Vec::with_capacity(28 + 32);
    data.push(0x19);
    data.extend_from_slice(b"Ethereum Signed Message:\n32");
    data.extend_from_slice(hash.as_bytes());
    keccak256(&data)
}

/// EIP-712 typed data hash.
fn eip712_hash(domain_sep: &H256, struct_hash: &H256) -> H256 {
    let mut data = vec![0x19u8, 0x01];
    data.extend_from_slice(domain_sep.as_bytes());
    data.extend_from_slice(struct_hash.as_bytes());
    keccak256(&data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keypair_random() {
        let kp = KeyPair::random();
        assert_ne!(kp.address, Address::zero());
    }

    #[test]
    fn test_sign_and_recover() {
        let kp = KeyPair::random();
        let hash = H256([0x42; 32]);
        let sig = kp.sign_hash(&hash).unwrap();
        let recovered = recover_address(&hash, &sig).unwrap();
        assert_eq!(recovered, kp.address);
    }

    #[test]
    fn test_v_is_27_or_28() {
        let kp = KeyPair::random();
        let hash = H256([0xAB; 32]);
        let sig = kp.sign_hash(&hash).unwrap();
        assert!(sig[64] == 27 || sig[64] == 28);
    }

    #[test]
    fn test_derive_address_is_deterministic() {
        let bytes = [0x01u8; 32];
        let kp1 = KeyPair::from_bytes(&bytes).unwrap();
        let kp2 = KeyPair::from_bytes(&bytes).unwrap();
        assert_eq!(kp1.address, kp2.address);
    }

    /// S7-SDK1 regression: the previous `saturating_sub(27)` mapped both
    /// v=0 and v=1 (raw EIP-2098) to recovery id 0, so half of all raw-v
    /// signatures recovered to the wrong address. The fix routes through
    /// `normalize_v_eip155`, which preserves parity for v ∈ {0,1}.
    #[test]
    fn test_recover_accepts_raw_v_0_and_1() {
        let kp = KeyPair::random();
        let hash = H256([0xCD; 32]);
        let mut sig = kp.sign_hash(&hash).unwrap();
        // sig[64] ∈ {27,28}; rewrite to raw {0,1} and recovery must still match.
        sig[64] -= 27;
        let recovered = recover_address(&hash, &sig).unwrap();
        assert_eq!(recovered, kp.address, "raw v∈{{0,1}} must recover correctly");
    }

    /// S7-SDK1 regression: v values in (1, 27) and (28, 35) must be rejected.
    #[test]
    fn test_recover_rejects_invalid_v() {
        let kp = KeyPair::random();
        let hash = H256([0xEF; 32]);
        let mut sig = kp.sign_hash(&hash).unwrap();
        for bad_v in [2u8, 5, 10, 26, 29, 34] {
            sig[64] = bad_v;
            assert!(
                recover_address(&hash, &sig).is_err(),
                "v={bad_v} must be rejected"
            );
        }
    }

    /// S7-SDK2 regression: a high-S forgery of a valid signature must be
    /// rejected even though k256 would happily recover the original signer
    /// from it. low-S enforcement lives in `zbx_crypto::recover_signer`.
    #[test]
    fn test_recover_rejects_high_s_malleable() {
        const N: [u8; 32] = [
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xfe, 0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c,
            0xd0, 0x36, 0x41, 0x41,
        ];
        let kp = KeyPair::random();
        let hash = H256([0x77; 32]);
        let sig = kp.sign_hash(&hash).unwrap();
        let mut high = sig;
        // high.s = N - sig.s, big-endian 256-bit
        let mut borrow: i32 = 0;
        for i in (0..32).rev() {
            let diff = N[i] as i32 - sig[32 + i] as i32 - borrow;
            if diff < 0 {
                high[32 + i] = (diff + 256) as u8;
                borrow = 1;
            } else {
                high[32 + i] = diff as u8;
                borrow = 0;
            }
        }
        // Flip parity to keep the forgery internally consistent (not strictly
        // required for the rejection test, but matches what an attacker would do).
        high[64] ^= 1;
        assert!(
            recover_address(&hash, &high).is_err(),
            "high-S forgery must be rejected"
        );
    }
}
