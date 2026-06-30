//! Hybrid ECDSA + Dilithium signing for ZBX Chain post-quantum transition.
//!
//! During the transition period (ZEP-015 Phase 1-2), transactions may carry
//! both an ECDSA signature AND a Dilithium signature. The chain accepts a
//! transaction if EITHER signature is valid, providing backward compatibility
//! while encouraging PQ adoption.
//!
//! ## Security Model
//! - Phase 1 (Block 500k+): ECDSA required; Dilithium optional (informational)
//! - Phase 2 (Block 750k+): EITHER ECDSA OR Dilithium valid → accepted
//! - Phase 3 (Block TBD, governance vote): Dilithium required; ECDSA optional
//!
//! Breaking BOTH ECDSA AND Dilithium requires breaking BOTH secp256k1 AND
//! Module-LWE — much stronger than either alone.

use crate::{
    dilithium::{DilithiumPublicKey, DilithiumSignature, verify as dilithium_verify},
    error::PqError,
};
use serde::{Deserialize, Serialize};

/// Phase of the PQ transition (determines which signatures are required).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum PqPhase {
    /// Block < 500,000: ECDSA only
    Classical = 0,
    /// Block 500,000–749,999: ECDSA required, Dilithium optional
    HybridEcdsaPrimary = 1,
    /// Block 750,000+: Either signature valid
    HybridPqPrimary = 2,
    /// Block TBD: Dilithium required (governance vote)
    PostQuantumOnly = 3,
}

impl PqPhase {
    /// Determine phase from block number.
    pub fn from_block(block: u64) -> Self {
        match block {
            0..=499_999   => PqPhase::Classical,
            500_000..=749_999 => PqPhase::HybridEcdsaPrimary,
            750_000..=u64::MAX => PqPhase::HybridPqPrimary,
        }
    }
}

/// A hybrid signature covering both ECDSA and Dilithium.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridSignature {
    /// ECDSA signature (v, r, s) — always present in Phase 0-1
    pub ecdsa_v: Option<u8>,
    pub ecdsa_r: Option<[u8; 32]>,
    pub ecdsa_s: Option<[u8; 32]>,
    /// Dilithium-3 signature — present in Phase 1+
    pub dilithium_sig: Option<DilithiumSignature>,
    /// Dilithium public key — required if dilithium_sig is present
    pub dilithium_pk:  Option<DilithiumPublicKey>,
}

impl HybridSignature {
    /// Create a classical-only (ECDSA) hybrid signature.
    pub fn classical(v: u8, r: [u8; 32], s: [u8; 32]) -> Self {
        HybridSignature {
            ecdsa_v: Some(v),
            ecdsa_r: Some(r),
            ecdsa_s: Some(s),
            dilithium_sig: None,
            dilithium_pk:  None,
        }
    }

    /// Create a Dilithium-only hybrid signature (for Phase 3).
    pub fn pq_only(pk: DilithiumPublicKey, sig: DilithiumSignature) -> Self {
        HybridSignature {
            ecdsa_v: None,
            ecdsa_r: None,
            ecdsa_s: None,
            dilithium_sig: Some(sig),
            dilithium_pk:  Some(pk),
        }
    }

    /// Create a full hybrid signature with both.
    pub fn full(
        v: u8, r: [u8; 32], s: [u8; 32],
        pk: DilithiumPublicKey, sig: DilithiumSignature,
    ) -> Self {
        HybridSignature {
            ecdsa_v: Some(v),
            ecdsa_r: Some(r),
            ecdsa_s: Some(s),
            dilithium_sig: Some(sig),
            dilithium_pk:  Some(pk),
        }
    }

    /// Check if ECDSA component is present.
    pub fn has_ecdsa(&self) -> bool {
        self.ecdsa_r.is_some() && self.ecdsa_s.is_some()
    }

    /// Check if Dilithium component is present.
    pub fn has_dilithium(&self) -> bool {
        self.dilithium_sig.is_some() && self.dilithium_pk.is_some()
    }
}

/// Result of hybrid signature validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HybridVerifyResult {
    /// Only ECDSA valid (classical tx)
    ClassicalOnly,
    /// Only Dilithium valid (PQ-native tx)
    PostQuantumOnly,
    /// Both valid (maximum security)
    Both,
}

/// Verify a hybrid signature against a message hash.
///
/// `ecdsa_valid`: result from ECDSA recovery check (provided by caller
/// who has access to secp256k1 — avoids circular deps here).
pub fn verify_hybrid(
    sig: &HybridSignature,
    message: &[u8],
    ecdsa_valid: bool,
    phase: PqPhase,
) -> Result<HybridVerifyResult, PqError> {
    let pq_valid = match (&sig.dilithium_sig, &sig.dilithium_pk) {
        (Some(dsig), Some(dpk)) => dilithium_verify(dpk, message, dsig).is_ok(),
        _ => false,
    };

    match phase {
        PqPhase::Classical => {
            if ecdsa_valid {
                Ok(HybridVerifyResult::ClassicalOnly)
            } else {
                Err(PqError::HybridVerificationFailed)
            }
        }
        PqPhase::HybridEcdsaPrimary => {
            // ECDSA required in this phase
            if !ecdsa_valid {
                return Err(PqError::HybridVerificationFailed);
            }
            if pq_valid {
                Ok(HybridVerifyResult::Both)
            } else {
                Ok(HybridVerifyResult::ClassicalOnly)
            }
        }
        PqPhase::HybridPqPrimary => {
            // Either is sufficient
            if ecdsa_valid && pq_valid {
                Ok(HybridVerifyResult::Both)
            } else if ecdsa_valid {
                Ok(HybridVerifyResult::ClassicalOnly)
            } else if pq_valid {
                Ok(HybridVerifyResult::PostQuantumOnly)
            } else {
                Err(PqError::HybridVerificationFailed)
            }
        }
        PqPhase::PostQuantumOnly => {
            if pq_valid {
                if ecdsa_valid {
                    Ok(HybridVerifyResult::Both)
                } else {
                    Ok(HybridVerifyResult::PostQuantumOnly)
                }
            } else {
                Err(PqError::HybridVerificationFailed)
            }
        }
    }
}

/// Derive a ZBX address from a Dilithium public key.
/// Identical to ECDSA address derivation (keccak256[12..32]).
pub fn dilithium_address(pk: &DilithiumPublicKey) -> [u8; 20] {
    pk.to_address()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dilithium::{keygen_from_seed, sign};

    #[test]
    fn hybrid_pq_only_verification() {
        let kp = keygen_from_seed(&[5u8; 32]);
        let msg = b"zebvix hybrid tx test";
        let sig = sign(&kp.private_key, msg);
        let hsig = HybridSignature::pq_only(kp.public_key, sig);

        let result = verify_hybrid(&hsig, msg, false, PqPhase::HybridPqPrimary);
        assert_eq!(result.unwrap(), HybridVerifyResult::PostQuantumOnly);
    }

    #[test]
    fn hybrid_classical_required_in_phase1() {
        let kp = keygen_from_seed(&[6u8; 32]);
        let msg = b"phase1 test";
        let sig = sign(&kp.private_key, msg);
        let hsig = HybridSignature::pq_only(kp.public_key, sig);

        // Phase 1 requires ECDSA
        let result = verify_hybrid(&hsig, msg, false, PqPhase::HybridEcdsaPrimary);
        assert!(result.is_err());

        // Phase 1 with valid ECDSA → ok
        let result2 = verify_hybrid(&hsig, msg, true, PqPhase::HybridEcdsaPrimary);
        assert!(result2.is_ok());
    }

    #[test]
    fn phase_from_block() {
        assert_eq!(PqPhase::from_block(0),       PqPhase::Classical);
        assert_eq!(PqPhase::from_block(499_999), PqPhase::Classical);
        assert_eq!(PqPhase::from_block(500_000), PqPhase::HybridEcdsaPrimary);
        assert_eq!(PqPhase::from_block(750_000), PqPhase::HybridPqPrimary);
    }
}
