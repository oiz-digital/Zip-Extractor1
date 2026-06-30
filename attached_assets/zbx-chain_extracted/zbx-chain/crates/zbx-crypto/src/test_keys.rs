//! Deterministic test key helpers for unit and integration tests.
//!
//! Each function derives keys from a single `seed` byte so tests are
//! hermetic and reproducible without an external key store.
//!
//! **DO NOT use in production.** These keys are derived from a fixed
//! byte pattern and offer NO security. They are only enabled under
//! `#[cfg(any(test, feature = "testing"))]`.

use crate::secp256k1::{PrivKey, PubKey, address_from_pubkey};
use zbx_types::address::Address;

/// Derive a deterministic secp256k1 `(PrivKey, PubKey, Address)` triple
/// from a single `seed` byte.
///
/// # Panics
/// Panics if `seed == 0` (scalar 0 is invalid for secp256k1). Use seeds 1–255.
///
/// # Example
/// ```rust
/// use zbx_crypto::test_keys::test_keypair;
/// let (priv_key, pub_key, addr) = test_keypair(1);
/// ```
pub fn test_keypair(seed: u8) -> (PrivKey, PubKey, Address) {
    assert!(seed != 0, "test_keypair: seed 0 is an invalid secp256k1 scalar");
    let mut raw = [0u8; 32];
    // Fill with seed in a pattern that keeps the scalar in [1, n-1].
    // Put seed byte at position 31 (little-endian end) so the value == seed,
    // which is always < n for seed in 1..=255.
    raw[31] = seed;
    let priv_key = PrivKey::from_bytes(&raw)
        .expect("test_keypair: deterministic privkey construction failed");
    let pub_key = priv_key.to_pubkey();
    let address = address_from_pubkey(&pub_key);
    (priv_key, pub_key, address)
}

/// Return a deterministic test `Address` for a given seed (1–255).
pub fn test_address(seed: u8) -> Address {
    test_keypair(seed).2
}

/// Return a deterministic test `PrivKey` for a given seed (1–255).
pub fn test_privkey(seed: u8) -> PrivKey {
    test_keypair(seed).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keypair_deterministic() {
        let (_, _, addr1a) = test_keypair(1);
        let (_, _, addr1b) = test_keypair(1);
        assert_eq!(addr1a, addr1b, "same seed must produce same address");
    }

    #[test]
    fn test_keypair_unique_per_seed() {
        let (_, _, addr1) = test_keypair(1);
        let (_, _, addr2) = test_keypair(2);
        assert_ne!(addr1, addr2, "different seeds must produce different addresses");
    }

    #[test]
    fn test_address_matches_keypair() {
        for seed in [1u8, 5, 42, 127, 255] {
            let (_, _, addr_from_pair) = test_keypair(seed);
            let addr_direct = test_address(seed);
            assert_eq!(addr_from_pair, addr_direct);
        }
    }

    #[test]
    #[should_panic(expected = "seed 0 is an invalid secp256k1 scalar")]
    fn test_keypair_seed_zero_panics() {
        test_keypair(0);
    }
}
