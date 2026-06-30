//! KZG polynomial-commitment verification for EIP-4844 point evaluation
//! (precompile `0x0B`).
//!
//! Task #4 — replaces the Pass-18 `Err(InvalidInput)` fail-closed body. The
//! verifier is built on the `bls12_381` crate already pulled in by `bls.rs`
//! (no `c-kzg` C dep — sandbox-buildable).
//!
//! # Cryptographic check (EIP-4844 §"verify_kzg_proof_impl")
//!
//! Given commitment `C ∈ G1`, evaluation point `z ∈ Fr`, claimed value
//! `y ∈ Fr`, and proof `π ∈ G1`, accept iff
//!
//!   e(C − [y]·G1,  G2)  ==  e(π,  [s]·G2 − [z]·G2)
//!
//! where `[s]·G2` is the lone trusted-setup element this verifier needs.
//! (The full Ethereum ceremony output stores 4096 G1 + 65 G2 points; only
//! `setup.g2[1] = [s]·G2` is needed for the point-evaluation precompile.)
//!
//! # Versioned-hash binding
//!
//! [`kzg_to_versioned_hash`] computes `0x01 ‖ sha256(commitment)[1..]` per
//! EIP-4844 §"kzg_to_versioned_hash"; the precompile rejects any input
//! whose first 32 bytes don't match this digest, binding the on-chain
//! `BLOBHASH` opcode result to the off-chain commitment.
//!
//! # Trusted setup
//!
//! [`load_trusted_setup`] reads a single line of hex (96 bytes) from the
//! supplied path — the compressed serialization of `[s]·G2`. Production
//! pins Ethereum's mainnet ceremony output by extracting `g2_monomial[1]`
//! into this file. Tests use [`KzgSettings::from_g2_compressed`] directly.
//!
//! # Globals
//!
//! [`init_global_kzg_settings`] sets the process-wide `OnceLock` consulted
//! by [`global_kzg_settings`]; precompile dispatchers in `zbx-zvm` and
//! `zbx-evm` look it up here so both engines share one source of truth.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use bls12_381::{G1Affine, G1Projective, G2Affine, G2Projective, Scalar};
use group::Curve;
use pairing::PairingCurveAffine;
use sha2::{Digest as _, Sha256};

/// Versioned-hash domain byte (EIP-4844). The first byte of every blob's
/// versioned hash MUST equal this value.
pub const BLOB_COMMITMENT_VERSION_KZG: u8 = 0x01;

/// EIP-4844 success-return constants.
///   FIELD_ELEMENTS_PER_BLOB = 4096
pub const FIELD_ELEMENTS_PER_BLOB: u64 = 4096;
/// BLS12-381 scalar-field modulus, big-endian.
pub const BLS_MODULUS_BE: [u8; 32] = [
    0x73, 0xed, 0xa7, 0x53, 0x29, 0x9d, 0x7d, 0x48,
    0x33, 0x39, 0xd8, 0x08, 0x09, 0xa1, 0xd8, 0x05,
    0x53, 0xbd, 0xa4, 0x02, 0xff, 0xfe, 0x5b, 0xfe,
    0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x01,
];

/// Errors surfaced from KZG verification.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum KzgError {
    #[error("KZG: trusted setup file not found / unreadable: {0}")]
    SetupRead(String),
    #[error("KZG: trusted setup payload must be 96 hex bytes (compressed G2)")]
    SetupSize,
    #[error("KZG: trusted setup is not a valid G2 point")]
    SetupInvalidG2,
    #[error("KZG: invalid input length; expected 192, got {0}")]
    InputLength(usize),
    #[error("KZG: versioned hash mismatch")]
    VersionedHash,
    #[error("KZG: scalar (z or y) is not canonically reduced mod r")]
    ScalarNotInField,
    #[error("KZG: commitment is not a valid compressed G1 point")]
    BadCommitment,
    #[error("KZG: proof is not a valid compressed G1 point")]
    BadProof,
    #[error("KZG: pairing check failed")]
    PairingFailed,
    #[error("KZG: out of gas (need {need}, have {have})")]
    OutOfGas { need: u64, have: u64 },
}

/// Trusted-setup material required by point-evaluation: `[s]·G2`.
#[derive(Clone, Debug)]
pub struct KzgSettings {
    pub s_g2: G2Affine,
}

impl KzgSettings {
    /// Build from a 96-byte compressed-G2 serialization. Returns
    /// [`KzgError::SetupInvalidG2`] if decoding fails.
    pub fn from_g2_compressed(bytes: &[u8; 96]) -> Result<Self, KzgError> {
        let opt = G2Affine::from_compressed(bytes);
        if bool::from(opt.is_some()) {
            Ok(KzgSettings { s_g2: opt.unwrap() })
        } else {
            Err(KzgError::SetupInvalidG2)
        }
    }
}

/// Read a trusted-setup file. Two formats accepted:
///
/// **Ceremony format (canonical, c-kzg / Ethereum mainnet):**
/// ```text
/// 4096                       # FIELD_ELEMENTS_PER_BLOB
/// 65                         # number of G2 points
/// <96 hex chars>             # G1 monomial[0]   (4096 lines)
/// ...
/// <192 hex chars>            # G2 monomial[0..=64]   (65 lines)
/// ...
/// ```
/// Only `g2_monomial[1] = [s]·G2` is consumed by the point-evaluation
/// precompile; the remaining ceremony output is parsed for header
/// validation but ignored. The mainnet-ceremony provenance is documented
/// in `node/configs/trusted_setup_*.txt` headers.
///
/// **Legacy compact format (kept for unit tests and small-footprint
/// embeds):** a single non-comment line of 192 hex chars = 96 bytes =
/// compressed-G2 serialization of `[s]·G2`.
///
/// Lines starting with `#` and blank lines are ignored in both formats.
pub fn load_trusted_setup<P: AsRef<Path>>(path: P) -> Result<KzgSettings, KzgError> {
    let raw = std::fs::read_to_string(path.as_ref())
        .map_err(|e| KzgError::SetupRead(e.to_string()))?;

    // Collect non-comment, non-blank lines preserving order.
    let mut lines: Vec<&str> = Vec::new();
    for line in raw.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        lines.push(l);
    }
    if lines.is_empty() {
        return Err(KzgError::SetupSize);
    }

    // Detect ceremony format: first two lines are decimal counts.
    let header_pair = lines
        .get(0)
        .and_then(|a| a.parse::<usize>().ok())
        .zip(lines.get(1).and_then(|b| b.parse::<usize>().ok()));

    if let Some((n_g1, n_g2)) = header_pair {
        // Sanity: refuse anything other than the EIP-4844 / c-kzg shape.
        // (4096 G1 + 65 G2). We only consume g2_monomial[1] here, but
        // enforcing the shape catches truncation / wrong-file-type errors
        // at load time rather than at first precompile call.
        if n_g1 != 4096 || n_g2 != 65 {
            return Err(KzgError::SetupSize);
        }
        // Two accepted layouts (both used by the Ethereum / c-kzg
        // ecosystem). In BOTH layouts G1 monomial directly precedes G2
        // monomial, so the index of `g2_monomial[1] = [s]·G2` is the
        // same: `2 + n_g1 + 1`.
        //
        //   * Compact (monomial-only):
        //       2 (header) + n_g1 (G1 monomial) + n_g2 (G2 monomial)
        //       = 2 + 4096 + 65 = 4163 lines
        //
        //   * Full c-kzg trusted_setup.txt:
        //       2 (header) + n_g1 (G1 monomial) + n_g2 (G2 monomial)
        //       + n_g1 (G1 Lagrange)
        //       = 2 + 4096 + 65 + 4096 = 8259 lines
        let compact_total = 2 + n_g1 + n_g2;
        let full_total = compact_total + n_g1;
        let has_lagrange = if lines.len() == full_total {
            true
        } else if lines.len() == compact_total {
            false
        } else {
            return Err(KzgError::SetupSize);
        };
        // Length-check every G1 monomial line (we don't decompress —
        // point evaluation never touches them — but the length check
        // catches gross corruption).
        for g1_line in &lines[2..2 + n_g1] {
            let s = g1_line.strip_prefix("0x").unwrap_or(g1_line);
            let b = hex::decode(s).map_err(|_| KzgError::SetupSize)?;
            if b.len() != 48 {
                return Err(KzgError::SetupSize);
            }
        }
        // g2_monomial[0] is the G2 generator; g2_monomial[1] is [s]·G2.
        let g2_idx_s = 2 + n_g1 + 1;
        let s_g2_line = lines[g2_idx_s];
        let hexstr = s_g2_line.strip_prefix("0x").unwrap_or(s_g2_line);
        let bytes = hex::decode(hexstr).map_err(|_| KzgError::SetupSize)?;
        if bytes.len() != 96 {
            return Err(KzgError::SetupSize);
        }
        // Length-check remaining G2 monomial lines too.
        for g2_line in &lines[2 + n_g1..2 + n_g1 + n_g2] {
            let s = g2_line.strip_prefix("0x").unwrap_or(g2_line);
            let b = hex::decode(s).map_err(|_| KzgError::SetupSize)?;
            if b.len() != 96 {
                return Err(KzgError::SetupSize);
            }
        }
        // Length-check trailing G1 Lagrange block when present.
        if has_lagrange {
            for g1_line in &lines[2 + n_g1 + n_g2..2 + n_g1 + n_g2 + n_g1] {
                let s = g1_line.strip_prefix("0x").unwrap_or(g1_line);
                let b = hex::decode(s).map_err(|_| KzgError::SetupSize)?;
                if b.len() != 48 {
                    return Err(KzgError::SetupSize);
                }
            }
        }
        let mut arr = [0u8; 96];
        arr.copy_from_slice(&bytes);
        return KzgSettings::from_g2_compressed(&arr);
    }

    // Legacy compact format: a single line of 192 hex chars.
    let line = lines[0];
    let hexstr = line.strip_prefix("0x").unwrap_or(line);
    let bytes = hex::decode(hexstr).map_err(|_| KzgError::SetupSize)?;
    if bytes.len() != 96 {
        return Err(KzgError::SetupSize);
    }
    let mut arr = [0u8; 96];
    arr.copy_from_slice(&bytes);
    KzgSettings::from_g2_compressed(&arr)
}

/// Process-wide trusted setup. Set once at node startup via
/// [`init_global_kzg_settings`]; consumed by precompile dispatchers.
static GLOBAL_KZG: OnceLock<Arc<KzgSettings>> = OnceLock::new();

/// Install the global trusted setup. Idempotent: subsequent calls are
/// silently ignored (returning `false`) so concurrent inits race-free.
pub fn init_global_kzg_settings(s: KzgSettings) -> bool {
    GLOBAL_KZG.set(Arc::new(s)).is_ok()
}

/// Fetch the global trusted setup, if installed.
pub fn global_kzg_settings() -> Option<Arc<KzgSettings>> {
    GLOBAL_KZG.get().cloned()
}

/// EIP-4844 versioned hash: `0x01 ‖ sha256(commitment_48)[1..]`.
pub fn kzg_to_versioned_hash(commitment_48: &[u8; 48]) -> [u8; 32] {
    let mut h = Sha256::digest(commitment_48);
    h[0] = BLOB_COMMITMENT_VERSION_KZG;
    let mut out = [0u8; 32];
    out.copy_from_slice(&h);
    out
}

/// Decode a 32-byte big-endian scalar, requiring it lies in `[0, r)`.
fn scalar_from_be32(b: &[u8; 32]) -> Option<Scalar> {
    // bls12_381::Scalar::from_bytes expects little-endian and rejects
    // non-canonical reps; flip endianness then defer to it.
    let mut le = [0u8; 32];
    for (i, &x) in b.iter().enumerate() {
        le[31 - i] = x;
    }
    let opt = Scalar::from_bytes(&le);
    if bool::from(opt.is_some()) {
        Some(opt.unwrap())
    } else {
        None
    }
}

fn g1_from_compressed(b: &[u8; 48]) -> Option<G1Affine> {
    let opt = G1Affine::from_compressed(b);
    if bool::from(opt.is_some()) { Some(opt.unwrap()) } else { None }
}

/// Verify a single KZG point-evaluation proof.
///
/// Returns `Ok(())` on a passing pairing check; `Err(KzgError)` on any
/// input-decoding failure or pairing-check failure. Inputs match the
/// EIP-4844 wire layout (commitment / proof are compressed G1; z / y are
/// big-endian scalars).
pub fn verify_kzg_proof(
    commitment_48: &[u8; 48],
    z_be32: &[u8; 32],
    y_be32: &[u8; 32],
    proof_48: &[u8; 48],
    settings: &KzgSettings,
) -> Result<(), KzgError> {
    let commitment = g1_from_compressed(commitment_48).ok_or(KzgError::BadCommitment)?;
    let proof      = g1_from_compressed(proof_48).ok_or(KzgError::BadProof)?;
    let z = scalar_from_be32(z_be32).ok_or(KzgError::ScalarNotInField)?;
    let y = scalar_from_be32(y_be32).ok_or(KzgError::ScalarNotInField)?;

    // LHS:  C − [y]·G1
    let g1_y = G1Projective::generator() * y;
    let lhs_g1 = (G1Projective::from(commitment) - g1_y).to_affine();

    // RHS_g2: [s]·G2 − [z]·G2  =  ([s] − [z])·G2 (we have s·G2 and compute z·G2)
    let g2_z = G2Projective::generator() * z;
    let rhs_g2 = (G2Projective::from(settings.s_g2) - g2_z).to_affine();

    // e(C − y·G1,  G2)  ==  e(π,  s·G2 − z·G2)
    let lhs = lhs_g1.pairing_with(&G2Affine::generator());
    let rhs = proof.pairing_with(&rhs_g2);

    if lhs == rhs {
        Ok(())
    } else {
        Err(KzgError::PairingFailed)
    }
}

/// EIP-4844 fixed precompile gas cost.
pub const POINT_EVALUATION_PRECOMPILE_GAS: u64 = 50_000;

/// Run the EIP-4844 point-evaluation precompile (`0x0B`).
///
/// Input layout (192 bytes, exact length required by spec):
///
/// ```text
/// [   0..32 ]  versioned_hash  (== 0x01 ‖ sha256(commitment)[1..])
/// [  32..64 ]  z   — evaluation point   (BE u256, < r)
/// [  64..96 ]  y   — claimed value      (BE u256, < r)
/// [  96..144]  commitment   (compressed G1, 48 B)
/// [ 144..192]  proof        (compressed G1, 48 B)
/// ```
///
/// Returns `(out64, 50_000)` on success where `out64` is
/// `U256(FIELD_ELEMENTS_PER_BLOB) ‖ U256(BLS_MODULUS)`.
///
/// Both VM dispatchers (`zbx-zvm` + `zbx-evm`) call into this single
/// implementation so the engines cannot drift on consensus-critical
/// behaviour. Caller is responsible for the gas-availability check
/// (the function still validates `gas < cost` to keep callers honest).
pub fn do_kzg_point_eval(
    input: &[u8],
    gas: u64,
    settings: &KzgSettings,
) -> Result<(Vec<u8>, u64), KzgError> {
    if input.len() != 192 {
        return Err(KzgError::InputLength(input.len()));
    }
    let cost = POINT_EVALUATION_PRECOMPILE_GAS;
    if gas < cost {
        return Err(KzgError::OutOfGas { need: cost, have: gas });
    }

    let mut commitment = [0u8; 48];
    commitment.copy_from_slice(&input[96..144]);
    let mut proof = [0u8; 48];
    proof.copy_from_slice(&input[144..192]);
    let mut z = [0u8; 32];
    z.copy_from_slice(&input[32..64]);
    let mut y = [0u8; 32];
    y.copy_from_slice(&input[64..96]);

    // Bind the on-chain blob hash to the supplied commitment.
    let expected_vh = kzg_to_versioned_hash(&commitment);
    if input[0..32] != expected_vh {
        return Err(KzgError::VersionedHash);
    }

    verify_kzg_proof(&commitment, &z, &y, &proof, settings)?;

    Ok((point_evaluation_success_return().to_vec(), cost))
}

/// Encode the EIP-4844 success return value:
///   `U256(FIELD_ELEMENTS_PER_BLOB) ‖ U256(BLS_MODULUS)`  (64 bytes).
pub fn point_evaluation_success_return() -> [u8; 64] {
    let mut out = [0u8; 64];
    let fe = FIELD_ELEMENTS_PER_BLOB.to_be_bytes(); // 8 BE bytes
    out[24..32].copy_from_slice(&fe);
    out[32..64].copy_from_slice(&BLS_MODULUS_BE);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bls12_381::{G1Projective, G2Projective, Scalar};
    use ff::Field;
    use rand::rngs::OsRng;

    /// Build a *test-only* trusted setup with a known secret `s`.
    /// Returns (settings, s_g1).
    fn test_setup(s: Scalar) -> (KzgSettings, G1Affine) {
        let s_g2 = (G2Projective::generator() * s).to_affine();
        let s_g1 = (G1Projective::generator() * s).to_affine();
        (KzgSettings { s_g2 }, s_g1)
    }

    /// Commit a degree-1 polynomial p(X) = a + b·X with the test setup.
    fn commit_deg1(a: Scalar, b: Scalar, s_g1: &G1Affine) -> G1Affine {
        // C = a·G1 + b·(s·G1)
        let c = G1Projective::generator() * a + G1Projective::from(*s_g1) * b;
        c.to_affine()
    }

    /// For p(X) = a + b·X and evaluation point z, the quotient
    /// q(X) = (p(X) − p(z)) / (X − z) = b (constant). So proof = b·G1.
    fn proof_deg1(b: Scalar) -> G1Affine {
        (G1Projective::generator() * b).to_affine()
    }

    fn scalar_to_be32(s: &Scalar) -> [u8; 32] {
        let le = s.to_bytes(); // little-endian
        let mut be = [0u8; 32];
        for (i, x) in le.iter().enumerate() {
            be[31 - i] = *x;
        }
        be
    }

    #[test]
    fn versioned_hash_first_byte_is_domain() {
        let c = [0u8; 48];
        let v = kzg_to_versioned_hash(&c);
        assert_eq!(v[0], BLOB_COMMITMENT_VERSION_KZG);
    }

    #[test]
    fn success_return_layout_4096_and_modulus() {
        let r = point_evaluation_success_return();
        // FIELD_ELEMENTS_PER_BLOB right-aligned in first 32-byte word.
        let mut fe = [0u8; 32];
        fe[30] = 0x10; // 4096 = 0x1000
        assert_eq!(&r[0..32], &fe);
        assert_eq!(&r[32..64], &BLS_MODULUS_BE);
    }

    #[test]
    fn verify_passes_for_valid_proof_deg1() {
        let s = Scalar::random(&mut OsRng);
        let (settings, s_g1) = test_setup(s);

        let a = Scalar::from(7u64);
        let b = Scalar::from(3u64);
        let z = Scalar::from(11u64);
        let y = a + b * z; // p(z)

        let c = commit_deg1(a, b, &s_g1);
        let pi = proof_deg1(b);

        let res = verify_kzg_proof(
            &c.to_compressed(),
            &scalar_to_be32(&z),
            &scalar_to_be32(&y),
            &pi.to_compressed(),
            &settings,
        );
        assert!(res.is_ok(), "valid proof must verify, got {:?}", res);
    }

    #[test]
    fn verify_rejects_wrong_y() {
        let s = Scalar::random(&mut OsRng);
        let (settings, s_g1) = test_setup(s);

        let a = Scalar::from(7u64);
        let b = Scalar::from(3u64);
        let z = Scalar::from(11u64);
        let y_bad = a + b * z + Scalar::from(1u64); // off by 1

        let c = commit_deg1(a, b, &s_g1);
        let pi = proof_deg1(b);

        let res = verify_kzg_proof(
            &c.to_compressed(),
            &scalar_to_be32(&z),
            &scalar_to_be32(&y_bad),
            &pi.to_compressed(),
            &settings,
        );
        assert_eq!(res, Err(KzgError::PairingFailed));
    }

    #[test]
    fn verify_rejects_tampered_proof() {
        let s = Scalar::random(&mut OsRng);
        let (settings, s_g1) = test_setup(s);

        let a = Scalar::from(7u64);
        let b = Scalar::from(3u64);
        let z = Scalar::from(11u64);
        let y = a + b * z;

        let c = commit_deg1(a, b, &s_g1);
        // Tamper: use proof for b+1 instead of b.
        let pi_bad = proof_deg1(b + Scalar::from(1u64));

        let res = verify_kzg_proof(
            &c.to_compressed(),
            &scalar_to_be32(&z),
            &scalar_to_be32(&y),
            &pi_bad.to_compressed(),
            &settings,
        );
        assert_eq!(res, Err(KzgError::PairingFailed));
    }

    #[test]
    fn verify_rejects_tampered_commitment() {
        let s = Scalar::random(&mut OsRng);
        let (settings, s_g1) = test_setup(s);

        let a = Scalar::from(7u64);
        let b = Scalar::from(3u64);
        let z = Scalar::from(11u64);
        let y = a + b * z;

        let c_correct = commit_deg1(a, b, &s_g1);
        // Tamper: bump c by +G1
        let c_bad = (G1Projective::from(c_correct) + G1Projective::generator()).to_affine();
        let pi = proof_deg1(b);

        let res = verify_kzg_proof(
            &c_bad.to_compressed(),
            &scalar_to_be32(&z),
            &scalar_to_be32(&y),
            &pi.to_compressed(),
            &settings,
        );
        assert_eq!(res, Err(KzgError::PairingFailed));
    }

    #[test]
    fn bad_g1_compressed_rejected() {
        let s = Scalar::random(&mut OsRng);
        let (settings, _) = test_setup(s);
        let bad_c = [0xFFu8; 48];
        let pi = [0u8; 48]; // identity G1 compressed → all-zero with high bit; not 0xFF
        let res = verify_kzg_proof(
            &bad_c,
            &[0u8; 32],
            &[0u8; 32],
            &pi,
            &settings,
        );
        // Expect either BadCommitment OR BadProof depending on which fails first.
        assert!(matches!(res, Err(KzgError::BadCommitment) | Err(KzgError::BadProof)),
            "got {:?}", res);
    }

    #[test]
    fn non_canonical_scalar_rejected() {
        // Scalar bytes equal to the modulus are NOT canonical (must be < r).
        let s = Scalar::random(&mut OsRng);
        let (settings, _) = test_setup(s);

        // Build any valid commitment / proof so the failure surfaces in the
        // scalar decode step rather than the G1 decode step.
        let c = G1Affine::generator().to_compressed();
        let pi = G1Affine::generator().to_compressed();

        let res = verify_kzg_proof(&c, &BLS_MODULUS_BE, &[0u8; 32], &pi, &settings);
        assert_eq!(res, Err(KzgError::ScalarNotInField));
    }

    #[test]
    fn global_setup_is_settable_once() {
        // Cannot easily test OnceLock outside an isolated process; just
        // exercise the helper paths.
        let s = Scalar::from(42u64);
        let (settings, _) = test_setup(s);
        // The global may already be set by another test in this binary;
        // either branch is acceptable.
        let _ = init_global_kzg_settings(settings);
        let _ = global_kzg_settings();
    }
}
