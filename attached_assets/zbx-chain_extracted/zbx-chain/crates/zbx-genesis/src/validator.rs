//! Genesis validator — verifies that a running node matches the expected genesis.

use crate::{GenesisError, GenesisSpec};
use zbx_types::H256;
use zbx_types::pinned_genesis::{
    PinPolicy, verify_pinned_with_policy,
};

/// Verify that the stored genesis matches the spec.
pub fn verify_genesis_hash(
    spec:          &GenesisSpec,
    stored_hash:   &str,
    expected_hash: &str,
) -> Result<(), GenesisError> {
    if stored_hash != expected_hash {
        return Err(GenesisError::GenesisHashMismatch {
            expected: expected_hash.to_string(),
            got:      stored_hash.to_string(),
        });
    }
    Ok(())
}

/// Verify state root matches.
pub fn verify_state_root(expected: &str, got: &str) -> Result<(), GenesisError> {
    if expected != got {
        return Err(GenesisError::StateRootMismatch {
            expected: expected.to_string(),
            got:      got.to_string(),
        });
    }
    Ok(())
}

/// Check chain ID consistency.
pub fn verify_chain_id(spec: &GenesisSpec, node_chain_id: u64) -> Result<(), GenesisError> {
    if spec.chain_id != node_chain_id {
        return Err(GenesisError::Invalid(
            format!("chain ID mismatch: genesis={}, node={}", spec.chain_id, node_chain_id)
        ));
    }
    Ok(())
}

/// S30 — Verify a locally-computed genesis hash against the pinned
/// canonical hash for `chain_id`. Bridge to
/// [`zbx_types::pinned_genesis::verify_pinned_with_policy`]; converts
/// `PinError` to `GenesisError::Invalid` so callers using the genesis
/// crate's error type get a uniform error surface.
///
/// Use [`PinPolicy::Required`] for production; under
/// [`PinPolicy::AllowUnregistered`] devnet chain_ids skip enforcement.
pub fn verify_against_pinned(
    chain_id: u64,
    computed_hash: H256,
    policy: PinPolicy,
) -> Result<(), GenesisError> {
    verify_pinned_with_policy(chain_id, computed_hash, policy)
        .map_err(|e| GenesisError::Invalid(format!("genesis pinning: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbx_types::CHAIN_ID_MAINNET;

    #[test]
    fn verify_against_pinned_rejects_sentinel_for_mainnet() {
        // Mainnet const is sentinel until operator pins.
        let arbitrary = H256([0x42u8; 32]);
        let err = verify_against_pinned(
            CHAIN_ID_MAINNET, arbitrary, PinPolicy::Required,
        ).unwrap_err();
        match err {
            GenesisError::Invalid(msg) => {
                assert!(msg.contains("SENTINEL"), "got: {msg}");
            }
            _ => panic!("wrong variant: {err:?}"),
        }
    }

    #[test]
    fn verify_against_pinned_allows_devnet_under_permissive_policy() {
        // Devnet chain_id (31337) unknown → AllowUnregistered passes.
        let arbitrary = H256([0x42u8; 32]);
        verify_against_pinned(31337, arbitrary, PinPolicy::AllowUnregistered).unwrap();
    }

    #[test]
    fn verify_against_pinned_rejects_devnet_under_strict_policy() {
        let arbitrary = H256([0x42u8; 32]);
        let err = verify_against_pinned(31337, arbitrary, PinPolicy::Required).unwrap_err();
        match err {
            GenesisError::Invalid(msg) => {
                assert!(msg.contains("not registered"), "got: {msg}");
            }
            _ => panic!("wrong variant: {err:?}"),
        }
    }

    #[test]
    fn verify_against_pinned_strict_rejects_sentinel_even_under_permissive() {
        // Operator cannot bypass mainnet pinning by flipping the policy flag.
        let arbitrary = H256([0x42u8; 32]);
        let err = verify_against_pinned(
            CHAIN_ID_MAINNET, arbitrary, PinPolicy::AllowUnregistered,
        ).unwrap_err();
        match err {
            GenesisError::Invalid(msg) => {
                assert!(msg.contains("SENTINEL"), "got: {msg}");
            }
            _ => panic!("wrong variant: {err:?}"),
        }
    }
}