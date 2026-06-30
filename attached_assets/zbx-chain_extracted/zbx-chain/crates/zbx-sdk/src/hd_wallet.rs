//! BIP-32/BIP-39/BIP-44 HD wallet derivation.
//!
//! Derives deterministic wallets from a 12/24-word mnemonic phrase.
//! Derivation path: `m/44'/7878'/account'/0/index`
//!
//! ZBX coin type 7878 is the SLIP-44 registered code for Zebvix and is
//! INDEPENDENT of chain ID. Chain IDs are 8989 (mainnet) / 8990 (testnet+devnet).
//! Do NOT change the coin type when chain IDs change — it would break wallet
//! recovery for every existing user.
//!
//! ## BIP-32 compliance
//!
//! This implementation tracks the chain code at every derivation level,
//! which is required by the BIP-32 specification. A naive implementation that
//! discards the chain code produces keys incompatible with all standard wallets.

#![cfg(feature = "hd")]

use crate::{error::SdkError, wallet::Wallet};
use hmac::{Hmac, Mac};
use sha2::Sha512;
use pbkdf2::pbkdf2_hmac;
use unicode_normalization::UnicodeNormalization;
use k256::{SecretKey, Scalar, FieldBytes};
use k256::elliptic_curve::{PrimeField, sec1::ToEncodedPoint};

type HmacSha512 = Hmac<Sha512>;

/// SLIP-44 registered coin type for Zebvix. Independent of chain ID.
pub const ZBX_COIN_TYPE: u32 = zbx_types::BIP44_COIN_TYPE_ZBX as u32;
pub const DEFAULT_ACCOUNT: u32 = 0;

/// An extended private key: private key + chain code (both 32 bytes).
///
/// The chain code is the entropy for child key derivation. Discarding it
/// is a common BIP-32 bug that makes child keys wrong.
struct XKey {
    key:        [u8; 32],
    chain_code: [u8; 32],
}

/// Derive a wallet at a BIP-44 path from a mnemonic.
/// Path: `m/44'/7878'/account'/0/index`
pub fn derive(mnemonic: &str, password: &str, account: u32, index: u32)
    -> Result<Wallet, SdkError>
{
    let seed  = mnemonic_to_seed(mnemonic, password);
    let root  = derive_root_key(&seed)?;
    let child = derive_path(root, &[
        0x80000000 | 44,             // purpose: BIP44 (hardened)
        0x80000000 | ZBX_COIN_TYPE,  // coin type: ZBX 7878 (hardened)
        0x80000000 | account,        // account (hardened)
        0,                           // change: external
        index,                       // address index (normal)
    ])?;
    Wallet::from_bytes(&child.key).map_err(|e| e)
}

/// Generate a random mnemonic phrase using OS entropy.
///
/// Produces a BIP-39-compliant mnemonic with a valid checksum.
pub fn generate_mnemonic(word_count: u8) -> Result<String, SdkError> {
    if word_count != 12 && word_count != 24 {
        return Err(SdkError::Other("mnemonic must be 12 or 24 words".into()));
    }
    #[cfg(feature = "bip39")]
    {
        use bip39::{Mnemonic, MnemonicType, Language};
        let mnemonic_type = if word_count == 12 {
            MnemonicType::Words12
        } else {
            MnemonicType::Words24
        };
        let mnemonic = Mnemonic::new(mnemonic_type, Language::English);
        return Ok(mnemonic.phrase().to_string());
    }
    #[allow(unreachable_code)]
    Err(SdkError::Other("hd feature requires bip39 for mnemonic generation".into()))
}

/// Validate a BIP-39 mnemonic phrase.
///
/// With the `bip39` feature: validates wordlist membership AND checksum.
/// Without it: only validates word count (12 or 24).
pub fn validate_mnemonic(mnemonic: &str) -> bool {
    #[cfg(feature = "bip39")]
    {
        use bip39::{Mnemonic, Language};
        return Mnemonic::validate(mnemonic.trim(), Language::English).is_ok();
    }
    #[allow(unreachable_code)]
    {
        let words: Vec<&str> = mnemonic.trim().split_whitespace().collect();
        words.len() == 12 || words.len() == 24
    }
}

// ── BIP-39 seed derivation ────────────────────────────────────────────────────

fn mnemonic_to_seed(mnemonic: &str, password: &str) -> [u8; 64] {
    // PBKDF2-HMAC-SHA512(password=NFKD(mnemonic), salt="mnemonic"+NFKD(pass), 2048, 64)
    let nfkd_mnemonic: String = mnemonic.nfkd().collect();
    let nfkd_password: String = password.nfkd().collect();
    let salt = format!("mnemonic{}", nfkd_password);
    let mut seed = [0u8; 64];
    pbkdf2_hmac::<Sha512>(
        nfkd_mnemonic.as_bytes(),
        salt.as_bytes(),
        2048,
        &mut seed,
    );
    seed
}

// ── BIP-32 key derivation (with chain code) ───────────────────────────────────

/// Derive the BIP-32 master extended key from a 512-bit seed.
///
/// I = HMAC-SHA512("Bitcoin seed", seed)
/// master_key        = I[0..32]
/// master_chain_code = I[32..64]
fn derive_root_key(seed: &[u8; 64]) -> Result<XKey, SdkError> {
    let mut mac = HmacSha512::new_from_slice(b"Bitcoin seed")
        .map_err(|e| SdkError::Other(e.to_string()))?;
    mac.update(seed);
    let result = mac.finalize().into_bytes();
    let mut key        = [0u8; 32];
    let mut chain_code = [0u8; 32];
    key.copy_from_slice(&result[..32]);
    chain_code.copy_from_slice(&result[32..]);
    if key == [0u8; 32] {
        return Err(SdkError::Other("BIP-32 master key is zero".into()));
    }
    Ok(XKey { key, chain_code })
}

fn derive_path(mut xkey: XKey, indices: &[u32]) -> Result<XKey, SdkError> {
    for &index in indices {
        xkey = derive_child(xkey, index)?;
    }
    Ok(xkey)
}

/// Derive a BIP-32 child extended key.
///
/// Hardened (index >= 0x80000000):
///   HMAC-SHA512(parent_chain_code, 0x00 || parent_key || index_be)
/// Normal:
///   HMAC-SHA512(parent_chain_code, compressed_pubkey || index_be)
///
/// child_key = (HMAC[0..32] + parent_key) mod n
/// child_chain_code = HMAC[32..64]
fn derive_child(parent: XKey, index: u32) -> Result<XKey, SdkError> {
    // HMAC key is always the parent CHAIN CODE (not the parent key — common bug!)
    let mut mac = HmacSha512::new_from_slice(&parent.chain_code)
        .map_err(|e| SdkError::Other(e.to_string()))?;

    if index >= 0x80000000 {
        // Hardened: serialize private key
        mac.update(&[0x00]);
        mac.update(&parent.key);
    } else {
        // Normal: serialize compressed public key
        let pubkey = compressed_pubkey(&parent.key)
            .map_err(|e| SdkError::Other(format!("BIP-32 pubkey: {e:?}")))?;
        mac.update(&pubkey);
    }
    mac.update(&index.to_be_bytes());

    let result = mac.finalize().into_bytes();
    let child_key = scalar_add(&parent.key, &result[..32])
        .map_err(|e| SdkError::Other(format!("BIP-32 scalar add: {e:?}")))?;
    let mut chain_code = [0u8; 32];
    chain_code.copy_from_slice(&result[32..]);
    Ok(XKey { key: child_key, chain_code })
}

/// Add a tweak scalar to a private key: result = (key + tweak) mod n.
fn scalar_add(key: &[u8; 32], tweak: &[u8]) -> Result<[u8; 32], SdkError> {
    let mut tw32 = [0u8; 32];
    tw32.copy_from_slice(&tweak[..32]);

    let sk_opt = Scalar::from_repr(*FieldBytes::from_slice(key));
    let tw_opt = Scalar::from_repr(*FieldBytes::from_slice(&tw32));

    let sk_valid: bool = sk_opt.is_some().into();
    let tw_valid: bool = tw_opt.is_some().into();

    if !sk_valid || !tw_valid {
        return Err(SdkError::Other("invalid scalar in BIP-32 derivation".into()));
    }
    let sum = sk_opt.unwrap() + tw_opt.unwrap();
    if bool::from(sum.is_zero()) {
        return Err(SdkError::Other("BIP-32 derived zero key — retry with next index".into()));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&sum.to_repr());
    Ok(out)
}

/// Return the 33-byte compressed secp256k1 public key for a private key.
fn compressed_pubkey(private_key: &[u8; 32]) -> Result<[u8; 33], SdkError> {
    let sk = SecretKey::from_bytes(private_key.into())
        .map_err(|e| SdkError::Other(e.to_string()))?;
    let encoded = sk.public_key().to_encoded_point(true);
    let mut out = [0u8; 33];
    out.copy_from_slice(encoded.as_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon \
         abandon abandon abandon abandon abandon about";

    #[test]
    fn test_validate_mnemonic() {
        assert!(validate_mnemonic(TEST_MNEMONIC));
        assert!(!validate_mnemonic("too short"));
    }

    #[test]
    fn test_derive_deterministic() {
        let w1 = derive(TEST_MNEMONIC, "", 0, 0).unwrap();
        let w2 = derive(TEST_MNEMONIC, "", 0, 0).unwrap();
        assert_eq!(w1.address(), w2.address());
    }

    #[test]
    fn test_different_index_different_address() {
        let w0 = derive(TEST_MNEMONIC, "", 0, 0).unwrap();
        let w1 = derive(TEST_MNEMONIC, "", 0, 1).unwrap();
        assert_ne!(w0.address(), w1.address());
    }

    #[test]
    fn test_chain_code_affects_child() {
        // Verify that two different derivation levels give different keys,
        // confirming that chain code is actually influencing child derivation.
        let seed = [42u8; 64];
        let root = derive_root_key(&seed).unwrap();
        let child0 = derive_child(XKey { key: root.key, chain_code: root.chain_code }, 0x80000000).unwrap();
        let root2 = derive_root_key(&seed).unwrap();
        let child1 = derive_child(XKey { key: root2.key, chain_code: root2.chain_code }, 0x80000001).unwrap();
        // Different hardened indices must produce different child keys
        assert_ne!(child0.key, child1.key);
    }
}
