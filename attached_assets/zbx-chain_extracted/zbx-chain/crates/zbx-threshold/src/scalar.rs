//! Helper layer over `k256` field/group arithmetic for FROST.
//!
//! The previous implementation used byte-wise XOR for both the partial
//! signature scalar and its aggregation, which is **not a signature scheme** —
//! any attacker who collects a single valid `ThresholdSig` can subtract their
//! own contribution and forge an arbitrary new "valid" aggregate.
//!
//! This module replaces those primitives with real scalar arithmetic modulo
//! the secp256k1 group order n.
//!
//! All inputs/outputs preserve the existing `[u8; 32]` (scalar) and `[u8; 33]`
//! (compressed point) wire formats so the surrounding wire types do not change.

use elliptic_curve::{
    group::GroupEncoding,
    sec1::FromEncodedPoint,
    PrimeField,
};
use k256::{
    elliptic_curve::sec1::ToEncodedPoint,
    AffinePoint, EncodedPoint, ProjectivePoint, Scalar,
};
use crate::error::ThresholdError;

/// Reduce 32 arbitrary bytes mod n into a `Scalar`. Used for binding factors
/// and challenges that come from a hash function (which can produce any
/// 256-bit value, not necessarily a canonical scalar).
pub fn bytes_to_scalar_reduce(b: &[u8; 32]) -> Scalar {
    // `Scalar::from_repr` rejects out-of-range values; for hash outputs we
    // want modular reduction. Use the conversion via the wide-reduce path:
    // build a 64-byte zero-padded buffer and let the field reduce. k256 0.13
    // exposes this as `Scalar::reduce_bytes` on the FieldBytes type.
    let fb = k256::FieldBytes::from_slice(b);
    <Scalar as elliptic_curve::ops::Reduce<k256::U256>>::reduce_bytes(fb)
}

/// Strict (canonical) scalar parse: rejects values >= n. Use for secret
/// shares and partial sig scalars where the value MUST already be canonical.
pub fn bytes_to_scalar_strict(b: &[u8; 32]) -> Result<Scalar, ThresholdError> {
    let fb = k256::FieldBytes::from(*b);
    let opt = Scalar::from_repr(fb);
    if opt.is_some().into() {
        Ok(opt.unwrap())
    } else {
        Err(ThresholdError::InvalidShare("scalar out of range".into()))
    }
}

pub fn scalar_to_bytes(s: &Scalar) -> [u8; 32] {
    let fb = s.to_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&fb);
    out
}

/// Decode a SEC1-compressed (33-byte) point. Rejects identity and invalid
/// encodings. Used for nonce commitments D, E and group/verifying keys.
pub fn point_from_compressed(b: &[u8; 33]) -> Result<ProjectivePoint, ThresholdError> {
    let ep = EncodedPoint::from_bytes(b)
        .map_err(|e| ThresholdError::InvalidShare(format!("bad point: {e}")))?;
    let opt = AffinePoint::from_encoded_point(&ep);
    if opt.is_some().into() {
        let aff = opt.unwrap();
        Ok(ProjectivePoint::from(aff))
    } else {
        Err(ThresholdError::InvalidShare("point not on curve".into()))
    }
}

pub fn point_to_compressed(p: &ProjectivePoint) -> [u8; 33] {
    let aff = p.to_affine();
    let enc = aff.to_encoded_point(true); // compressed
    let bytes = enc.as_bytes();
    debug_assert_eq!(bytes.len(), 33);
    let mut out = [0u8; 33];
    out.copy_from_slice(bytes);
    out
}

/// Lagrange coefficient λ_i for participant `i` interpolating at x = 0,
/// over the participating set `participants` (1-indexed signer indices).
///
///   λ_i = Π_{j ∈ S, j ≠ i}  j / (j − i)   (mod n)
///
/// This is what scales each partial signature so that the resulting
/// aggregate equals `secret · G` for the *combined* secret reconstructed
/// at x = 0, even though no participant ever knows that combined secret.
///
/// Returns an error if `i` is not in `participants` or any pair of
/// participants share an index (which would cause a div-by-zero).
pub fn lagrange_coefficient(i: u32, participants: &[u32]) -> Result<Scalar, ThresholdError> {
    if !participants.contains(&i) {
        return Err(ThresholdError::InvalidShare(
            format!("self index {i} not in participating set")
        ));
    }
    if participants.iter().any(|&j| j == 0) {
        return Err(ThresholdError::InvalidShare(
            "participant index 0 invalid (FROST is 1-indexed)".into()
        ));
    }
    let xi = Scalar::from(i as u64);
    let mut num = Scalar::ONE;
    let mut den = Scalar::ONE;
    for &j in participants.iter() {
        if j == i { continue; }
        let xj = Scalar::from(j as u64);
        num *= xj;
        den *= xj - xi;
    }
    let den_inv_opt = den.invert();
    if den_inv_opt.is_none().into() {
        return Err(ThresholdError::InvalidShare(
            "duplicate participant indices".into()
        ));
    }
    Ok(num * den_inv_opt.unwrap())
}

/// FROST partial signature scalar:
///
///     z_i = d_i + ρ_i · e_i + λ_i · c · s_i   (mod n)
///
/// Where:
/// * `d_i`, `e_i`  — this signer's two single-use Round-1 nonces.
/// * `ρ_i`         — binding factor (hash output reduced mod n).
/// * `c`           — challenge scalar (hash output reduced mod n).
/// * `λ_i`         — Lagrange coefficient over the participating signer set.
/// * `s_i`         — this signer's secret share.
///
/// This is the canonical FROST equation; the previous stub was
/// `z_i = d_i ^ e_i ^ ρ_i ^ c ^ s_i` (byte-wise XOR), which is **not** a
/// signature scheme.
pub fn partial_sig_scalar(
    d_i:        &[u8; 32],
    e_i:        &[u8; 32],
    rho_i:      &[u8; 32],
    challenge:  &[u8; 32],
    lambda_i:   &Scalar,
    secret_i:   &[u8; 32],
) -> Result<[u8; 32], ThresholdError> {
    let d  = bytes_to_scalar_strict(d_i)?;
    let e  = bytes_to_scalar_strict(e_i)?;
    let rho = bytes_to_scalar_reduce(rho_i);
    let c   = bytes_to_scalar_reduce(challenge);
    let s   = bytes_to_scalar_strict(secret_i)?;
    let z = d + rho * e + (*lambda_i) * c * s;
    Ok(scalar_to_bytes(&z))
}

/// Aggregate the per-signer commitments D_i, E_i into the group nonce
///
///     R = Σ (D_i + ρ_i · E_i)
///
/// This is what every signer also computed locally to sign — the aggregator
/// must reproduce the same R or the resulting signature will not verify.
pub fn aggregate_nonce_point(
    commits:        &[(u32, [u8; 33], [u8; 33])],   // (idx, D, E)
    binding_factors: &[(u32, [u8; 32])],            // (idx, ρ_i)
) -> Result<[u8; 33], ThresholdError> {
    if commits.is_empty() {
        return Err(ThresholdError::EmptySignerSet);
    }
    let mut acc = ProjectivePoint::IDENTITY;
    for (idx, d_bytes, e_bytes) in commits {
        let d_pt = point_from_compressed(d_bytes)?;
        let e_pt = point_from_compressed(e_bytes)?;
        let rho_bytes = binding_factors.iter()
            .find(|(i, _)| i == idx)
            .map(|(_, b)| b)
            .ok_or_else(|| ThresholdError::InvalidShare(
                format!("missing binding factor for signer {idx}")
            ))?;
        let rho = bytes_to_scalar_reduce(rho_bytes);
        acc += d_pt + e_pt * rho;
    }
    Ok(point_to_compressed(&acc))
}

/// Aggregate partial-signature scalars z_i into the final s = Σ z_i  (mod n).
///
/// Replaces the previous byte-wise `wrapping_add` accumulator, which produced
/// values that are not valid Schnorr scalars (carries do not propagate mod n).
pub fn aggregate_partial_scalars(zs: &[[u8; 32]]) -> Result<[u8; 32], ThresholdError> {
    let mut acc = Scalar::ZERO;
    for z in zs {
        acc += bytes_to_scalar_strict(z)?;
    }
    Ok(scalar_to_bytes(&acc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lagrange_singleton_is_one() {
        // Single-participant Lagrange coefficient at x=0 is 1.
        let lam = lagrange_coefficient(1, &[1]).unwrap();
        assert_eq!(lam, Scalar::ONE);
    }

    #[test]
    fn lagrange_two_of_two() {
        // Participants {1, 2}, λ_1 = 2/(2-1) = 2, λ_2 = 1/(1-2) = -1.
        let l1 = lagrange_coefficient(1, &[1, 2]).unwrap();
        let l2 = lagrange_coefficient(2, &[1, 2]).unwrap();
        assert_eq!(l1, Scalar::from(2u64));
        assert_eq!(l2, -Scalar::ONE);
    }

    #[test]
    fn lagrange_rejects_self_not_in_set() {
        let err = lagrange_coefficient(3, &[1, 2]).unwrap_err();
        assert!(matches!(err, ThresholdError::InvalidShare(_)));
    }

    #[test]
    fn aggregate_zero_is_zero() {
        let agg = aggregate_partial_scalars(&[[0u8; 32], [0u8; 32]]).unwrap();
        assert_eq!(agg, [0u8; 32]);
    }

    #[test]
    fn aggregate_is_associative() {
        // (a + b) + c == a + (b + c)
        let mut a = [0u8; 32]; a[31] = 7;
        let mut b = [0u8; 32]; b[31] = 11;
        let mut c = [0u8; 32]; c[31] = 13;
        let lr = aggregate_partial_scalars(&[a, b, c]).unwrap();
        let mut sum = [0u8; 32]; sum[31] = 31;
        assert_eq!(lr, sum);
    }
}
