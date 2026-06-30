//! Pinned genesis-hash registry — single source of truth.
//!
//! # Why pinning matters
//!
//! Without a hard-coded canonical genesis hash, an attacker running a
//! malicious bootnode can trick a fresh node into syncing an alternate
//! chain (long-range fork attack). Pinning at compile time ensures the
//! node refuses to trust ANY peer that reports a different genesis,
//! and refuses to trust any locally-built genesis state whose computed
//! hash does not match the pinned constant.
//!
//! # Operator workflow (one-time, pre-mainnet)
//!
//! 1. Finalize the mainnet `genesis.json` (allocations, validators,
//!    timestamp, gas limit, base fee, extra data).
//! 2. Build the genesis state trie and compute the canonical genesis
//!    block hash via [`zbx_genesis::GenesisBuilder::genesis_block_hash`].
//! 3. Replace the [`MAINNET_GENESIS_HASH`] constant in this file (and
//!    [`TESTNET_GENESIS_HASH`] for testnet) with the real value.
//! 4. Re-build the binary, sign the release, and distribute.
//!
//! # Sentinel-until-pinned
//!
//! Both constants currently default to [`SENTINEL_HASH`] = `[0xFF; 32]`.
//! Any production code path that calls
//! [`verify_pinned_with_policy`] with [`PinPolicy::Required`] will
//! HARD ERROR (not warn) when the constant is still the sentinel.
//! This is intentional: a binary built with the sentinel MUST refuse
//! to start in production. The all-zero hash is also rejected because
//! it indicates an uninitialised computed-hash bug rather than a real
//! genesis. See `PinError::Sentinel` and `PinError::AllZero`.
//!
//! # Devnet / local-test path
//!
//! Devnet and local tests use chain IDs OUTSIDE the {8989, 8990} pair
//! (e.g., 31337). Calling [`verify_pinned_with_policy`] with
//! [`PinPolicy::AllowUnregistered`] returns Ok for unregistered chain
//! IDs without enforcement, while still rejecting sentinel for known
//! chain IDs (operator can't accidentally bypass mainnet pinning by
//! flipping a flag).

use crate::{H256, CHAIN_ID_MAINNET, CHAIN_ID_TESTNET};

/// Sentinel value indicating "operator has not pinned this constant
/// yet — production startup MUST refuse to proceed". Distinct from
/// the all-zero hash so misconfigurations and uninitialised bugs are
/// independently identifiable.
pub const SENTINEL_HASH: H256 = H256([0xFFu8; 32]);

/// Canonical genesis block hash for Zebvix mainnet (chain ID 8989).
///
/// **OPERATOR ACTION REQUIRED before mainnet launch**: replace this
/// constant with the real canonical hash produced by
/// [`zbx_genesis::GenesisBuilder::genesis_block_hash`]. The sentinel
/// value will cause [`verify_pinned_with_policy`] to hard-error in
/// production paths (see module-level docs).
pub const MAINNET_GENESIS_HASH: H256 = SENTINEL_HASH;

/// Canonical genesis block hash for Zebvix public testnet AND devnet
/// (chain ID 8990 — both share this preset; see `ChainConfig::testnet`).
///
/// Same operator-action requirement as [`MAINNET_GENESIS_HASH`].
pub const TESTNET_GENESIS_HASH: H256 = SENTINEL_HASH;

/// Lookup the pinned genesis hash for a known chain ID.
///
/// Returns `None` for unregistered chain IDs (devnet / local test
/// nets). Returns the sentinel for known chain IDs that have not
/// yet been pinned by the operator.
pub fn pinned_for(chain_id: u64) -> Option<H256> {
    match chain_id {
        CHAIN_ID_MAINNET => Some(MAINNET_GENESIS_HASH),
        CHAIN_ID_TESTNET => Some(TESTNET_GENESIS_HASH),
        _ => None,
    }
}

/// `true` iff `h` is the [`SENTINEL_HASH`] (operator hasn't pinned).
pub fn is_sentinel(h: &H256) -> bool {
    h.0 == SENTINEL_HASH.0
}

/// `true` iff `h` is the all-zero hash (uninitialised / bug indicator).
pub fn is_all_zero(h: &H256) -> bool {
    h.0 == [0u8; 32]
}

/// Pinning enforcement policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinPolicy {
    /// Production: known chain IDs MUST have a non-sentinel pinned
    /// hash; unregistered chain IDs are rejected.
    Required,
    /// Devnet / local test: unregistered chain IDs are accepted
    /// without enforcement. Known chain IDs are still subject to
    /// sentinel/zero/mismatch checks (operator cannot bypass mainnet
    /// pinning by flipping this flag).
    AllowUnregistered,
}

/// Errors raised by [`verify_pinned_with_policy`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PinError {
    /// Chain ID is not registered in the pinning registry. Operator
    /// must either add it to [`pinned_for`] or use
    /// [`PinPolicy::AllowUnregistered`] for devnet.
    #[error("chain_id {0} is not registered in the pinned-genesis registry")]
    UnknownChainId(u64),

    /// Chain ID is registered but the constant is still the sentinel.
    /// Operator MUST replace the constant before deploying to this chain.
    #[error("chain_id {0}: pinned genesis hash is still the SENTINEL — \
             operator MUST replace the constant in zbx_types::pinned_genesis \
             before deploying to this chain")]
    Sentinel(u64),

    /// Computed genesis hash is all-zero — strong indicator of a bug
    /// in the genesis-builder, not a real chain.
    #[error("chain_id {0}: computed genesis hash is all-zero (likely uninitialised)")]
    AllZero(u64),

    /// Pinned hash and computed hash diverge.
    #[error("chain_id {chain_id}: genesis hash mismatch — \
             pinned=0x{expected_hex} computed=0x{got_hex}")]
    Mismatch {
        chain_id:     u64,
        expected_hex: String,
        got_hex:      String,
    },
}

impl PinError {
    pub fn mismatch(chain_id: u64, expected: H256, got: H256) -> Self {
        Self::Mismatch {
            chain_id,
            expected_hex: hex::encode(expected.0),
            got_hex:      hex::encode(got.0),
        }
    }
}

/// Verify that `computed` matches the pinned hash for `chain_id`.
///
/// HARD-rejects:
///   1. Unknown `chain_id` (unless `policy == AllowUnregistered`).
///   2. Sentinel pinned constant (operator hasn't pinned).
///   3. All-zero `computed` hash.
///   4. `computed != pinned`.
///
/// On `policy == AllowUnregistered` AND `chain_id` not in registry,
/// returns Ok WITHOUT enforcement (devnet path). Sentinel is still
/// rejected for registered chain IDs even under this policy.
pub fn verify_pinned_with_policy(
    chain_id: u64,
    computed: H256,
    policy:   PinPolicy,
) -> Result<(), PinError> {
    let expected = match (pinned_for(chain_id), policy) {
        (None, PinPolicy::AllowUnregistered) => return Ok(()),
        (None, PinPolicy::Required)          => return Err(PinError::UnknownChainId(chain_id)),
        (Some(h), _)                         => h,
    };

    if is_sentinel(&expected) {
        return Err(PinError::Sentinel(chain_id));
    }
    if is_all_zero(&computed) {
        return Err(PinError::AllZero(chain_id));
    }
    if expected != computed {
        return Err(PinError::mismatch(chain_id, expected, computed));
    }
    Ok(())
}

/// Convenience: strict-production verify.
pub fn verify_pinned(chain_id: u64, computed: H256) -> Result<(), PinError> {
    verify_pinned_with_policy(chain_id, computed, PinPolicy::Required)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_real_hash() -> H256 {
        H256([0x42u8; 32])
    }

    #[test]
    fn sentinel_is_distinct_from_zero() {
        assert!(is_sentinel(&SENTINEL_HASH));
        assert!(!is_all_zero(&SENTINEL_HASH));
        assert!(!is_sentinel(&H256::zero()));
        assert!(is_all_zero(&H256::zero()));
    }

    #[test]
    fn pinned_for_known_chain_ids_returns_sentinel_until_operator_pins() {
        // Both constants are the sentinel by default.
        assert_eq!(pinned_for(CHAIN_ID_MAINNET), Some(SENTINEL_HASH));
        assert_eq!(pinned_for(CHAIN_ID_TESTNET), Some(SENTINEL_HASH));
    }

    #[test]
    fn pinned_for_unknown_chain_id_returns_none() {
        assert_eq!(pinned_for(31337), None);
        assert_eq!(pinned_for(1), None);
    }

    #[test]
    fn verify_pinned_required_rejects_unknown_chain() {
        let err = verify_pinned(31337, fake_real_hash()).unwrap_err();
        assert!(matches!(err, PinError::UnknownChainId(31337)));
    }

    #[test]
    fn verify_pinned_required_rejects_sentinel() {
        // Mainnet const is sentinel → must hard-error.
        let err = verify_pinned(CHAIN_ID_MAINNET, fake_real_hash()).unwrap_err();
        assert!(matches!(err, PinError::Sentinel(CHAIN_ID_MAINNET)));
    }

    #[test]
    fn verify_pinned_allow_unregistered_accepts_devnet() {
        // Devnet chain_id (31337) unknown → AllowUnregistered passes.
        verify_pinned_with_policy(
            31337, fake_real_hash(), PinPolicy::AllowUnregistered,
        ).unwrap();
    }

    #[test]
    fn verify_pinned_allow_unregistered_still_rejects_sentinel_for_known_chain() {
        // Operator cannot bypass mainnet pinning by flipping the policy flag.
        let err = verify_pinned_with_policy(
            CHAIN_ID_MAINNET, fake_real_hash(), PinPolicy::AllowUnregistered,
        ).unwrap_err();
        assert!(matches!(err, PinError::Sentinel(CHAIN_ID_MAINNET)));
    }

    // ─── Once-pinned simulation ──────────────────────────────────────
    // We can't mutate consts at runtime; these tests use a local helper
    // that mirrors verify_pinned_with_policy with a swapped expected.

    fn verify_with_expected(
        chain_id: u64,
        computed: H256,
        expected: H256,
    ) -> Result<(), PinError> {
        if is_sentinel(&expected) {
            return Err(PinError::Sentinel(chain_id));
        }
        if is_all_zero(&computed) {
            return Err(PinError::AllZero(chain_id));
        }
        if expected != computed {
            return Err(PinError::mismatch(chain_id, expected, computed));
        }
        Ok(())
    }

    #[test]
    fn once_pinned_accepts_matching_computed() {
        let pin = fake_real_hash();
        verify_with_expected(CHAIN_ID_MAINNET, pin, pin).unwrap();
    }

    #[test]
    fn once_pinned_rejects_zero_computed() {
        let pin = fake_real_hash();
        let err = verify_with_expected(CHAIN_ID_MAINNET, H256::zero(), pin).unwrap_err();
        assert!(matches!(err, PinError::AllZero(CHAIN_ID_MAINNET)));
    }

    #[test]
    fn once_pinned_rejects_mismatched_computed() {
        let pin = fake_real_hash();
        let bad = H256([0x99u8; 32]);
        let err = verify_with_expected(CHAIN_ID_MAINNET, bad, pin).unwrap_err();
        match err {
            PinError::Mismatch { chain_id, expected_hex, got_hex } => {
                assert_eq!(chain_id, CHAIN_ID_MAINNET);
                assert_eq!(expected_hex, hex::encode(pin.0));
                assert_eq!(got_hex,      hex::encode(bad.0));
            }
            _ => panic!("wrong variant: {err:?}"),
        }
    }
}
