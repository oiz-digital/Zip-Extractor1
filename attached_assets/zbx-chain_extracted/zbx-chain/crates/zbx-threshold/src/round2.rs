//! FROST Round 2: Partial signature generation.

use serde::{Serialize, Deserialize};
use crate::{
    keyshare::KeyShare,
    round1::NonceCommitment,
    error::ThresholdError,
    scalar::{partial_sig_scalar, lagrange_coefficient, aggregate_nonce_point},
};
use sha2::{Sha256, Digest};

/// A partial signature from one participant.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PartialSig {
    /// Signer index
    pub index:   u32,
    /// Partial signature scalar z_i (canonical, ≤ n−1).
    pub z:       [u8; 32],
}

/// Compute a partial signature in Round 2 of FROST.
///
/// The signer applies the canonical FROST equation
///
///     z_i = d_i + ρ_i · e_i + λ_i · c · s_i   (mod n)
///
/// where `d_i, e_i` are the local Round-1 nonces, `ρ_i` is the per-signer
/// binding factor, `λ_i` the Lagrange coefficient over the participating
/// set, `c` the Schnorr challenge, and `s_i` this signer's secret share.
///
/// # Inputs
/// * `key_share`     — this participant's key share.
/// * `nonces`        — `(d_i, e_i)` from Round 1, kept locally.
/// * `group_commits` — every participant's Round-1 commitments `(D_i, E_i)`.
/// * `message`       — the message being signed (e.g. block hash).
pub fn sign(
    key_share:     &KeyShare,
    nonces:        ([u8; 32], [u8; 32]),
    group_commits: &[NonceCommitment],
    message:       &[u8],
) -> Result<PartialSig, ThresholdError> {
    if group_commits.is_empty() {
        return Err(ThresholdError::EmptySignerSet);
    }
    if group_commits.len() < key_share.threshold as usize {
        return Err(ThresholdError::InsufficientSigners {
            required: key_share.threshold as usize,
            got: group_commits.len(),
        });
    }
    if !group_commits.iter().any(|c| c.index == key_share.index) {
        return Err(ThresholdError::InvalidShare(
            format!("self index {} not in commitment set", key_share.index)
        ));
    }

    // Lagrange coefficient over the participating signer set, evaluated at x=0.
    let participants: Vec<u32> = group_commits.iter().map(|c| c.index).collect();
    let lambda_i = lagrange_coefficient(key_share.index, &participants)?;

    // Per-signer binding factors ρ_i = H(i, msg, B) — caller-side reproducible.
    let binding_factors: Vec<(u32, [u8; 32])> = group_commits.iter()
        .map(|c| (c.index, compute_binding_factor(c.index, message, group_commits)))
        .collect();
    let rho_i = binding_factors.iter()
        .find(|(i, _)| *i == key_share.index)
        .map(|(_, b)| *b)
        .expect("present by construction (we just inserted self)");

    // Aggregate nonce R = Σ (D_j + ρ_j · E_j) — REAL group-element addition.
    let commits_tuples: Vec<(u32, [u8; 33], [u8; 33])> = group_commits.iter()
        .map(|c| (c.index, c.D, c.E))
        .collect();
    let r_bytes = aggregate_nonce_point(&commits_tuples, &binding_factors)?;

    // Challenge c = H(R, group_key, msg) — reduced to a scalar.
    let challenge = compute_challenge(&r_bytes, &key_share.group_key, message);

    // z_i — REAL field arithmetic (replaces the previous byte-XOR stub).
    let z = partial_sig_scalar(
        &nonces.0,
        &nonces.1,
        &rho_i,
        &challenge,
        &lambda_i,
        key_share.secret(),
    )?;

    tracing::debug!(
        signer = key_share.index,
        "FROST Round 2: partial signature generated"
    );

    Ok(PartialSig { index: key_share.index, z })
}

/// Compute the binding factor ρ_i = H(domain || index_le || msg || B).
///
/// `B` is the canonical concatenation of every signer's `(index, D, E)` in
/// the order supplied to this function. All participants compute the same
/// factor for the same `(index, msg, commits)` tuple, which is what makes
/// FROST's R reproducible across signers without an extra round.
pub fn compute_binding_factor(index: u32, msg: &[u8], commits: &[NonceCommitment]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"frost:binding:v2:");
    h.update(index.to_le_bytes());
    h.update((msg.len() as u64).to_le_bytes());
    h.update(msg);
    for c in commits {
        h.update(c.index.to_le_bytes());
        h.update(c.D);
        h.update(c.E);
    }
    h.finalize().into()
}

/// Compute the Schnorr challenge c = H(R, group_key, msg).
pub fn compute_challenge(r: &[u8; 33], group_key: &[u8; 33], msg: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"frost:challenge:v2:");
    h.update(r);
    h.update(group_key);
    h.update((msg.len() as u64).to_le_bytes());
    h.update(msg);
    h.finalize().into()
}
