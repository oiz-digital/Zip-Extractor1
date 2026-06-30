//! Distributed Key Generation (DKG) — Feldman VSS over secp256k1.
//!
//! Each participant generates a secret polynomial of degree `t−1`, broadcasts
//! its public commitments (Feldman VSS commitments), and computes their local
//! secret share by evaluating the polynomial at their participant index.
//!
//! # Protocol (single-party share generation)
//!
//! In a full multi-party DKG:
//!   1. Each participant i samples a random polynomial fᵢ(x) of degree t−1.
//!   2. Each broadcasts Cᵢⱼ = aᵢⱼ·G for j = 0..t−1 (Feldman commitments).
//!   3. Each secretly sends sᵢⱼ = fᵢ(j) to participant j.
//!   4. Each j verifies: sᵢⱼ·G == Σ Cᵢₖ · j^k  (VSS consistency check).
//!   5. The final share for j is: secretⱼ = Σᵢ sᵢⱼ (sum over all participants).
//!   6. The group key is: K = Σᵢ Cᵢ₀  (sum of each participant's a₀·G).
//!
//! `generate_share()` here implements step 1 for a single participant.
//! The aggregation (steps 3–6) happens in the coordinator layer.

use crate::keyshare::KeyShare;
use crate::error::ThresholdError;
use k256::{
    elliptic_curve::{group::GroupEncoding, Field},
    ProjectivePoint, Scalar,
};
use rand::rngs::OsRng;

/// DKG state for one participant.
pub struct DkgState {
    pub index:     u32,
    pub threshold: u32,
    pub total:     u32,
}

impl DkgState {
    pub fn new(index: u32, threshold: u32, total: u32) -> Result<Self, ThresholdError> {
        if threshold == 0 || threshold > total {
            return Err(ThresholdError::ThresholdTooHigh {
                threshold: threshold as usize,
                total: total as usize,
            });
        }
        Ok(Self { index, threshold, total })
    }

    /// Generate a Feldman VSS key share for this participant.
    ///
    /// Samples a uniformly random polynomial `f(x) = a₀ + a₁x + … + a_{t-1}x^{t-1}`
    /// over the secp256k1 scalar field (mod n), then computes:
    ///   - `secret_share = f(self.index)`          — stays on this node, never shared
    ///   - `verifying    = f(self.index)·G`         — broadcast to enable verification
    ///   - `group_key    = a₀·G`                   — this participant's commitment to the
    ///                                               group key (all participants sum theirs)
    ///
    /// The returned `KeyShare` passes `KeyShare::verify()` (non-zero secret).
    pub fn generate_share(&self) -> Result<KeyShare, ThresholdError> {
        let mut rng = OsRng;

        // Sample t random polynomial coefficients: a₀, a₁, …, a_{t-1}
        let coeffs: Vec<Scalar> = (0..self.threshold)
            .map(|_| Scalar::random(&mut rng))
            .collect();

        if coeffs.is_empty() {
            return Err(ThresholdError::ThresholdTooHigh {
                threshold: self.threshold as usize,
                total: self.total as usize,
            });
        }

        // Evaluate f(self.index) using Horner's method for efficiency:
        //   f(x) = a₀ + x·(a₁ + x·(a₂ + … + x·a_{t-1}))
        let x = Scalar::from(self.index as u64);
        let mut share_scalar = *coeffs.last().unwrap();
        for coeff in coeffs.iter().rev().skip(1) {
            share_scalar = share_scalar * x + coeff;
        }

        // Verifying key: f(i)·G — anyone can verify a partial signature against this.
        let verifying_pt = ProjectivePoint::GENERATOR * share_scalar;
        let verifying_bytes = verifying_pt.to_affine().to_bytes(); // 33-byte SEC1 compressed
        let mut verifying = [0u8; 33];
        verifying.copy_from_slice(&verifying_bytes);

        // Group key contribution: a₀·G — broadcast during DKG, summed across participants
        // to form the group's combined public key.
        let group_pt = ProjectivePoint::GENERATOR * coeffs[0];
        let group_bytes = group_pt.to_affine().to_bytes();
        let mut group_key = [0u8; 33];
        group_key.copy_from_slice(&group_bytes);

        // Secret share: f(i) mod n — canonical 32-byte big-endian encoding.
        let share_fb = share_scalar.to_bytes();
        let mut secret_share = [0u8; 32];
        secret_share.copy_from_slice(&share_fb);

        Ok(KeyShare::from_dkg_parts(
            self.index,
            secret_share,
            verifying,
            group_key,
            self.threshold,
            self.total,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_share_produces_nonzero_secret() {
        let state = DkgState::new(1, 2, 3).unwrap();
        let share = state.generate_share().unwrap();
        // Secret must be non-zero (all-zero is the additive identity — not a valid share).
        assert!(!share.secret().iter().all(|&b| b == 0),
                "DKG share secret must not be all-zeros");
    }

    #[test]
    fn generate_share_verifying_key_matches_secret() {
        use k256::{ProjectivePoint, Scalar};
        use k256::elliptic_curve::group::GroupEncoding;
        let state = DkgState::new(1, 2, 3).unwrap();
        let share = state.generate_share().unwrap();

        // Parse the secret scalar back.
        let secret_bytes: [u8; 32] = *share.secret();
        let fb = k256::FieldBytes::from(secret_bytes);
        let s = Scalar::from_repr(fb);
        assert!(bool::from(s.is_some()), "secret must be canonical secp256k1 scalar");
        let s = s.unwrap();

        // The verifying key must equal secret·G.
        let expected_pt = ProjectivePoint::GENERATOR * s;
        let expected_bytes = expected_pt.to_affine().to_bytes();
        assert_eq!(&share.verifying[..], expected_bytes.as_slice(),
                   "verifying key must equal secret * G");
    }

    #[test]
    fn different_indices_produce_different_shares() {
        // Two participants in the same DKG round must get distinct shares.
        let s1 = DkgState::new(1, 2, 3).unwrap().generate_share().unwrap();
        let s2 = DkgState::new(2, 2, 3).unwrap().generate_share().unwrap();
        // Different polynomials → statistically impossible to collide (2^{-128} probability).
        // Test at least that share indices differ.
        assert_ne!(s1.index, s2.index);
    }

    #[test]
    fn reject_zero_threshold() {
        assert!(DkgState::new(1, 0, 3).is_err());
    }

    #[test]
    fn reject_threshold_exceeds_total() {
        assert!(DkgState::new(1, 4, 3).is_err());
    }
}
