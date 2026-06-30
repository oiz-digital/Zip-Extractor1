//! Pedersen commitments over Ristretto255.
//!
//! A Pedersen commitment to value `v` with blinding factor `r` is:
//!   `C = v·G + r·H`
//!
//! where G and H are independent generator points on the Ristretto255 curve.
//! The commitment is:
//! - **Perfectly hiding**: C reveals nothing about v (with uniformly random r)
//! - **Computationally binding**: Cannot open C to a different v' (discrete-log assumption)
//! - **Additively homomorphic**: C(v₁, r₁) + C(v₂, r₂) = C(v₁+v₂, r₁+r₂)
//!
//! ## ZBX Implementation
//!
//! Uses `curve25519-dalek` v4 Ristretto255 group:
//! - G = Ristretto255 standard base point (RISTRETTO_BASEPOINT_POINT)
//! - H = hash_from_bytes::<Sha512>(b"zbx-pedersen-H-v1") — independent generator
//! - Blinding factors are Ristretto255 scalars (mod group order ℓ ≈ 2^252)
//! - Commitments are 32-byte compressed Ristretto points

use crate::error::ConfidentialError;
use curve25519_dalek::{
    RistrettoPoint, Scalar,
    constants::RISTRETTO_BASEPOINT_POINT,
    ristretto::CompressedRistretto,
    traits::Identity,
};
use sha2::Sha512;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

// ── Generator points ──────────────────────────────────────────────────────────

/// Pedersen generator G: the canonical Ristretto255 base point.
#[inline]
fn generator_g() -> RistrettoPoint {
    RISTRETTO_BASEPOINT_POINT
}

/// Pedersen generator H: independent point derived via hash-to-curve.
///
/// H = Elligator2(SHA-512(b"zbx-pedersen-H-v1")) on the Ristretto255 group.
/// This ensures H is independent from G with no known discrete-log relation.
#[inline]
fn generator_h() -> RistrettoPoint {
    RistrettoPoint::hash_from_bytes::<Sha512>(b"zbx-pedersen-H-v1")
}

// ── Types ─────────────────────────────────────────────────────────────────────

/// A Pedersen commitment C = v·G + r·H, stored as 32-byte compressed Ristretto255 point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PedersenCommitment(pub [u8; 32]);

/// A blinding factor r ∈ ℤ_ℓ (Ristretto scalar, secret, zeroized on drop).
///
/// Stored as 32 canonical little-endian bytes representing a scalar mod ℓ.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct BlindingFactor(pub [u8; 32]);

impl std::fmt::Debug for BlindingFactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BlindingFactor([REDACTED])")
    }
}

/// An opened commitment: value + blinding factor that produced it.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct CommitmentOpening {
    pub value:    u64,
    pub blinding: BlindingFactor,
}

impl std::fmt::Debug for CommitmentOpening {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommitmentOpening")
            .field("value", &self.value)
            .field("blinding", &"[REDACTED]")
            .finish()
    }
}

// ── PedersenCommitment ────────────────────────────────────────────────────────

impl PedersenCommitment {
    /// The identity commitment C(0, 0) — neutral element for homomorphic addition.
    ///
    /// This is the Ristretto255 identity (neutral) point, which compresses to [0u8; 32].
    pub fn zero() -> Self {
        PedersenCommitment(RistrettoPoint::identity().compress().to_bytes())
    }

    /// Commit to a value with a given blinding factor.
    ///
    /// Computes C = v·G + r·H using full elliptic-curve scalar multiplication.
    /// This is the actual Pedersen commitment construction over Ristretto255.
    pub fn commit(value: u64, blinding: &BlindingFactor) -> Self {
        let g = generator_g();
        let h = generator_h();
        let v_scalar = Scalar::from(value);
        let r_scalar = Scalar::from_bytes_mod_order(blinding.0);
        let point = v_scalar * g + r_scalar * h;
        PedersenCommitment(point.compress().to_bytes())
    }

    /// Decompress into a Ristretto255 point. Panics if the bytes are invalid.
    fn to_point(&self) -> RistrettoPoint {
        CompressedRistretto(self.0)
            .decompress()
            .expect("PedersenCommitment: invalid Ristretto255 point encoding")
    }

    /// Homomorphic addition: C(v₁, r₁) + C(v₂, r₂) = C(v₁+v₂, r₁+r₂).
    ///
    /// This holds exactly because:
    ///   (v₁·G + r₁·H) + (v₂·G + r₂·H) = (v₁+v₂)·G + (r₁+r₂)·H
    pub fn add(&self, other: &PedersenCommitment) -> PedersenCommitment {
        let sum = self.to_point() + other.to_point();
        PedersenCommitment(sum.compress().to_bytes())
    }

    /// Homomorphic subtraction: C(v₁, r₁) − C(v₂, r₂) = C(v₁−v₂, r₁−r₂).
    pub fn sub(&self, other: &PedersenCommitment) -> PedersenCommitment {
        let diff = self.to_point() - other.to_point();
        PedersenCommitment(diff.compress().to_bytes())
    }

    /// Verify an opening: check that commit(value, blinding) == self.
    pub fn verify_opening(&self, opening: &CommitmentOpening) -> Result<(), ConfidentialError> {
        let expected = PedersenCommitment::commit(opening.value, &opening.blinding);
        if expected == *self {
            Ok(())
        } else {
            Err(ConfidentialError::CommitmentOpeningFailed)
        }
    }

    pub fn as_bytes(&self) -> &[u8; 32] { &self.0 }
}

// ── BlindingFactor ────────────────────────────────────────────────────────────

impl BlindingFactor {
    /// Generate a random blinding factor using the OS RNG.
    pub fn random<R: rand_core::RngCore>(rng: &mut R) -> Self {
        let mut bytes = [0u8; 32];
        rng.fill_bytes(&mut bytes);
        // Reduce mod ℓ to ensure a uniform scalar.
        let s = Scalar::from_bytes_mod_order(bytes);
        BlindingFactor(s.to_bytes())
    }

    /// Add two blinding factors modulo the Ristretto255 group order ℓ.
    ///
    /// This is the correct scalar addition for the homomorphic commitment scheme:
    /// r₁ + r₂ (mod ℓ) ensures that C(v₁, r₁) + C(v₂, r₂) = C(v₁+v₂, r₁+r₂).
    pub fn add(&self, other: &BlindingFactor) -> BlindingFactor {
        let s1 = Scalar::from_bytes_mod_order(self.0);
        let s2 = Scalar::from_bytes_mod_order(other.0);
        BlindingFactor((s1 + s2).to_bytes())
    }

    /// Subtract blinding factor modulo ℓ.
    pub fn sub(&self, other: &BlindingFactor) -> BlindingFactor {
        let s1 = Scalar::from_bytes_mod_order(self.0);
        let s2 = Scalar::from_bytes_mod_order(other.0);
        BlindingFactor((s1 - s2).to_bytes())
    }

    /// Zero blinding factor — used for fee commitments with transparent amounts.
    pub fn zero() -> Self {
        BlindingFactor([0u8; 32])
    }
}

// ── Balance conservation ──────────────────────────────────────────────────────

/// Verify balance conservation: sum(input_commits) − sum(output_commits) = C(fee, 0).
///
/// If this holds, then: Σv_in = Σv_out + fee, proving no tokens were created
/// or destroyed. This works because of exact Ristretto255 group homomorphism:
///
///   Σ C(v_in_i, r_in_i) − Σ C(v_out_j, r_out_j) = C(fee, 0)
///   ⟺ (Σv_in − Σv_out − fee)·G + (Σr_in − Σr_out)·H = 0
///   ⟺ Σv_in = Σv_out + fee  (if Σr_in = Σr_out, which the prover arranges)
pub fn verify_balance_conservation(
    input_commits:  &[PedersenCommitment],
    output_commits: &[PedersenCommitment],
    fee:            u64,
) -> Result<(), ConfidentialError> {
    if input_commits.is_empty() {
        return Err(ConfidentialError::BalanceConservationFailed);
    }

    // Sum all input commitments.
    let input_sum = input_commits
        .iter()
        .fold(RistrettoPoint::identity(), |acc, c| acc + c.to_point());

    // Fee commitment with zero blinding (transparent fee).
    let zero_blinding = BlindingFactor::zero();
    let fee_commit = PedersenCommitment::commit(fee, &zero_blinding);

    // Sum all output commitments + fee commitment.
    let output_sum = output_commits
        .iter()
        .fold(fee_commit.to_point(), |acc, c| acc + c.to_point());

    if input_sum == output_sum {
        Ok(())
    } else {
        Err(ConfidentialError::BalanceConservationFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    #[test]
    fn commit_and_verify() {
        let r = BlindingFactor::random(&mut OsRng);
        let c = PedersenCommitment::commit(1000, &r);
        let opening = CommitmentOpening { value: 1000, blinding: r };
        assert!(c.verify_opening(&opening).is_ok());
    }

    #[test]
    fn wrong_value_fails() {
        let r = BlindingFactor::random(&mut OsRng);
        let c = PedersenCommitment::commit(1000, &r);
        let bad_opening = CommitmentOpening { value: 999, blinding: r };
        assert!(c.verify_opening(&bad_opening).is_err());
    }

    #[test]
    fn homomorphic_add_is_correct() {
        let mut rng = OsRng;
        let r1 = BlindingFactor::random(&mut rng);
        let r2 = BlindingFactor::random(&mut rng);
        let r_sum = r1.add(&r2);

        let c1 = PedersenCommitment::commit(300, &r1);
        let c2 = PedersenCommitment::commit(700, &r2);
        // C(300, r1) + C(700, r2) must equal C(1000, r1+r2)
        let c_sum_homomorphic = c1.add(&c2);
        let c_sum_direct = PedersenCommitment::commit(1000, &r_sum);
        assert_eq!(c_sum_homomorphic, c_sum_direct, "homomorphic add must hold exactly");
    }

    #[test]
    fn balance_conservation_passes() {
        let mut rng = OsRng;
        let r1 = BlindingFactor::random(&mut rng);
        let r2 = BlindingFactor::random(&mut rng);
        // Prover arranges: r_out = r1 + r2 so blinding cancels
        let r_out = r1.add(&r2);

        let c_in1 = PedersenCommitment::commit(500, &r1);
        let c_in2 = PedersenCommitment::commit(500, &r2);
        let c_out  = PedersenCommitment::commit(990, &r_out);
        let fee    = 10u64;

        let result = verify_balance_conservation(&[c_in1, c_in2], &[c_out], fee);
        assert!(result.is_ok(), "balance conservation must hold: 500+500 = 990+10");
    }

    #[test]
    fn balance_conservation_fails_when_inflated() {
        let mut rng = OsRng;
        let r = BlindingFactor::random(&mut rng);
        let c_in  = PedersenCommitment::commit(100, &r);
        // Attacker claims output of 200 — more than input
        let c_out = PedersenCommitment::commit(200, &r);
        let fee   = 0u64;

        let result = verify_balance_conservation(&[c_in], &[c_out], fee);
        assert!(result.is_err(), "inflated output must fail conservation check");
    }

    #[test]
    fn zero_commitment_is_identity() {
        let zero = PedersenCommitment::zero();
        let r = BlindingFactor::random(&mut OsRng);
        let c = PedersenCommitment::commit(42, &r);
        // zero + c == c
        assert_eq!(zero.add(&c), c);
    }

    #[test]
    fn blinding_factor_random_is_non_zero() {
        let r = BlindingFactor::random(&mut OsRng);
        assert_ne!(r.0, [0u8; 32], "random blinding should not be zero");
    }
}
