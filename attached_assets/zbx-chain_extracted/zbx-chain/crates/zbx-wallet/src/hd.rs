//! BIP-32 HD wallet key derivation with full chain code tracking.
//!
//! Implements the complete BIP-32 specification:
//! - Master key derivation from seed: HMAC-SHA512("Bitcoin seed", seed)
//! - Hardened child: HMAC-SHA512(chain_code, 0x00 || key || index_be)
//! - Normal child:   HMAC-SHA512(chain_code, compressed_pub || index_be)
//! - Path parsing:   "m/44'/7878'/account'/change/index"
//!
//! ## Key difference from naive implementations
//!
//! The **chain code** is tracked alongside the private key at every level.
//! A common stub error is using the parent private key as the HMAC input
//! instead of the parent chain code — this breaks cross-wallet compatibility.
//!
//! ## ZBX derivation path
//!   m/44'/7878'/account'/0/index   (external chain)
//!   m/44'/7878'/account'/1/index   (internal/change chain)
//!
//!   Coin type 7878 = SLIP-44 Zebvix, INDEPENDENT of chain IDs 8989/8990.

use hmac::{Hmac, Mac};
use sha2::Sha512;
use k256::{SecretKey, Scalar, FieldBytes};
use k256::elliptic_curve::{PrimeField, sec1::ToEncodedPoint};
use crate::create_import::WalletError;

type HmacSha512 = Hmac<Sha512>;

/// An extended private key: private key bytes + 32-byte chain code.
///
/// Both fields are required for full BIP-32 compliance. Discarding the chain
/// code after deriving the master key is a common bug that makes child keys
/// incompatible with all standard HD wallets.
#[derive(Clone)]
pub struct XKey {
    /// secp256k1 private key (32 bytes, must be in [1, n−1])
    pub key:        [u8; 32],
    /// BIP-32 chain code (32 bytes — entropy for child derivation)
    pub chain_code: [u8; 32],
}

impl XKey {
    /// Derive the BIP-32 master extended key from a 512-bit BIP-39 seed.
    ///
    /// I = HMAC-SHA512(key="Bitcoin seed", data=seed)
    /// master_key        = I[0..32]
    /// master_chain_code = I[32..64]
    pub fn from_seed(seed: &[u8; 64]) -> Result<Self, WalletError> {
        let mut mac = HmacSha512::new_from_slice(b"Bitcoin seed")
            .expect("HMAC-SHA512 key is always valid for static key");
        mac.update(seed.as_ref());
        let result = mac.finalize().into_bytes();
        let mut key        = [0u8; 32];
        let mut chain_code = [0u8; 32];
        key.copy_from_slice(&result[..32]);
        chain_code.copy_from_slice(&result[32..]);
        // A zero key is invalid per BIP-32; probability is astronomically low
        if key == [0u8; 32] {
            return Err(WalletError::DerivationFailed);
        }
        Ok(Self { key, chain_code })
    }

    /// Derive a hardened child key (index must be 0–2^31−1; index|0x80000000 sent).
    ///
    /// Data = 0x00 || parent_key || (index | 0x80000000) in big-endian
    /// I = HMAC-SHA512(parent_chain_code, data)
    /// child_key = (I_L + parent_key) mod n
    pub fn child_hardened(&self, index: u32) -> Result<Self, WalletError> {
        let hardened = index | 0x8000_0000u32;
        let mut mac = HmacSha512::new_from_slice(&self.chain_code)
            .map_err(|_| WalletError::DerivationFailed)?;
        mac.update(&[0x00]);
        mac.update(&self.key);
        mac.update(&hardened.to_be_bytes());
        let result = mac.finalize().into_bytes();
        let child_key = scalar_add(&self.key, &result[..32])?;
        let mut chain_code = [0u8; 32];
        chain_code.copy_from_slice(&result[32..]);
        Ok(Self { key: child_key, chain_code })
    }

    /// Derive a normal (non-hardened) child key.
    ///
    /// Data = compressed_parent_pubkey (33 bytes) || index in big-endian
    /// I = HMAC-SHA512(parent_chain_code, data)
    /// child_key = (I_L + parent_key) mod n
    pub fn child_normal(&self, index: u32) -> Result<Self, WalletError> {
        let compressed_pub = compressed_pubkey(&self.key)?;
        let mut mac = HmacSha512::new_from_slice(&self.chain_code)
            .map_err(|_| WalletError::DerivationFailed)?;
        mac.update(&compressed_pub);
        mac.update(&index.to_be_bytes());
        let result = mac.finalize().into_bytes();
        let child_key = scalar_add(&self.key, &result[..32])?;
        let mut chain_code = [0u8; 32];
        chain_code.copy_from_slice(&result[32..]);
        Ok(Self { key: child_key, chain_code })
    }

    /// Derive along an arbitrary BIP-32 path string.
    ///
    /// Examples:
    ///   "m/44'/7878'/0'/0/0"  — ZBX mainnet, account 0, external, index 0
    ///   "m/44'/60'/0'/0/0"    — Ethereum-compatible path
    ///   "m"                    — master key (returned as-is)
    pub fn derive_path(&self, path: &str) -> Result<Self, WalletError> {
        let segments = path.trim_start_matches("m/");
        if segments.is_empty() || segments == "m" {
            return Ok(self.clone());
        }
        let mut current = self.clone();
        for seg in segments.split('/') {
            let (hardened, idx_str) = if let Some(s) = seg.strip_suffix('\'') {
                (true, s)
            } else {
                (false, seg)
            };
            let idx: u32 = idx_str.parse().map_err(|_| WalletError::DerivationFailed)?;
            current = if hardened {
                current.child_hardened(idx)?
            } else {
                current.child_normal(idx)?
            };
        }
        Ok(current)
    }
}

/// Derive a BIP-44 private key for ZBX Chain.
///
/// Path: m/44'/{coin_type}'/account'/change/index
///
/// Standard ZBX path: m/44'/7878'/0'/0/0
///   purpose   = 44  (hardened)
///   coin_type = 7878 (hardened, SLIP-44 Zebvix)
///   account   = 0   (hardened)
///   change    = 0   (external) or 1 (internal/change)
///   index     = 0,1,2,... (normal)
pub fn derive_bip44(
    seed:      &[u8; 64],
    coin_type: u32,
    account:   u32,
    change:    u32,
    index:     u32,
) -> Result<[u8; 32], WalletError> {
    let root = XKey::from_seed(seed)?;
    let child = root
        .child_hardened(44)?
        .child_hardened(coin_type)?
        .child_hardened(account)?
        .child_normal(change)?
        .child_normal(index)?;
    Ok(child.key)
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Compute (key + tweak) mod secp256k1 curve order n.
///
/// Returns an error if the result is zero or if either input is not a valid scalar.
fn scalar_add(key: &[u8; 32], tweak: &[u8]) -> Result<[u8; 32], WalletError> {
    let mut tw32 = [0u8; 32];
    tw32.copy_from_slice(&tweak[..32]);

    let sk_opt = Scalar::from_repr(*FieldBytes::from_slice(key));
    let tw_opt = Scalar::from_repr(*FieldBytes::from_slice(&tw32));

    let sk_valid: bool = sk_opt.is_some().into();
    let tw_valid: bool = tw_opt.is_some().into();

    if !sk_valid || !tw_valid {
        return Err(WalletError::DerivationFailed);
    }

    let sk  = sk_opt.unwrap();
    let tw  = tw_opt.unwrap();
    let sum = sk + tw;

    if bool::from(sum.is_zero()) {
        return Err(WalletError::DerivationFailed);
    }

    let mut out = [0u8; 32];
    out.copy_from_slice(&sum.to_repr());
    Ok(out)
}

/// Return the 33-byte compressed public key for a private key.
fn compressed_pubkey(private_key: &[u8; 32]) -> Result<[u8; 33], WalletError> {
    let sk = SecretKey::from_bytes(private_key.into())
        .map_err(|_| WalletError::InvalidPrivateKey)?;
    let encoded = sk.public_key().to_encoded_point(true);
    let mut out = [0u8; 33];
    out.copy_from_slice(encoded.as_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_seed() -> [u8; 64] { [0x42u8; 64] }

    #[test]
    fn from_seed_succeeds() {
        let xk = XKey::from_seed(&test_seed()).unwrap();
        assert_ne!(xk.key, [0u8; 32]);
        assert_ne!(xk.chain_code, [0u8; 32]);
    }

    #[test]
    fn from_seed_is_deterministic() {
        let a = XKey::from_seed(&test_seed()).unwrap();
        let b = XKey::from_seed(&test_seed()).unwrap();
        assert_eq!(a.key, b.key);
        assert_eq!(a.chain_code, b.chain_code);
    }

    #[test]
    fn hardened_child_differs_from_parent() {
        let root = XKey::from_seed(&test_seed()).unwrap();
        let child = root.child_hardened(0).unwrap();
        assert_ne!(root.key, child.key);
    }

    #[test]
    fn normal_child_differs_from_parent() {
        let root = XKey::from_seed(&test_seed()).unwrap();
        let child = root.child_normal(0).unwrap();
        assert_ne!(root.key, child.key);
    }

    #[test]
    fn different_indices_give_different_keys() {
        let root = XKey::from_seed(&test_seed()).unwrap();
        let c0 = root.child_hardened(0).unwrap();
        let c1 = root.child_hardened(1).unwrap();
        assert_ne!(c0.key, c1.key);
    }

    #[test]
    fn derive_path_master_returns_self() {
        let root = XKey::from_seed(&test_seed()).unwrap();
        let same = root.derive_path("m").unwrap();
        assert_eq!(root.key, same.key);
    }

    #[test]
    fn derive_bip44_zbx_path_succeeds() {
        let key = derive_bip44(&test_seed(), 7878, 0, 0, 0).unwrap();
        assert_ne!(key, [0u8; 32]);
    }

    #[test]
    fn derive_bip44_different_indices_differ() {
        let k0 = derive_bip44(&test_seed(), 7878, 0, 0, 0).unwrap();
        let k1 = derive_bip44(&test_seed(), 7878, 0, 0, 1).unwrap();
        assert_ne!(k0, k1);
    }
}
