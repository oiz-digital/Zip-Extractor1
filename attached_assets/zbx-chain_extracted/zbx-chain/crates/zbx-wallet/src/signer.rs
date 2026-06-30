//! secp256k1 ECDSA signing for ZBX Chain.
//!
//! Implements:
//! - EIP-155: transaction signing with replay protection (v = chain_id*2+35+rec)
//! - EIP-191: personal_sign  ("\x19Ethereum Signed Message:\n" + len + msg)
//! - EIP-712: typed data     ("\x19\x01" + domain_sep + struct_hash)
//! - EIP-55:  checksum address (mixed-case hex based on keccak nibbles)
//!
//! All ECDSA operations use k256 (pure-Rust secp256k1) with low-S normalization
//! and deterministic RFC 6979 nonce generation.

use k256::ecdsa::{SigningKey, RecoveryId, Signature as EcdsaSig};
use k256::ecdsa::signature::hazmat::PrehashSigner;
use k256::SecretKey;
use k256::elliptic_curve::sec1::ToEncodedPoint;
use sha3::{Keccak256, Digest};
use crate::create_import::WalletError;

// ── Public key derivation ─────────────────────────────────────────────────────

/// Derive the uncompressed secp256k1 public key (65 bytes: 0x04 || X || Y).
pub fn public_key_uncompressed(private_key: &[u8; 32]) -> Result<[u8; 65], WalletError> {
    let sk = SecretKey::from_bytes(private_key.into())
        .map_err(|_| WalletError::InvalidPrivateKey)?;
    let encoded = sk.public_key().to_encoded_point(false); // uncompressed
    let mut out = [0u8; 65];
    out.copy_from_slice(encoded.as_bytes());
    Ok(out)
}

/// Derive the compressed secp256k1 public key (33 bytes: 0x02/0x03 || X).
pub fn public_key_compressed(private_key: &[u8; 32]) -> Result<[u8; 33], WalletError> {
    let sk = SecretKey::from_bytes(private_key.into())
        .map_err(|_| WalletError::InvalidPrivateKey)?;
    let encoded = sk.public_key().to_encoded_point(true); // compressed
    let mut out = [0u8; 33];
    out.copy_from_slice(encoded.as_bytes());
    Ok(out)
}

// ── Address derivation ────────────────────────────────────────────────────────

/// Compute the EVM address from an uncompressed public key (65 bytes).
///
/// address = keccak256(pubkey[1..])[12..]
///   - Skip the 0x04 prefix (1 byte)
///   - Hash the remaining 64 bytes (X || Y)
///   - Take the last 20 bytes of the 32-byte hash
pub fn evm_address_from_pubkey(pubkey_uncompressed: &[u8; 65]) -> [u8; 20] {
    let hash = Keccak256::digest(&pubkey_uncompressed[1..]);
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&hash[12..]);
    addr
}

/// Derive the EVM address directly from a private key.
pub fn address_from_key(private_key: &[u8; 32]) -> Result<[u8; 20], WalletError> {
    let pubkey = public_key_uncompressed(private_key)?;
    Ok(evm_address_from_pubkey(&pubkey))
}

/// Compute the EIP-55 checksum address (mixed-case hex, 0x-prefixed).
///
/// For each hex character at position i:
///   - Digit (0–9): output as-is
///   - Letter (a–f): uppercase if nibble i in keccak256(lowercase_address) ≥ 8
pub fn eip55_checksum(addr: &[u8; 20]) -> String {
    let hex_lower = hex::encode(addr);
    let hash = Keccak256::digest(hex_lower.as_bytes());
    let mut out = String::with_capacity(42);
    out.push_str("0x");
    for (i, c) in hex_lower.chars().enumerate() {
        if c.is_ascii_digit() {
            out.push(c);
        } else {
            let nibble = if i % 2 == 0 {
                (hash[i / 2] >> 4) & 0xf // high nibble
            } else {
                hash[i / 2] & 0xf         // low nibble
            };
            if nibble >= 8 {
                out.push(c.to_ascii_uppercase());
            } else {
                out.push(c);
            }
        }
    }
    out
}

// ── Signing ───────────────────────────────────────────────────────────────────

/// Sign a pre-hashed 32-byte message (no chain ID, legacy format).
///
/// Returns 65-byte signature [r(32) || s(32) || v(1)], v ∈ {27, 28}.
/// Use this for off-chain message signing; for transactions use `sign_hash_eip155`.
pub fn sign_hash(private_key: &[u8; 32], hash: &[u8; 32]) -> Result<[u8; 65], WalletError> {
    let sk = SigningKey::from_bytes(private_key.into())
        .map_err(|_| WalletError::InvalidPrivateKey)?;
    let (sig, recid): (EcdsaSig, RecoveryId) = sk
        .sign_prehash_recoverable(hash)
        .map_err(|_| WalletError::SigningFailed)?;
    let mut out = [0u8; 65];
    out[..32].copy_from_slice(sig.r().to_bytes().as_slice());
    out[32..64].copy_from_slice(sig.s().to_bytes().as_slice());
    out[64] = recid.to_byte() + 27;
    Ok(out)
}

/// Sign a transaction hash with EIP-155 replay protection.
///
/// Returns 65-byte signature [r(32) || s(32) || v(1)]
/// v = chain_id * 2 + 35 + recovery_id
///
/// For ZBX Chain:
///   Mainnet (8989): v ∈ {17,978 + recid} = {17978, 17979}
///   Testnet (8990): v ∈ {17,980 + recid} = {17980, 17981}
pub fn sign_hash_eip155(
    private_key: &[u8; 32],
    hash:        &[u8; 32],
    chain_id:    u64,
) -> Result<[u8; 65], WalletError> {
    let sk = SigningKey::from_bytes(private_key.into())
        .map_err(|_| WalletError::InvalidPrivateKey)?;
    let (sig, recid): (EcdsaSig, RecoveryId) = sk
        .sign_prehash_recoverable(hash)
        .map_err(|_| WalletError::SigningFailed)?;
    // EIP-155: v = chain_id * 2 + 35 + recovery_id
    let v = chain_id
        .checked_mul(2)
        .and_then(|x| x.checked_add(35))
        .and_then(|x| x.checked_add(u64::from(recid.to_byte())))
        .ok_or(WalletError::SigningFailed)?;
    let mut out = [0u8; 65];
    out[..32].copy_from_slice(sig.r().to_bytes().as_slice());
    out[32..64].copy_from_slice(sig.s().to_bytes().as_slice());
    out[64] = v as u8;
    Ok(out)
}

/// EIP-191 personal_sign: sign a human-readable message.
///
/// Hash: keccak256("\x19Ethereum Signed Message:\n" + len_str + message)
///
/// Returns 65-byte signature [r || s || v] with v ∈ {27, 28}.
pub fn personal_sign(private_key: &[u8; 32], message: &[u8]) -> Result<[u8; 65], WalletError> {
    let prefix = format!("\x19Ethereum Signed Message:\n{}", message.len());
    let hash: [u8; 32] = {
        let mut h = Keccak256::new();
        h.update(prefix.as_bytes());
        h.update(message);
        h.finalize().into()
    };
    sign_hash(private_key, &hash)
}

/// EIP-712 typed data signing.
///
/// Hash: keccak256("\x19\x01" || domain_separator || struct_hash)
///
/// Returns 65-byte signature [r || s || v] with v ∈ {27, 28}.
pub fn sign_typed_data(
    private_key: &[u8; 32],
    domain_sep:  &[u8; 32],
    struct_hash: &[u8; 32],
) -> Result<[u8; 65], WalletError> {
    let hash: [u8; 32] = {
        let mut h = Keccak256::new();
        h.update(b"\x19\x01");
        h.update(domain_sep);
        h.update(struct_hash);
        h.finalize().into()
    };
    sign_hash(private_key, &hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha3::{Keccak256, Digest};

    fn test_key() -> [u8; 32] {
        let mut k = [0u8; 32];
        k[31] = 1;
        k
    }

    #[test]
    fn pubkey_uncompressed_is_65_bytes() {
        let pk = public_key_uncompressed(&test_key()).unwrap();
        assert_eq!(pk.len(), 65);
        assert_eq!(pk[0], 0x04);
    }

    #[test]
    fn pubkey_compressed_is_33_bytes() {
        let pk = public_key_compressed(&test_key()).unwrap();
        assert_eq!(pk.len(), 33);
        assert!(pk[0] == 0x02 || pk[0] == 0x03);
    }

    #[test]
    fn address_derivation_is_deterministic() {
        let a1 = address_from_key(&test_key()).unwrap();
        let a2 = address_from_key(&test_key()).unwrap();
        assert_eq!(a1, a2);
    }

    #[test]
    fn eip55_checksum_prefix() {
        let addr = address_from_key(&test_key()).unwrap();
        let cs = eip55_checksum(&addr);
        assert!(cs.starts_with("0x"));
        assert_eq!(cs.len(), 42);
    }

    #[test]
    fn sign_hash_returns_65_bytes() {
        let hash = [0xabu8; 32];
        let sig = sign_hash(&test_key(), &hash).unwrap();
        assert_eq!(sig.len(), 65);
        assert!(sig[64] == 27 || sig[64] == 28);
    }

    #[test]
    fn sign_hash_eip155_v_encodes_chain_id() {
        let hash = [0x01u8; 32];
        let chain_id = 8990u64;
        let sig = sign_hash_eip155(&test_key(), &hash, chain_id).unwrap();
        let v = sig[64] as u64;
        // v = chain_id*2 + 35 + recid (recid 0 or 1)
        assert!(v == chain_id * 2 + 35 || v == chain_id * 2 + 36);
    }

    #[test]
    fn personal_sign_deterministic() {
        let msg = b"hello zebvix";
        let s1 = personal_sign(&test_key(), msg).unwrap();
        let s2 = personal_sign(&test_key(), msg).unwrap();
        assert_eq!(s1, s2);
    }

    #[test]
    fn sign_typed_data_different_from_personal_sign() {
        let msg = b"test message";
        let hash: [u8; 32] = Keccak256::digest(msg).into();
        let dom_sep = [0x11u8; 32];
        let struct_hash = [0x22u8; 32];
        let s1 = personal_sign(&test_key(), msg).unwrap();
        let s2 = sign_typed_data(&test_key(), &dom_sep, &struct_hash).unwrap();
        assert_ne!(s1, s2);
    }

    #[test]
    fn invalid_private_key_returns_error() {
        let zero_key = [0u8; 32];
        assert!(public_key_uncompressed(&zero_key).is_err());
    }
}
