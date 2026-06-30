//! FROST signature aggregation (combiner role).

use crate::{
    round1::NonceCommitment,
    round2::PartialSig,
    error::ThresholdError,
    scalar::aggregate_partial_scalars,
};
use serde_big_array::BigArray;
use serde::{Serialize, Deserialize};

/// Aggregated threshold Schnorr signature — identical wire format to a
/// normal Schnorr sig (R || s). Verifiable by anyone without knowing the
/// signature was threshold-produced.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[allow(non_snake_case)]
pub struct ThresholdSig {
    /// R: aggregate nonce commitment (33 bytes, compressed secp256k1 point).
    #[serde(with = "BigArray")]
    pub R:        [u8; 33],
    /// s: aggregate scalar (32 bytes, mod n).
    pub s:        [u8; 32],
    /// Indices of the participants whose partial sigs were combined here.
    /// Recorded for accountability — not part of the verification equation.
    pub signers:  Vec<u32>,
}

impl ThresholdSig {
    /// Serialize to 65 bytes (R || s) — standard Schnorr signature wire format.
    pub fn to_bytes(&self) -> [u8; 65] {
        let mut out = [0u8; 65];
        out[..33].copy_from_slice(&self.R);
        out[33..].copy_from_slice(&self.s);
        out
    }

    pub fn to_hex(&self) -> String { hex::encode(self.to_bytes()) }
}

/// Aggregate t partial signatures into one threshold signature.
///
/// # Inputs
///
/// * `partials` — at least `threshold` partial signatures from distinct signers.
///   Each `PartialSig.z` is a canonical scalar (≤ n−1) computed by
///   `crate::round2::sign` using the FROST partial-sig equation.
/// * `commits`  — the matching Round-1 nonce commitments (D_i, E_i) for each
///   signer. Used to deterministically reconstruct R when the caller supplies
///   `Some(R_bytes)`. The aggregator does not recompute R from commits here
///   because doing so requires the message and binding factors that only the
///   signing protocol knows; instead the caller (already privy to the
///   message) precomputes R and passes it via `precomputed_r`.
/// * `threshold` — minimum number of signers required.
/// * `precomputed_r` — the group nonce R = Σ (D_i + ρ_i · E_i) that every
///   signer also computed locally to sign. Use `crate::scalar::aggregate_nonce_point`
///   to derive this. If `None`, `R` will be set to the first commit's D for
///   wire-format compatibility (legacy callers); the resulting signature
///   will not verify and a warning is logged. Production callers MUST pass
///   `Some(...)`.
///
/// # Returns
///
/// A `ThresholdSig` with `s = Σ z_i  (mod n)` — the canonical FROST aggregate.
pub fn aggregate(
    partials:      &[PartialSig],
    commits:       &[NonceCommitment],
    threshold:     usize,
    precomputed_r: Option<[u8; 33]>,
) -> Result<ThresholdSig, ThresholdError> {
    if partials.len() < threshold {
        return Err(ThresholdError::InsufficientSigners {
            required: threshold,
            got: partials.len(),
        });
    }

    // Pick the first `threshold` partials with unique signer indices, in the
    // order they were submitted. Reject duplicates — a single signer cannot
    // contribute twice toward the threshold.
    let mut active:  Vec<&PartialSig> = Vec::with_capacity(threshold);
    let mut seen:    std::collections::HashSet<u32> = std::collections::HashSet::new();
    for p in partials {
        if !seen.insert(p.index) {
            return Err(ThresholdError::InvalidShare(
                format!("duplicate partial signature from signer {}", p.index)
            ));
        }
        active.push(p);
        if active.len() == threshold { break; }
    }
    if active.len() < threshold {
        return Err(ThresholdError::InsufficientSigners {
            required: threshold,
            got: active.len(),
        });
    }

    // Verify every active signer has a matching Round-1 commitment.
    for p in &active {
        if !commits.iter().any(|c| c.index == p.index) {
            return Err(ThresholdError::InvalidShare(
                format!("missing Round-1 commitment for signer {}", p.index)
            ));
        }
    }

    // s = Σ z_i  (mod n) — REAL field addition, not byte-wise wrapping_add.
    let zs: Vec<[u8; 32]> = active.iter().map(|p| p.z).collect();
    let s = aggregate_partial_scalars(&zs)?;

    // R is the group nonce. Caller is responsible for supplying it because
    // computing R requires the per-signer binding factors ρ_i = H(i, msg, B),
    // which depend on the message that the aggregator may not have.
    let r = match precomputed_r {
        Some(bytes) => bytes,
        None => {
            tracing::warn!(
                "FROST aggregate: precomputed_r not supplied; resulting signature will not verify"
            );
            commits.first().map(|c| c.D).unwrap_or([0u8; 33])
        }
    };

    let signers: Vec<u32> = active.iter().map(|p| p.index).collect();

    tracing::info!(
        signers = ?signers,
        threshold,
        "FROST: threshold signature aggregated"
    );

    Ok(ThresholdSig { R: r, s, signers })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round2::PartialSig;
    use crate::round1::NonceCommitment;

    fn fake_partial(idx: u32) -> PartialSig {
        // z must be a canonical scalar; small constants are always < n.
        let mut z = [0u8; 32];
        z[31] = idx as u8;
        PartialSig { index: idx, z }
    }
    fn fake_commit(idx: u32) -> NonceCommitment {
        // D, E need not be valid points for these aggregator-level tests
        // (the aggregator only checks their indices match partials).
        // Use a SEC1-valid compressed point: 0x02 || x where x = generator's x.
        let mut g = [0u8; 33];
        g[0] = 0x02;
        // x-coordinate of secp256k1 generator
        g[1..].copy_from_slice(&hex::decode(
            "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
        ).unwrap());
        NonceCommitment { index: idx, D: g, E: g }
    }

    #[test]
    fn aggregate_threshold_sums_scalars() {
        let partials = vec![fake_partial(1), fake_partial(2), fake_partial(3)];
        let commits  = vec![fake_commit(1),  fake_commit(2),  fake_commit(3)];
        let sig = aggregate(&partials, &commits, 2, Some(commits[0].D)).unwrap();
        assert_eq!(sig.signers, vec![1, 2]);
        // s should be 1+2 = 3 (with byte arithmetic since these scalars are tiny).
        assert_eq!(sig.s[31], 3);
        for &b in &sig.s[..31] { assert_eq!(b, 0); }
    }

    #[test]
    fn rejects_duplicate_signer() {
        let partials = vec![fake_partial(1), fake_partial(1), fake_partial(2)];
        let commits  = vec![fake_commit(1),  fake_commit(2)];
        let err = aggregate(&partials, &commits, 2, Some(commits[0].D)).unwrap_err();
        assert!(matches!(err, ThresholdError::InvalidShare(_)));
    }

    #[test]
    fn rejects_missing_commitment() {
        let partials = vec![fake_partial(1), fake_partial(2)];
        let commits  = vec![fake_commit(1)];
        let err = aggregate(&partials, &commits, 2, Some(commits[0].D)).unwrap_err();
        assert!(matches!(err, ThresholdError::InvalidShare(_)));
    }

    #[test]
    fn insufficient_signers() {
        let partials = vec![fake_partial(1)];
        let commits  = vec![fake_commit(1)];
        let err = aggregate(&partials, &commits, 2, Some(commits[0].D)).unwrap_err();
        assert!(matches!(err, ThresholdError::InsufficientSigners { .. }));
    }

    #[test]
    fn serializes_to_65_bytes() {
        let partials = (1..=3).map(fake_partial).collect::<Vec<_>>();
        let commits  = (1..=3).map(fake_commit).collect::<Vec<_>>();
        let sig = aggregate(&partials, &commits, 2, Some(commits[0].D)).unwrap();
        assert_eq!(sig.to_bytes().len(), 65);
    }
}
