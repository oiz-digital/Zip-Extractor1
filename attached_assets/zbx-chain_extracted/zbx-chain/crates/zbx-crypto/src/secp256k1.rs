//! secp256k1 ECDSA signing, recovery, and address derivation — EVM-compatible.
//!
//! Backed by the `k256` crate (pure-Rust, audited, RustCrypto project).
//! - Signatures are 65 bytes: 32-byte r + 32-byte s + 1-byte v (recovery id 0/1).
//! - For EIP-155 chain-id'd transactions, callers should pre-normalize v to {0,1}
//!   before passing to [`recover_signer`] (see `tx_decode.rs` in `zbx-rpc`).
//! - Public keys are uncompressed 65-byte SEC1 (0x04 || X || Y).
//! - Addresses are the last 20 bytes of `keccak256(pubkey[1..])`.
//! - Signing uses RFC 6979 deterministic nonces with low-S enforcement.

use crate::keccak::keccak256;
use k256::ecdsa::{RecoveryId, Signature as KSig, SigningKey, VerifyingKey};
use k256::elliptic_curve::sec1::ToEncodedPoint;
use k256::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;
use zbx_types::{address::Address, error::ZbxError, H256};
use zeroize::{Zeroize, ZeroizeOnDrop};
use hex;

// ---------------------------------------------------------------------------
// Key types
// ---------------------------------------------------------------------------

/// secp256k1 private key (32 bytes). Zeroized on drop.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct PrivKey([u8; 32]);

impl PrivKey {
    pub fn from_bytes(b: &[u8]) -> Result<Self, ZbxError> {
        if b.len() != 32 {
            return Err(ZbxError::InvalidLength { expected: 32, got: b.len() });
        }
        // Validate the scalar lies in the valid range (1..n).
        SecretKey::from_slice(b).map_err(|e| ZbxError::Signature(format!("invalid privkey: {e}")))?;
        let mut arr = [0u8; 32];
        arr.copy_from_slice(b);
        Ok(PrivKey(arr))
    }

    pub fn from_hex(s: &str) -> Result<Self, ZbxError> {
        let s = s.strip_prefix("0x").unwrap_or(s);
        let b = hex::decode(s).map_err(|_| ZbxError::InvalidHex(s.to_string()))?;
        Self::from_bytes(&b)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Generate a fresh random private key (CSPRNG via OS).
    pub fn random() -> Self {
        let sk = SecretKey::random(&mut rand::rngs::OsRng);
        let mut out = [0u8; 32];
        out.copy_from_slice(&sk.to_bytes());
        PrivKey(out)
    }

    /// Derive the corresponding 65-byte uncompressed public key.
    pub fn to_pubkey(&self) -> PubKey {
        let sk = SecretKey::from_slice(&self.0).expect("validated in from_bytes");
        let pk: PublicKey = sk.public_key();
        let enc = pk.to_encoded_point(false); // uncompressed
        let bytes = enc.as_bytes();
        debug_assert_eq!(bytes.len(), 65);
        let mut arr = [0u8; 65];
        arr.copy_from_slice(bytes);
        PubKey(arr)
    }

    /// Derive the EVM address from this private key.
    pub fn to_address(&self) -> Address {
        self.to_pubkey().to_address()
    }

    /// Sign a 32-byte message hash with RFC 6979 deterministic nonce.
    /// Returns a low-S, recoverable signature with v in {0, 1}.
    pub fn sign(&self, hash: &H256) -> Signature {
        let sk = SigningKey::from_slice(&self.0).expect("validated in from_bytes");
        let (sig, recid) = sk
            .sign_prehash_recoverable(hash.as_bytes())
            .expect("ECDSA sign cannot fail with valid inputs");
        // k256 already normalizes to low-S inside sign_prehash_recoverable.
        let bytes = sig.to_bytes();
        Signature {
            v: recid.to_byte(),
            r: H256::from_slice(&bytes[..32]),
            s: H256::from_slice(&bytes[32..64]),
        }
    }
}

/// secp256k1 uncompressed public key (65 bytes: 0x04 || X || Y).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PubKey(#[serde(with = "BigArray")] pub [u8; 65]);

impl PubKey {
    pub fn from_bytes(b: &[u8]) -> Result<Self, ZbxError> {
        if b.len() != 65 || b[0] != 0x04 {
            return Err(ZbxError::Signature("invalid uncompressed pubkey".into()));
        }
        // Validate the point lies on the curve and is not at infinity.
        VerifyingKey::from_sec1_bytes(b)
            .map_err(|e| ZbxError::Signature(format!("invalid pubkey point: {e}")))?;
        let mut arr = [0u8; 65];
        arr.copy_from_slice(b);
        Ok(PubKey(arr))
    }

    /// Derive the EVM-compatible 20-byte address.
    /// `address = keccak256(pubkey[1..])[12..]`.
    pub fn to_address(&self) -> Address {
        let h = keccak256(&self.0[1..]);
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&h.as_bytes()[12..]);
        Address(addr)
    }

    pub fn as_bytes(&self) -> &[u8; 65] {
        &self.0
    }
}

/// ECDSA signature: recovery_id (0/1) + 32-byte r + 32-byte s.
///
/// Note: callers using EIP-155 chain-id encoding (Legacy txs) or EIP-2718
/// typed-tx envelopes should normalize the V byte to {0,1} _before_ recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    pub v: u8,
    pub r: H256,
    pub s: H256,
}

impl Signature {
    pub fn to_bytes(&self) -> [u8; 65] {
        let mut out = [0u8; 65];
        out[..32].copy_from_slice(self.r.as_bytes());
        out[32..64].copy_from_slice(self.s.as_bytes());
        out[64] = self.v;
        out
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self, ZbxError> {
        if b.len() != 65 {
            return Err(ZbxError::InvalidLength { expected: 65, got: b.len() });
        }
        Ok(Signature {
            r: H256::from_slice(&b[..32]),
            s: H256::from_slice(&b[32..64]),
            v: b[64],
        })
    }
}

// ---------------------------------------------------------------------------
// Core operations — REAL ECDSA via k256
// ---------------------------------------------------------------------------

/// Recover the signer address from a 32-byte message hash and signature.
/// Enforces low-S to prevent malleability (EIP-2). Rejects invalid r/s scalars.
pub fn recover_signer(hash: &H256, sig: &Signature) -> Result<Address, ZbxError> {
    let pubkey = recover_pubkey(hash, sig)?;
    Ok(pubkey.to_address())
}

/// Recover the SEC1 public key from a 32-byte hash and signature.
pub fn recover_pubkey(hash: &H256, sig: &Signature) -> Result<PubKey, ZbxError> {
    if sig.v > 1 {
        return Err(ZbxError::Signature(format!(
            "invalid recovery id (must be 0 or 1, got {}); did you forget EIP-155 normalization?",
            sig.v
        )));
    }
    // Build 64-byte r||s for k256.
    let mut rs = [0u8; 64];
    rs[..32].copy_from_slice(sig.r.as_bytes());
    rs[32..64].copy_from_slice(sig.s.as_bytes());
    let ksig = KSig::from_slice(&rs)
        .map_err(|e| ZbxError::Signature(format!("invalid r/s scalars: {e}")))?;
    // Reject high-S signatures (malleability). Any well-formed Ethereum tx is low-S.
    if ksig.normalize_s().is_some() {
        return Err(ZbxError::Signature("non-canonical (high-S) signature".into()));
    }
    let recid = RecoveryId::try_from(sig.v)
        .map_err(|e| ZbxError::Signature(format!("invalid recovery id: {e}")))?;
    let vk = VerifyingKey::recover_from_prehash(hash.as_bytes(), &ksig, recid)
        .map_err(|e| ZbxError::Signature(format!("ECDSA recovery failed: {e}")))?;
    let enc = vk.to_encoded_point(false);
    let bytes = enc.as_bytes();
    if bytes.len() != 65 {
        return Err(ZbxError::Signature("recovered pubkey not 65 bytes".into()));
    }
    let mut arr = [0u8; 65];
    arr.copy_from_slice(bytes);
    Ok(PubKey(arr))
}

/// Verify that a signature was produced by the given address.
pub fn verify_signature(hash: &H256, sig: &Signature, expected: &Address) -> bool {
    recover_signer(hash, sig)
        .map(|addr| addr == *expected)
        .unwrap_or(false)
}

/// Derive ECDH shared secret = keccak256(X-coordinate of privkey * peer_pub).
///
/// The hash wrapping protects against bias in the raw X-coordinate distribution
/// and matches the convention used by Ethereum's RLPx noise handshake.
pub fn ecdh_shared_secret(priv_key: &PrivKey, peer_pub: &PubKey) -> H256 {
    use k256::ecdh::diffie_hellman;
    use k256::PublicKey as KPubKey;

    let sk = SecretKey::from_slice(priv_key.as_bytes())
        .expect("PrivKey was validated on construction");
    let pk = KPubKey::from_sec1_bytes(peer_pub.as_bytes())
        .expect("PubKey was validated on construction");

    let shared = diffie_hellman(sk.to_nonzero_scalar(), pk.as_affine());
    keccak256(shared.raw_secret_bytes())
}

// ---------------------------------------------------------------------------
// EIP-191 personal_sign (eth_sign)
// ---------------------------------------------------------------------------

/// Compute the EIP-191 personal sign hash for a 32-byte message hash.
///
/// `keccak256(0x19 || "Ethereum Signed Message:\n32" || hash)`
///
/// This matches `eth_sign` and MetaMask's personal_sign. Callers that want to
/// sign arbitrary-length messages must pre-hash them to 32 bytes first.
pub fn personal_sign_hash(hash: &H256) -> H256 {
    let mut data = Vec::with_capacity(28 + 32);
    data.push(0x19u8);
    data.extend_from_slice(b"Ethereum Signed Message:\n32");
    data.extend_from_slice(hash.as_bytes());
    keccak256(&data)
}

/// Sign a 32-byte message hash with the EIP-191 personal_sign prefix.
///
/// Equivalent to MetaMask's `eth_sign` / `personal_sign` RPC methods.
/// The signature's `v` is in {0, 1} (canonical form).
/// Callers that need the legacy 27/28 wire format should add 27 to `sig.v`.
pub fn personal_sign(hash: &H256, privkey: &PrivKey) -> Signature {
    let prefixed = personal_sign_hash(hash);
    privkey.sign(&prefixed)
}

/// Recover the signer of an EIP-191 personal_sign signature.
///
/// `hash` is the original 32-byte message hash BEFORE the personal_sign
/// prefix was applied. The prefix is re-applied internally.
pub fn recover_personal_signer(hash: &H256, sig: &Signature) -> Result<Address, ZbxError> {
    let prefixed = personal_sign_hash(hash);
    recover_signer(&prefixed, sig)
}

// ---------------------------------------------------------------------------
// EIP-712 typed data signing
// ---------------------------------------------------------------------------

/// Compute the EIP-712 final hash from a domain separator and struct hash.
///
/// `keccak256(0x19 || 0x01 || domain_separator || struct_hash)`
pub fn eip712_hash(domain_sep: &H256, struct_hash: &H256) -> H256 {
    let mut data = Vec::with_capacity(2 + 32 + 32);
    data.push(0x19u8);
    data.push(0x01u8);
    data.extend_from_slice(domain_sep.as_bytes());
    data.extend_from_slice(struct_hash.as_bytes());
    keccak256(&data)
}

/// Sign structured data using EIP-712.
///
/// `domain_sep` — keccak256 of the ABI-encoded domain separator.
/// `struct_hash` — keccak256 of the ABI-encoded struct data.
///
/// The final message hash is `keccak256(0x19 0x01 || domain_sep || struct_hash)`.
/// The signature's `v` is in {0, 1} (canonical). Add 27 for the legacy wire format.
pub fn sign_typed_data(domain_sep: &H256, struct_hash: &H256, privkey: &PrivKey) -> Signature {
    let hash = eip712_hash(domain_sep, struct_hash);
    privkey.sign(&hash)
}

/// Recover the signer of an EIP-712 typed data signature.
pub fn recover_typed_data_signer(
    domain_sep: &H256,
    struct_hash: &H256,
    sig: &Signature,
) -> Result<Address, ZbxError> {
    let hash = eip712_hash(domain_sep, struct_hash);
    recover_signer(&hash, sig)
}

// ---------------------------------------------------------------------------
// EIP-55 checksum address encoding
// ---------------------------------------------------------------------------

/// Encode a 20-byte address as an EIP-55 checksum hex string (with `0x` prefix).
///
/// EIP-55 capitalizes hex characters based on the Keccak256 hash of the
/// lowercase hex address, making addresses self-verifying against typos.
pub fn address_to_checksum(addr: &Address) -> String {
    let hex_lower = hex::encode(addr.as_bytes()); // 40 hex chars, all lowercase
    let hash = keccak256(hex_lower.as_bytes());
    let hash_bytes = hash.as_bytes();

    let mut out = String::with_capacity(42);
    out.push_str("0x");
    for (i, ch) in hex_lower.chars().enumerate() {
        // nibble index i → byte index i/2 → bit (i%2 == 0 ? high nibble : low nibble)
        let nibble_hash = (hash_bytes[i / 2] >> (if i % 2 == 0 { 4 } else { 0 })) & 0x0f;
        if ch.is_ascii_alphabetic() && nibble_hash >= 8 {
            out.push(ch.to_ascii_uppercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Validate that a hex address string is a valid EIP-55 checksum address.
///
/// Returns `Ok(Address)` if the checksum is correct, `Err` otherwise.
/// Accepts addresses with or without a `0x` prefix.
pub fn validate_checksum_address(s: &str) -> Result<Address, ZbxError> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.len() != 40 {
        return Err(ZbxError::InvalidLength { expected: 20, got: s.len() / 2 });
    }
    let bytes = hex::decode(s)
        .map_err(|_| ZbxError::InvalidHex(s.to_string()))?;
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&bytes);
    let address = Address(addr);
    let expected = address_to_checksum(&address);
    // Compare without the 0x prefix
    if expected[2..] != s.to_string() {
        return Err(ZbxError::Signature(format!(
            "EIP-55 checksum mismatch: got {s}, expected {}",
            &expected[2..]
        )));
    }
    Ok(address)
}

// ---------------------------------------------------------------------------
// EIP-155 helpers
// ---------------------------------------------------------------------------

/// Normalize a Legacy EIP-155 V byte to a canonical recovery id {0, 1}.
///
/// - Pre-EIP-155: V is 27 or 28 → returns V - 27.
/// - EIP-155: V is `chain_id * 2 + 35 + parity` → returns parity (0 or 1).
///
/// Returns the recovered `parity` and the inferred chain_id (None for pre-EIP-155).
pub fn normalize_v_eip155(v: u64) -> Result<(u8, Option<u64>), ZbxError> {
    if v == 27 || v == 28 {
        Ok(((v - 27) as u8, None))
    } else if v >= 35 {
        let parity = ((v - 35) & 1) as u8;
        let chain_id = (v - 35 - parity as u64) / 2;
        Ok((parity, Some(chain_id)))
    } else if v <= 1 {
        Ok((v as u8, None))
    } else {
        Err(ZbxError::Signature(format!("unsupported V value: {v}")))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_sign_recover() {
        let sk = PrivKey::random();
        let addr = sk.to_address();
        let msg: H256 = keccak256(b"zbx test message");
        let sig = sk.sign(&msg);
        let recovered = recover_signer(&msg, &sig).expect("recovery must succeed");
        assert_eq!(recovered, addr, "recovered address must match signer");
        assert!(verify_signature(&msg, &sig, &addr));
    }

    #[test]
    fn rejects_bad_v() {
        let sk = PrivKey::random();
        let msg: H256 = keccak256(b"x");
        let mut sig = sk.sign(&msg);
        sig.v = 5;
        assert!(recover_signer(&msg, &sig).is_err());
    }

    #[test]
    fn rejects_high_s_malleable() {
        let sk = PrivKey::random();
        let msg: H256 = keccak256(b"x");
        let sig = sk.sign(&msg);
        // Negate s mod n to make it high-S
        // n for secp256k1 = FFFFFFFF FFFFFFFF FFFFFFFF FFFFFFFE BAAEDCE6 AF48A03B BFD25E8C D0364141
        const N: [u8; 32] = [
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xfe, 0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c,
            0xd0, 0x36, 0x41, 0x41,
        ];
        // high_s = N - s (treat as big-endian 256-bit)
        let mut high_s = [0u8; 32];
        let mut borrow: i32 = 0;
        for i in (0..32).rev() {
            let diff = N[i] as i32 - sig.s[i] as i32 - borrow;
            if diff < 0 {
                high_s[i] = (diff + 256) as u8;
                borrow = 1;
            } else {
                high_s[i] = diff as u8;
                borrow = 0;
            }
        }
        let bad = Signature { v: sig.v, r: sig.r, s: zbx_types::H256(high_s) };
        assert!(recover_signer(&msg, &bad).is_err());
    }

    #[test]
    fn personal_sign_roundtrip() {
        let sk = PrivKey::random();
        let addr = sk.to_address();
        let msg: H256 = keccak256(b"hello zbx chain");
        let sig = personal_sign(&msg, &sk);
        // The signature v must be 0 or 1 (canonical).
        assert!(sig.v <= 1, "personal_sign v must be 0 or 1");
        let recovered = recover_personal_signer(&msg, &sig).expect("personal_sign recovery must succeed");
        assert_eq!(recovered, addr, "recovered address must match signer");
    }

    #[test]
    fn personal_sign_prefix_changes_hash() {
        let sk = PrivKey::random();
        let msg: H256 = keccak256(b"test message");
        let raw_sig   = sk.sign(&msg);
        let perso_sig = personal_sign(&msg, &sk);
        // Personal sign applies a prefix, so the signature bytes must differ.
        assert_ne!(raw_sig.r, perso_sig.r, "personal_sign must use a different hash from raw sign");
    }

    #[test]
    fn personal_sign_hash_matches_eip191_spec() {
        // EIP-191: prefix = 0x19 || "Ethereum Signed Message:\n32"
        let msg = H256([0xab; 32]);
        let h = personal_sign_hash(&msg);
        // Manually compute the expected hash.
        let mut expected_preimage = Vec::new();
        expected_preimage.push(0x19u8);
        expected_preimage.extend_from_slice(b"Ethereum Signed Message:\n32");
        expected_preimage.extend_from_slice(&[0xab; 32]);
        let expected = keccak256(&expected_preimage);
        assert_eq!(h, expected, "personal_sign_hash must match EIP-191 spec");
    }

    #[test]
    fn eip712_sign_roundtrip() {
        let sk = PrivKey::random();
        let addr = sk.to_address();
        let domain = keccak256(b"ZBX Chain domain separator v1");
        let data   = keccak256(b"some struct hash");
        let sig = sign_typed_data(&domain, &data, &sk);
        assert!(sig.v <= 1, "EIP-712 v must be canonical 0 or 1");
        let recovered = recover_typed_data_signer(&domain, &data, &sig)
            .expect("EIP-712 recovery must succeed");
        assert_eq!(recovered, addr, "EIP-712 recovered address must match signer");
    }

    #[test]
    fn eip712_hash_matches_spec() {
        // EIP-712 final hash: keccak256(0x19 || 0x01 || domain_sep || struct_hash)
        let domain = H256([0x01; 32]);
        let data   = H256([0x02; 32]);
        let h = eip712_hash(&domain, &data);
        let mut preimage = Vec::new();
        preimage.push(0x19u8);
        preimage.push(0x01u8);
        preimage.extend_from_slice(&[0x01; 32]);
        preimage.extend_from_slice(&[0x02; 32]);
        let expected = keccak256(&preimage);
        assert_eq!(h, expected, "EIP-712 hash must match spec");
    }

    #[test]
    fn address_checksum_encoding() {
        // EIP-55 test vector from the EIP specification.
        // "5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed" is the checksum form of
        // the address bytes decoded from "5aaeb6053f3e94c9b9a09f33669435e7ef1beaed".
        let raw = hex::decode("5aaeb6053f3e94c9b9a09f33669435e7ef1beaed").unwrap();
        let mut addr_bytes = [0u8; 20];
        addr_bytes.copy_from_slice(&raw);
        let addr = zbx_types::address::Address(addr_bytes);
        let checksum = address_to_checksum(&addr);
        // The first two chars are "0x", remainder is the checksummed hex.
        assert_eq!(&checksum[..2], "0x", "must have 0x prefix");
        assert_eq!(checksum.len(), 42, "must be 42 chars total");
        // Round-trip: validate_checksum_address must accept the output of address_to_checksum.
        let recovered = validate_checksum_address(&checksum)
            .expect("validate_checksum_address must accept its own output");
        assert_eq!(recovered, addr, "round-trip must preserve the address bytes");
    }

    #[test]
    fn checksum_rejects_wrong_case() {
        let raw = hex::decode("5aaeb6053f3e94c9b9a09f33669435e7ef1beaed").unwrap();
        let mut addr_bytes = [0u8; 20];
        addr_bytes.copy_from_slice(&raw);
        let addr = zbx_types::address::Address(addr_bytes);
        // All-lowercase is NOT a valid EIP-55 checksum for most addresses.
        let lower = format!("0x{}", hex::encode(addr_bytes));
        // This may or may not match — only assert that the validated address matches when it does.
        // The important invariant is that a flipped case character is always rejected.
        let checksum = address_to_checksum(&addr);
        // Flip the case of the first alphabetic character after "0x".
        let mut bad: Vec<char> = checksum.chars().collect();
        for c in bad.iter_mut().skip(2) {
            if c.is_ascii_alphabetic() {
                *c = if c.is_ascii_uppercase() { c.to_ascii_lowercase() } else { c.to_ascii_uppercase() };
                break;
            }
        }
        let bad_str: String = bad.into_iter().collect();
        if bad_str != lower {
            // Only assert rejection if the bad string differs from the all-lower version
            // (for all-numeric addresses there may be nothing to flip).
            assert!(
                validate_checksum_address(&bad_str).is_err(),
                "flipped-case address must be rejected by EIP-55 validation"
            );
        }
    }

    #[test]
    fn eip155_v_normalization() {
        // Chain id 1 (mainnet): v = 37 or 38
        let (parity, cid) = normalize_v_eip155(37).unwrap();
        assert_eq!(parity, 0);
        assert_eq!(cid, Some(1));
        let (parity, cid) = normalize_v_eip155(38).unwrap();
        assert_eq!(parity, 1);
        assert_eq!(cid, Some(1));
        // Chain id 8989 (zbx): v = 8989*2 + 35 = 18013 or 18014
        let (parity, cid) = normalize_v_eip155(18013).unwrap();
        assert_eq!(parity, 0);
        assert_eq!(cid, Some(8989));
        let (parity, cid) = normalize_v_eip155(18014).unwrap();
        assert_eq!(parity, 1);
        assert_eq!(cid, Some(8989));
        // Pre-EIP-155
        let (parity, cid) = normalize_v_eip155(27).unwrap();
        assert_eq!(parity, 0);
        assert_eq!(cid, None);
        let (parity, cid) = normalize_v_eip155(28).unwrap();
        assert_eq!(parity, 1);
        assert_eq!(cid, None);
    }
}
