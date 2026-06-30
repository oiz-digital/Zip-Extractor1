//! BIP-39 mnemonic generation, validation, and seed derivation.
//!
//! Implements the full BIP-39 specification:
//! - Random entropy → mnemonic phrase with valid checksum
//! - Mnemonic → 512-bit seed via PBKDF2-HMAC-SHA512 (2048 rounds)
//! - Word validation against the 2048-word English wordlist
//!
//! ZBX HD path: m/44'/7878'/account'/change/index
//! (coin type 7878 = SLIP-44 Zebvix — independent of chain ID 8989/8990)
//!
//! Uses bip39 v1.x API (rust-bitcoin team):
//!   - `Mnemonic::from_entropy(&[u8])` — entropy → mnemonic (1 arg)
//!   - `Mnemonic::parse(&str)`          — parse + validate
//!   - `mnemonic.to_seed(passphrase)`   — returns [u8; 64]
//!   - `mnemonic.to_string()`           — canonical phrase string

use bip39::Mnemonic;
use getrandom::getrandom;
use crate::create_import::WalletError;

/// Generate a new random BIP-39 mnemonic phrase.
///
/// Uses OS entropy (getrandom) internally — no additional randomness needed.
/// The returned phrase includes a valid BIP-39 checksum.
///
/// Supported word counts: 12, 15, 18, 21, 24
pub fn generate(word_count: usize) -> Result<String, WalletError> {
    // BIP-39 entropy lengths by word count:
    //   12 → 16 bytes,  15 → 20,  18 → 24,  21 → 28,  24 → 32
    let entropy_len: usize = match word_count {
        12 => 16,
        15 => 20,
        18 => 24,
        21 => 28,
        24 => 32,
        _  => return Err(WalletError::InvalidWordCount),
    };
    let mut entropy = vec![0u8; entropy_len];
    getrandom(&mut entropy).map_err(|_| WalletError::InvalidMnemonic)?;
    let mnemonic = Mnemonic::from_entropy(&entropy)
        .map_err(|_| WalletError::InvalidMnemonic)?;
    Ok(mnemonic.to_string())
}

/// Convert a BIP-39 mnemonic to a 512-bit seed.
///
/// Algorithm: PBKDF2-HMAC-SHA512(
///   password  = NFKD(mnemonic_phrase),
///   salt      = "mnemonic" + NFKD(passphrase),
///   iterations = 2048,
///   dkLen      = 64 bytes,
/// )
///
/// Compatible with all BIP-39 wallets (MetaMask, Ledger, Trezor, etc.).
pub fn to_seed(mnemonic: &str, passphrase: &str) -> Result<[u8; 64], WalletError> {
    let m = Mnemonic::parse(mnemonic.trim())
        .map_err(|_| WalletError::InvalidMnemonic)?;
    Ok(m.to_seed(passphrase))
}

/// Validate a BIP-39 mnemonic phrase.
///
/// Checks:
/// 1. All words are in the BIP-39 English wordlist
/// 2. Word count is valid (12, 15, 18, 21, or 24)
/// 3. Checksum bits are correct
pub fn validate(mnemonic: &str) -> bool {
    Mnemonic::parse(mnemonic.trim()).is_ok()
}

/// Parse and normalize a mnemonic phrase.
///
/// Returns the canonical whitespace-normalized phrase on success.
pub fn parse(mnemonic: &str) -> Result<String, WalletError> {
    let m = Mnemonic::parse(mnemonic.trim())
        .map_err(|_| WalletError::InvalidMnemonic)?;
    Ok(m.to_string())
}

/// Convert entropy bytes to a BIP-39 mnemonic (low-level).
///
/// Entropy lengths → word counts:
///   16 bytes → 12 words
///   20 bytes → 15 words
///   24 bytes → 18 words
///   28 bytes → 21 words
///   32 bytes → 24 words
pub fn entropy_to_mnemonic(entropy: &[u8]) -> Result<String, WalletError> {
    let m = Mnemonic::from_entropy(entropy)
        .map_err(|_| WalletError::InvalidMnemonic)?;
    Ok(m.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_12: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    const VALID_24: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";

    #[test]
    fn validate_known_valid_mnemonic() {
        assert!(validate(VALID_12));
    }

    #[test]
    fn validate_rejects_invalid() {
        assert!(!validate("this is not a valid bip39 mnemonic phrase at all sorry"));
    }

    #[test]
    fn to_seed_is_deterministic() {
        let s1 = to_seed(VALID_12, "").unwrap();
        let s2 = to_seed(VALID_12, "").unwrap();
        assert_eq!(s1, s2);
    }

    #[test]
    fn to_seed_passphrase_changes_result() {
        let s1 = to_seed(VALID_12, "").unwrap();
        let s2 = to_seed(VALID_12, "passphrase").unwrap();
        assert_ne!(s1, s2);
    }

    #[test]
    fn parse_normalizes_whitespace() {
        let with_extra = format!("  {}  ", VALID_12);
        let parsed = parse(&with_extra).unwrap();
        assert_eq!(parsed.split_whitespace().count(), 12);
    }

    #[test]
    fn entropy_to_mnemonic_16_bytes_gives_12_words() {
        let entropy = [0u8; 16];
        let phrase = entropy_to_mnemonic(&entropy).unwrap();
        assert_eq!(phrase.split_whitespace().count(), 12);
    }

    #[test]
    fn entropy_to_mnemonic_32_bytes_gives_24_words() {
        let entropy = [0xffu8; 32];
        let phrase = entropy_to_mnemonic(&entropy).unwrap();
        assert_eq!(phrase.split_whitespace().count(), 24);
    }

    #[test]
    fn generate_12_words() {
        let phrase = generate(12).unwrap();
        assert_eq!(phrase.split_whitespace().count(), 12);
        assert!(validate(&phrase));
    }

    #[test]
    fn generate_24_words() {
        let phrase = generate(24).unwrap();
        assert_eq!(phrase.split_whitespace().count(), 24);
        assert!(validate(&phrase));
    }

    #[test]
    fn generate_invalid_word_count_errors() {
        assert!(generate(13).is_err());
    }
}
