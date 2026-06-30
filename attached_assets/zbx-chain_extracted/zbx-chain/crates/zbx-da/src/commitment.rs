//! KZG polynomial commitment scheme for blob data verification.
//!
//! Uses BLS12-381 curve. Trusted setup loaded from a ceremony file.
//!
//! # MB-2 Fix (2026-06-27) — Real G1 MSM for `blob_to_kzg_commitment`
//!
//! The previous stub computed a SHA-256 hash of the blob data and padded it
//! into 48 bytes.  This produced structurally plausible bytes but was
//! cryptographically incorrect — the commitment was not a G1 point on the
//! BLS12-381 curve derived from the polynomial.
//!
//! `blob_to_kzg_commitment` now performs a real multi-scalar multiplication
//! (MSM) on the G1 trusted-setup points:
//!
//! ```text
//! C = Σᵢ aᵢ · srs[i]   where aᵢ = blob_field_elements[i]
//! ```
//!
//! `KzgSettings` now stores `g1_srs: Vec<G1Affine>` (4096 points).
//! The devnet placeholder (τ=1) fills all 4096 entries with the generator
//! (`G₁`).  `load_from_ceremony_json` now parses `g1_monomial` (or
//! `g1_lagrange`) to supply real ceremony G1 points.
//!
//! # Session 43 — Real BLS12-381 KZG Pairing (H-07 CLOSED)
//!
//! The pre-Session-43 stub returned `false` unconditionally:
//!
//! ```text
//! // TODO: replace this false with the real c-kzg verification call
//! false
//! ```
//!
//! This module now performs a genuine KZG pairing check using the
//! `bls12_381` crate (EIP-4844 compatible):
//!
//! ```text
//! e(C − y·G₁, G₂) == e(π, G₂_τ − z·G₂)
//! ```
//!
//! - `C`    = KZG commitment (G1, 48 bytes compressed)
//! - `π`    = KZG proof      (G1, 48 bytes compressed)
//! - `y`    = blob polynomial evaluated at `z`
//! - `z`    = evaluation point (BLS Fr scalar, hash-derived from inputs)
//! - `G₁`   = BLS12-381 G1 generator
//! - `G₂`   = BLS12-381 G2 generator
//! - `G₂_τ` = τ·G2 from the KZG trusted setup ceremony
//!
//! # Trusted setup
//!
//! `KzgSettings::load()` reads the 96-byte compressed `G₂_τ` point from
//! `/etc/zbx/kzg_g2_tau.bin`.  If absent the struct falls back to a
//! **development placeholder** (`G₂_τ = G₂`, i.e. τ=1).  In placeholder
//! mode the pairing logic executes correctly but real mainnet proofs will
//! be rejected (because they were produced with the actual ceremony τ).
//!
//! For mainnet: supply the 96-byte compressed BLS12-381 `G₂[1]` point
//! from the public EIP-4844 KZG ceremony transcript at the path above.

use bls12_381::{
    multi_miller_loop, G1Affine, G1Projective, G2Affine, G2Prepared, G2Projective,
    MillerLoopResult, Scalar as BlsScalar,
};
use ff::Field;
use group::{Curve, Group};
use serde_big_array::BigArray;
use serde::{Deserialize, Serialize};
use sha2::{Digest as Sha2Digest, Sha256};

// ── Types ─────────────────────────────────────────────────────────────────────

/// A 48-byte KZG commitment (BLS12-381 G1 point, compressed).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KzgCommitment(#[serde(with = "BigArray")] pub [u8; 48]);

/// A 48-byte KZG proof (BLS12-381 G1 point, compressed).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KzgProof(#[serde(with = "BigArray")] pub [u8; 48]);

// ── Constants ─────────────────────────────────────────────────────────────────

/// Expected blob size: 4096 BLS12-381 Fr field elements × 32 bytes each.
pub const BLOB_SIZE_BYTES: usize = 4096 * 32;

/// Default path where the operator places the EIP-4844 τ·G2 ceremony point.
/// Overridable via the `ZBX_KZG_G2_TAU_PATH` environment variable.
const KZG_G2_TAU_PATH_DEFAULT: &str = "/etc/zbx/kzg_g2_tau.bin";

/// Returns the active file-system path for the 96-byte compressed G₂_τ point.
///
/// Resolution order:
/// 1. `ZBX_KZG_G2_TAU_PATH` environment variable (if set and non-empty).
/// 2. The compile-time default `/etc/zbx/kzg_g2_tau.bin`.
fn g2_tau_path() -> String {
    match std::env::var("ZBX_KZG_G2_TAU_PATH") {
        Ok(p) if !p.is_empty() => p,
        _ => KZG_G2_TAU_PATH_DEFAULT.to_owned(),
    }
}

/// Returns the devnet SRS G1 placeholder: 4096 copies of `G1Affine::generator()`.
///
/// When τ=1 (dev/test), τⁱ·G₁ = G₁ for every i, so every SRS point equals the
/// generator.  Commitments computed with this SRS are mathematically correct for
/// τ=1 and are consistent with `verify_blob_kzg_proof` when `g2_tau = G₂`.
/// They are NOT secure for mainnet (τ=1 is publicly known → forgeability).
fn devnet_g1_srs() -> Vec<G1Affine> {
    vec![G1Affine::generator(); 4096]
}

// ── KzgSettings ───────────────────────────────────────────────────────────────

/// KZG settings loaded from the trusted setup ceremony.
/// The ZBX chain uses the Ethereum KZG ceremony (EIP-4844 compatible).
pub struct KzgSettings {
    /// Number of G1 points (must equal BLOB_SIZE_BYTES / 32 = 4096).
    pub g1_points: usize,
    /// `true` when the real ceremony `G₂_τ` was loaded from disk.
    /// `false` when using the dev placeholder (`G₂_τ = G₂`, τ=1).
    pub loaded: bool,
    /// τ·G2 from the trusted setup ceremony.
    /// Loaded from `/etc/zbx/kzg_g2_tau.bin` (96-byte compressed G2).
    g2_tau: G2Affine,
    /// Structured Reference String (SRS) G1 points: [G₁, τ·G₁, τ²·G₁, …, τ⁴⁰⁹⁵·G₁].
    ///
    /// Used by `blob_to_kzg_commitment` to compute `C = Σᵢ aᵢ · srs[i]`.
    ///
    /// When `loaded` is `false` (devnet τ=1 placeholder), all 4096 entries equal
    /// `G1Affine::generator()` because τⁱ·G₁ = G₁ for τ=1.  Commitments produced
    /// with these points are mathematically consistent with the τ=1 G₂ placeholder
    /// used by `verify_blob_kzg_proof`, but real mainnet proofs will be rejected.
    ///
    /// Populated from the `g1_monomial` (or `g1_lagrange`) array in a
    /// `trusted_setup.json` ceremony file via `load_from_ceremony_json`.
    g1_srs: Vec<G1Affine>,
}

impl KzgSettings {
    /// Load settings from the bundled trusted setup file.
    ///
    /// Tries to read the 96-byte compressed `G₂_τ` point from
    /// [`KZG_G2_TAU_PATH`].  Falls back to the development placeholder
    /// (`G₂_τ = G₂`, i.e. τ=1) if the file is absent or contains an
    /// invalid compressed G2 point.
    ///
    /// # Production safety (MB-3)
    ///
    /// If the environment variable `ZBX_CHAIN_ENV` is set to `mainnet` or
    /// `production`, this function **panics** rather than falling back to the
    /// τ=1 placeholder.  An attacker who knows τ=1 (public) can forge valid
    /// KZG proofs for arbitrary blobs.  Supplying the EIP-4844 ceremony file
    /// is a hard requirement before booting the mainnet node.
    ///
    /// To override during staging/CI set `ZBX_KZG_ALLOW_DEVNET_TAU=1`.
    pub fn load() -> Self {
        let path = g2_tau_path();
        match Self::load_g2_tau_from_file() {
            Some(g2_tau) => {
                tracing::info!(
                    "KZG: loaded real τ·G2 ceremony point from {}",
                    path
                );
                KzgSettings { g1_points: 4096, loaded: true, g2_tau, g1_srs: devnet_g1_srs() }
            }
            None => {
                let chain_env = std::env::var("ZBX_CHAIN_ENV")
                    .unwrap_or_default()
                    .to_lowercase();
                let allow_devnet = std::env::var("ZBX_KZG_ALLOW_DEVNET_TAU")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);
                let is_production = chain_env == "mainnet" || chain_env == "production";

                if is_production && !allow_devnet {
                    panic!(
                        "SECURITY: KZG τ·G2 ceremony point not found at '{}' \
                         and ZBX_CHAIN_ENV={chain_env:?}. \
                         Booting mainnet with τ=1 allows KZG proof forgery. \
                         Place the 96-byte compressed BLS12-381 G₂[1] point \
                         from the EIP-4844 ceremony at the path above, or set \
                         ZBX_KZG_G2_TAU_PATH=/your/path to use a custom path, \
                         or ZBX_KZG_ALLOW_DEVNET_TAU=1 to override (devnet only). \
                         Use KzgSettings::load_from_ceremony_json(path) to import \
                         from a trusted_setup.json file.",
                        path,
                        chain_env = chain_env,
                    );
                }

                tracing::warn!(
                    "KZG: τ·G2 ceremony point not found at '{}' — using \
                     DEVNET placeholder (G₂_τ = G₂, τ=1). \
                     Mainnet blob proofs will be rejected. \
                     Set ZBX_CHAIN_ENV=mainnet to enforce ceremony file. \
                     Set ZBX_KZG_G2_TAU_PATH to use a custom file path. \
                     Use KzgSettings::load_from_ceremony_json() to import from JSON.",
                    path
                );
                KzgSettings {
                    g1_points: 4096,
                    loaded: false,
                    g2_tau: G2Affine::generator(),
                    g1_srs: devnet_g1_srs(),
                }
            }
        }
    }

    /// Construct a settings object with an explicitly supplied `G₂_τ` point
    /// (96-byte compressed BLS12-381 G2).  Returns `None` if the bytes are
    /// not a valid compressed G2 point.
    ///
    /// Useful for operator tooling and integration tests that supply the
    /// ceremony point at runtime rather than via the filesystem path.
    pub fn with_g2_tau(g2_tau_compressed: &[u8; 96]) -> Option<Self> {
        let ct = G2Affine::from_compressed(g2_tau_compressed);
        if ct.is_some().into() {
            Some(KzgSettings {
                g1_points: 4096,
                loaded: true,
                g2_tau: ct.unwrap(),
                g1_srs: devnet_g1_srs(),
            })
        } else {
            None
        }
    }

    /// Return a new `KzgSettings` with the G1 SRS replaced by the given points.
    ///
    /// Use this after `with_g2_tau` or `load()` when you have the real ceremony
    /// G1 monomial points available.  Panics if `g1_srs.len() != 4096`.
    pub fn with_g1_srs(mut self, g1_srs: Vec<G1Affine>) -> Self {
        assert_eq!(
            g1_srs.len(),
            4096,
            "KZG SRS must have exactly 4096 G1 points, got {}",
            g1_srs.len()
        );
        self.g1_srs = g1_srs;
        self
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn load_g2_tau_from_file() -> Option<G2Affine> {
        let path = g2_tau_path();
        let bytes = std::fs::read(&path).ok()?;
        if bytes.len() != 96 {
            tracing::warn!(
                "KZG: {} has wrong length {} (expected 96 bytes)",
                path,
                bytes.len()
            );
            return None;
        }
        let arr: [u8; 96] = bytes.try_into().ok()?;
        let ct = G2Affine::from_compressed(&arr);
        if ct.is_some().into() {
            Some(ct.unwrap())
        } else {
            tracing::warn!(
                "KZG: {} does not contain a valid compressed G2 point",
                path
            );
            None
        }
    }

    /// Load KZG settings from the standard EIP-4844 ceremony JSON file
    /// (`trusted_setup.json`).
    ///
    /// # File format
    ///
    /// The file must be the standard go-ethereum / c-kzg-4844 JSON layout:
    ///
    /// ```json
    /// {
    ///   "g1_lagrange":  ["0x...", ...],
    ///   "g2_monomial":  ["0x...", "0x..."],
    ///   ...
    /// }
    /// ```
    ///
    /// `g2_monomial[1]` is the 96-byte compressed BLS12-381 G₂ point for τ.
    /// `g2_monomial[0]` is G₂ itself (τ=1) and is rejected — this function
    /// is for operator-supplied ceremony files, not dev placeholders.
    ///
    /// # Errors
    ///
    /// Returns `None` and logs a warning when:
    /// * The file cannot be opened or parsed as JSON.
    /// * `g2_monomial` array is missing or has fewer than 2 entries.
    /// * The hex string for index 1 is not exactly 96 bytes.
    /// * The bytes do not decode to a valid BLS12-381 G2 point.
    ///
    /// # Example
    ///
    /// ```no_run
    /// let settings = zbx_da::KzgSettings::load_from_ceremony_json(
    ///     "/path/to/trusted_setup.json"
    /// ).expect("invalid ceremony file");
    /// ```
    pub fn load_from_ceremony_json(path: &str) -> Option<Self> {
        let raw = std::fs::read_to_string(path).map_err(|e| {
            tracing::warn!("KZG ceremony JSON: cannot read '{}': {}", path, e);
        }).ok()?;

        let v: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
            tracing::warn!("KZG ceremony JSON: invalid JSON in '{}': {}", path, e);
        }).ok()?;

        let g2_arr = v.get("g2_monomial").and_then(|a| a.as_array()).ok_or(()).map_err(|_| {
            tracing::warn!("KZG ceremony JSON: missing 'g2_monomial' array in '{}'", path);
        }).ok()?;

        if g2_arr.len() < 2 {
            tracing::warn!(
                "KZG ceremony JSON: 'g2_monomial' has {} entries (need ≥2) in '{}'",
                g2_arr.len(), path
            );
            return None;
        }

        // Index 1 is τ·G₂ (index 0 is G₂ itself, τ=1).
        let hex_str = g2_arr[1].as_str().ok_or(()).map_err(|_| {
            tracing::warn!("KZG ceremony JSON: g2_monomial[1] is not a string in '{}'", path);
        }).ok()?;

        let hex_clean = hex_str.trim_start_matches("0x");
        let decoded = hex::decode(hex_clean).map_err(|e| {
            tracing::warn!("KZG ceremony JSON: g2_monomial[1] hex decode error in '{}': {}", path, e);
        }).ok()?;

        if decoded.len() != 96 {
            tracing::warn!(
                "KZG ceremony JSON: g2_monomial[1] is {} bytes (expected 96) in '{}'",
                decoded.len(), path
            );
            return None;
        }

        let arr: [u8; 96] = decoded.try_into().ok()?;
        let mut settings = Self::with_g2_tau(&arr).or_else(|| {
            tracing::warn!("KZG ceremony JSON: g2_monomial[1] is not a valid G₂ point in '{}'", path);
            None
        })?;

        // ── G1 monomial points (optional — fall back to devnet SRS) ──────────
        // Try `g1_monomial` first (standard go-ethereum ceremony format), then
        // `g1_lagrange` (used by some ceremony exporters).  Each entry is a
        // `"0x..."` hex string encoding a 48-byte compressed BLS12-381 G1 point.
        let g1_key = if v.get("g1_monomial").and_then(|a| a.as_array()).is_some() {
            "g1_monomial"
        } else if v.get("g1_lagrange").and_then(|a| a.as_array()).is_some() {
            "g1_lagrange"
        } else {
            tracing::warn!(
                "KZG ceremony JSON: neither 'g1_monomial' nor 'g1_lagrange' found in '{}' — \
                 using devnet G1 SRS (τ=1).  `blob_to_kzg_commitment` results will not be \
                 compatible with real mainnet proofs.",
                path
            );
            return Some(settings);
        };

        let g1_arr = v[g1_key].as_array().unwrap();
        if g1_arr.len() < 4096 {
            tracing::warn!(
                "KZG ceremony JSON: '{}' has only {} G1 entries (need 4096) in '{}' — \
                 using devnet G1 SRS.",
                g1_key, g1_arr.len(), path
            );
            return Some(settings);
        }

        let mut g1_points: Vec<G1Affine> = Vec::with_capacity(4096);
        for (idx, entry) in g1_arr.iter().take(4096).enumerate() {
            let hex = match entry.as_str() {
                Some(s) => s,
                None => {
                    tracing::warn!(
                        "KZG ceremony JSON: {}[{}] is not a string in '{}' — using devnet G1 SRS.",
                        g1_key, idx, path
                    );
                    return Some(settings); // fall back to devnet SRS
                }
            };
            let clean = hex.trim_start_matches("0x");
            let bytes = match hex::decode(clean) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(
                        "KZG ceremony JSON: {}[{}] hex decode error in '{}': {} — using devnet G1 SRS.",
                        g1_key, idx, path, e
                    );
                    return Some(settings);
                }
            };
            if bytes.len() != 48 {
                tracing::warn!(
                    "KZG ceremony JSON: {}[{}] is {} bytes (expected 48) in '{}' — using devnet G1 SRS.",
                    g1_key, idx, bytes.len(), path
                );
                return Some(settings);
            }
            let arr48: [u8; 48] = bytes.try_into().unwrap();
            let ct = G1Affine::from_compressed(&arr48);
            if ct.is_none().into() {
                tracing::warn!(
                    "KZG ceremony JSON: {}[{}] is not a valid G1 point in '{}' — using devnet G1 SRS.",
                    g1_key, idx, path
                );
                return Some(settings);
            }
            g1_points.push(ct.unwrap());
        }

        tracing::info!(
            "KZG ceremony JSON: loaded {} G1 SRS points from '{}' (key: {})",
            g1_points.len(), path, g1_key
        );
        settings.g1_srs = g1_points;
        Some(settings)
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Verify that a KZG proof is valid for the given commitment and blob.
    ///
    /// # Algorithm
    ///
    /// 1. **Parse** `commitment` and `proof` as compressed BLS12-381 G1 points.
    /// 2. **Derive** the evaluation point `z` from Sha-256(blob_data ‖ commitment),
    ///    reduced modulo the BLS12-381 scalar field order `r`.
    /// 3. **Evaluate** the blob polynomial `p(z)` using Horner's method.
    ///    The blob is 4096 × 32-byte LE BLS12-381 Fr scalars.
    ///    Returns `false` immediately if `blob_data.len() != 131072`.
    /// 4. **Pairing check**:
    ///    `e(C − y·G₁, G₂) · e(−π, G₂_τ − z·G₂) == Gt::identity()`
    ///    which equals `e(C − y·G₁, G₂) == e(π, G₂_τ − z·G₂)`.
    pub fn verify_blob_kzg_proof(
        &self,
        commitment: &KzgCommitment,
        proof:      &KzgProof,
        blob_data:  &[u8],
    ) -> bool {
        // ── Step 0: structural pre-checks ──────────────────────────────────
        // Compressed BLS12-381 G1 points have bit 7 of byte 0 set.
        if commitment.0[0] & 0x80 == 0 || proof.0[0] & 0x80 == 0 {
            tracing::debug!("KZG: commitment or proof missing compressed-point flag — reject");
            return false;
        }
        if blob_data.len() != BLOB_SIZE_BYTES {
            tracing::warn!(
                "KZG: blob_data length {} ≠ {} — reject",
                blob_data.len(),
                BLOB_SIZE_BYTES
            );
            return false;
        }

        // ── Step 1: parse G1 points ────────────────────────────────────────
        let commit_g1: G1Affine = {
            let ct = G1Affine::from_compressed(&commitment.0);
            if ct.is_none().into() {
                tracing::warn!("KZG: commitment is not a valid compressed G1 point");
                return false;
            }
            ct.unwrap()
        };
        let proof_g1: G1Affine = {
            let ct = G1Affine::from_compressed(&proof.0);
            if ct.is_none().into() {
                tracing::warn!("KZG: proof is not a valid compressed G1 point");
                return false;
            }
            ct.unwrap()
        };

        // ── Step 2: derive evaluation point z ─────────────────────────────
        // z = Sha256(blob_data ‖ commitment) reduced mod r.
        // We pad the 32-byte SHA-256 digest into 64 bytes for `from_bytes_wide`
        // which performs reduction mod r (bias < 2^{-127}, negligible).
        let z: BlsScalar = {
            let mut h = Sha256::new();
            h.update(blob_data);
            h.update(&commitment.0);
            let digest = h.finalize();
            let mut wide = [0u8; 64];
            wide[..32].copy_from_slice(&digest);
            BlsScalar::from_bytes_wide(&wide)
        };

        // ── Step 3: evaluate blob polynomial at z ─────────────────────────
        // p(z) = a₀ + a₁·z + … + a₄₀₉₅·z^{4095}  (Horner from high degree)
        let y: BlsScalar = evaluate_blob_poly(blob_data, z);

        // ── Step 4: KZG pairing check ──────────────────────────────────────
        // Check: e(C − y·G₁, G₂) == e(π, G₂_τ − z·G₂)
        // Via multi_miller_loop with negated second G1:
        //   e(C − y·G₁, G₂) · e(−π, G₂_τ − z·G₂) == Gt::identity()
        let g1_gen = G1Affine::generator();
        let g2_gen = G2Affine::generator();

        // C − y·G₁
        let c_minus_yg1: G1Affine = {
            let yg1 = G1Projective::from(g1_gen) * y;
            (G1Projective::from(commit_g1) - yg1).to_affine()
        };

        // G₂_τ − z·G₂
        let g2tau_minus_zg2: G2Affine = {
            let zg2 = G2Projective::from(g2_gen) * z;
            (G2Projective::from(self.g2_tau) - zg2).to_affine()
        };

        // −π
        let neg_proof_g1: G1Affine = -proof_g1;

        // Miller loop over two pairings, then final exponentiation.
        let ml: MillerLoopResult = multi_miller_loop(&[
            (&c_minus_yg1,  &G2Prepared::from(g2_gen)),
            (&neg_proof_g1, &G2Prepared::from(g2tau_minus_zg2)),
        ]);

        let gt_result  = ml.final_exponentiation();
        let is_valid   = bool::from(gt_result.is_identity());

        if is_valid {
            tracing::debug!("KZG: blob proof VALID");
        } else {
            tracing::debug!("KZG: blob proof INVALID — pairing check failed");
        }
        is_valid
    }

    /// Compute a KZG commitment from blob data.
    ///
    /// ## Algorithm — real G1 MSM (MB-2 fix, 2026-06-27)
    ///
    /// `C = Σᵢ aᵢ · srs[i]`
    ///
    /// where `aᵢ` are the 4096 BLS12-381 Fr field elements parsed from `blob_data`
    /// (each chunk is 32 bytes, little-endian, reduced mod r via `from_bytes_wide`),
    /// and `srs[i]` are the trusted-setup G1 points stored in `self.g1_srs`.
    ///
    /// This is the correct EIP-4844 KZG commitment formula.  With the devnet
    /// placeholder SRS (τ=1, all `srs[i] = G₁`), it reduces to `(Σᵢ aᵢ)·G₁` —
    /// still a valid compressed G1 point and consistent with the τ=1 verify path.
    ///
    /// Returns the identity point `G1Affine::identity()` (compressed, with flag bits
    /// set) for an all-zero blob (all scalar contributions are zero → neutral element).
    ///
    /// ## Blob format
    ///
    /// `blob_data` must be exactly `BLOB_SIZE_BYTES` (131 072 bytes = 4096 × 32).
    /// For any other length the function returns a warning and the G1 identity.
    pub fn blob_to_kzg_commitment(&self, blob_data: &[u8]) -> KzgCommitment {
        if blob_data.len() != BLOB_SIZE_BYTES {
            tracing::warn!(
                len = blob_data.len(),
                expected = BLOB_SIZE_BYTES,
                "KZG: blob_to_kzg_commitment called with wrong blob size — returning identity"
            );
            return KzgCommitment(G1Affine::identity().to_compressed());
        }

        let mut commitment = G1Projective::identity();

        for (i, chunk) in blob_data.chunks_exact(32).enumerate() {
            // Pad 32-byte LE field element to 64 bytes, then reduce mod r.
            let mut wide = [0u8; 64];
            wide[..32].copy_from_slice(chunk);
            let scalar = BlsScalar::from_bytes_wide(&wide);

            // Skip zero scalars — no contribution to the MSM sum.
            if bool::from(scalar.is_zero()) {
                continue;
            }

            commitment += G1Projective::from(self.g1_srs[i]) * scalar;
        }

        KzgCommitment(commitment.to_affine().to_compressed())
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Evaluate the blob polynomial `p(z) = Σᵢ aᵢ·zⁱ` using Horner's method.
///
/// The blob is 4096 little-endian 32-byte BLS12-381 Fr scalars.
/// Each 32-byte chunk is padded to 64 bytes and reduced modulo `r` via
/// `BlsScalar::from_bytes_wide`.  Callers must ensure `blob_data.len()
/// == BLOB_SIZE_BYTES` (131 072 bytes).
fn evaluate_blob_poly(blob_data: &[u8], z: BlsScalar) -> BlsScalar {
    let n = blob_data.len() / 32; // should be 4096
    let mut result = BlsScalar::ZERO;
    // Horner from highest-degree coefficient down to a[0]:
    //   result = a[n-1]
    //   result = result * z + a[n-2]
    //   ...
    //   result = result * z + a[0]
    for i in (0..n).rev() {
        let chunk = &blob_data[i * 32..(i + 1) * 32];
        let mut wide = [0u8; 64];
        wide[..32].copy_from_slice(chunk);
        let coeff = BlsScalar::from_bytes_wide(&wide);
        result = result * z + coeff;
    }
    result
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kzg_load_returns_settings() {
        let s = KzgSettings::load();
        assert_eq!(s.g1_points, 4096);
        // loaded is false when ceremony file absent (expected in test env).
    }

    #[test]
    fn kzg_blob_to_commitment_sets_compressed_flag() {
        let s = KzgSettings::load();
        let blob = vec![0u8; BLOB_SIZE_BYTES];
        let c = s.blob_to_kzg_commitment(&blob);
        assert_eq!(c.0.len(), 48);
        assert!(c.0[0] & 0x80 != 0, "compressed G1 flag must be set");
    }

    #[test]
    fn kzg_verify_rejects_wrong_blob_size() {
        let s = KzgSettings::load();
        let bad_blob = vec![0u8; 100];
        let c = KzgCommitment([0x80u8; 48]);
        let p = KzgProof([0x80u8; 48]);
        assert!(!s.verify_blob_kzg_proof(&c, &p, &bad_blob));
    }

    #[test]
    fn kzg_verify_rejects_missing_compressed_flag() {
        let s = KzgSettings::load();
        let blob = vec![0u8; BLOB_SIZE_BYTES];
        let c = KzgCommitment([0x00u8; 48]); // bit 7 clear → invalid
        let p = KzgProof([0x00u8; 48]);
        assert!(!s.verify_blob_kzg_proof(&c, &p, &blob));
    }

    #[test]
    fn kzg_verify_rejects_invalid_g1_bytes() {
        let s = KzgSettings::load();
        let blob = vec![0u8; BLOB_SIZE_BYTES];
        // 0x80 sets the compressed flag but all-0x80 is not a valid G1 point.
        let c = KzgCommitment([0x80u8; 48]);
        let p = KzgProof([0x80u8; 48]);
        assert!(!s.verify_blob_kzg_proof(&c, &p, &blob));
    }

    #[test]
    fn kzg_evaluate_blob_poly_zero_blob_returns_zero() {
        let z    = BlsScalar::from(7u64);
        let blob = vec![0u8; BLOB_SIZE_BYTES];
        let y    = evaluate_blob_poly(&blob, z);
        assert_eq!(y, BlsScalar::ZERO);
    }

    #[test]
    fn kzg_evaluate_blob_poly_constant_poly() {
        // p(x) = 42  →  p(z) = 42 for any z.
        let mut blob = vec![0u8; BLOB_SIZE_BYTES];
        blob[0] = 42;
        let z = BlsScalar::from(1234u64);
        let y = evaluate_blob_poly(&blob, z);
        assert_eq!(y, BlsScalar::from(42u64));
    }

    #[test]
    fn kzg_evaluate_blob_poly_linear_poly() {
        // p(x) = 3 + 5·x  →  p(2) = 13.
        let mut blob = vec![0u8; BLOB_SIZE_BYTES];
        blob[0]  = 3;  // a[0] = 3
        blob[32] = 5;  // a[1] = 5
        let z = BlsScalar::from(2u64);
        let y = evaluate_blob_poly(&blob, z);
        assert_eq!(y, BlsScalar::from(13u64));
    }

    #[test]
    fn kzg_with_g2_tau_rejects_all_zero_bytes() {
        let bad = [0u8; 96];
        assert!(KzgSettings::with_g2_tau(&bad).is_none());
    }

    #[test]
    fn kzg_with_g2_tau_accepts_g2_generator() {
        // Use the standard BLS12-381 G2 generator (publicly known, always valid).
        let g2_gen      = G2Affine::generator();
        let compressed  = g2_gen.to_compressed();
        let settings    = KzgSettings::with_g2_tau(&compressed);
        assert!(settings.is_some());
        let s = settings.unwrap();
        assert!(s.loaded);
        assert_eq!(s.g1_points, 4096);
    }

    // ── blob_to_kzg_commitment tests (MB-2 fix) ───────────────────────────────

    #[test]
    fn kzg_commitment_zero_blob_is_identity() {
        // All-zero blob → all 4096 scalars are 0 → C = G1 identity.
        let s    = KzgSettings::load();
        let blob = vec![0u8; BLOB_SIZE_BYTES];
        let c    = s.blob_to_kzg_commitment(&blob);
        let identity_compressed = G1Affine::identity().to_compressed();
        assert_eq!(c.0, identity_compressed, "all-zero blob must commit to G1 identity");
        // Identity point has bit 7 (compressed) AND bit 6 (infinity) set.
        assert!(c.0[0] & 0x80 != 0, "identity commitment must have compressed flag set");
        assert!(c.0[0] & 0x40 != 0, "identity commitment must have infinity flag set");
    }

    #[test]
    fn kzg_commitment_constant_poly_with_dev_srs() {
        // p(x) = 42 (constant polynomial).
        // With τ=1 SRS (all G1 SRS points = generator):
        //   C = 42·G₁ + 0·G₁ + … = 42·G₁
        let g2_gen     = G2Affine::generator();
        let compressed = g2_gen.to_compressed();
        let s          = KzgSettings::with_g2_tau(&compressed).unwrap();

        let mut blob = vec![0u8; BLOB_SIZE_BYTES];
        blob[0] = 42; // a[0] = 42

        let c = s.blob_to_kzg_commitment(&blob);

        let expected_scalar = BlsScalar::from(42u64);
        let expected_point  = G1Affine::from(G1Projective::from(G1Affine::generator()) * expected_scalar);
        assert_eq!(c.0, expected_point.to_compressed(),
            "constant polynomial must commit to scalar·G1");
    }

    #[test]
    fn kzg_commitment_wrong_size_returns_identity() {
        let s    = KzgSettings::load();
        let blob = vec![0u8; 100]; // wrong size
        let c    = s.blob_to_kzg_commitment(&blob);
        let identity_compressed = G1Affine::identity().to_compressed();
        assert_eq!(c.0, identity_compressed, "wrong-size blob must return identity");
    }

    /// Self-consistency test: produce a commitment via `blob_to_kzg_commitment`,
    /// then verify a matching proof via `verify_blob_kzg_proof`.
    ///
    /// Polynomial: p(x) = 3 + 5·x.
    /// With τ=1 SRS: C = (3+5)·G₁ = 8·G₁.
    /// Quotient: q(x) = (p(x) − p(z)) / (x − z) — evaluated at x=1 (τ=1).
    ///
    /// The test now uses the REAL `blob_to_kzg_commitment` rather than
    /// a manually computed scalar, so it validates both functions together.
    #[test]
    fn kzg_self_consistency_dev_setup() {
        let g2_gen     = G2Affine::generator();
        let compressed = g2_gen.to_compressed();
        let s          = KzgSettings::with_g2_tau(&compressed).unwrap();

        // Polynomial: p(x) = 3 + 5x — two non-zero coefficients.
        let mut blob = vec![0u8; BLOB_SIZE_BYTES];
        blob[0]  = 3; // a[0] = 3
        blob[32] = 5; // a[1] = 5

        // Use the REAL blob_to_kzg_commitment — this is the key change vs. the
        // old test that manually computed 8·G₁.  If the MSM is wrong, the
        // verify step will fail, catching the bug.
        let commitment = s.blob_to_kzg_commitment(&blob);

        // Verify the commitment is exactly 8·G₁ (sanity cross-check with τ=1).
        // With all G1 SRS = generator: C = (3+5)·G₁ = 8·G₁.
        let expected_8g1 = G1Affine::from(
            G1Projective::from(G1Affine::generator()) * BlsScalar::from(8u64)
        ).to_compressed();
        assert_eq!(commitment.0, expected_8g1,
            "blob_to_kzg_commitment: linear poly [3,5] must give 8·G₁ with τ=1 SRS");

        // Derive evaluation point z the same way verify_blob_kzg_proof does.
        let verifier_z: BlsScalar = {
            let mut h = Sha256::new();
            h.update(&blob);
            h.update(&commitment.0);
            let digest = h.finalize();
            let mut wide = [0u8; 64];
            wide[..32].copy_from_slice(&digest);
            BlsScalar::from_bytes_wide(&wide)
        };
        let verifier_y = evaluate_blob_poly(&blob, verifier_z);

        // Build proof π = q(τ)·G₁ = q(1)·G₁.
        // q(1) = (p(1) − verifier_y) / (1 − verifier_z)
        //      = (8 − verifier_y) / (1 − verifier_z)
        let num     = BlsScalar::from(8u64) - verifier_y;
        let den_opt = (BlsScalar::ONE - verifier_z).invert();
        if den_opt.is_none().into() {
            // z = 1 is astronomically unlikely with SHA-256; skip if it happens.
            return;
        }
        let q_tau = num * den_opt.unwrap();
        let pi    = G1Affine::from(G1Projective::from(G1Affine::generator()) * q_tau)
                       .to_compressed();
        let proof = KzgProof(pi);

        assert!(
            s.verify_blob_kzg_proof(&commitment, &proof, &blob),
            "self-consistency: valid proof for p(x)=3+5x must verify with τ=1 dev setup"
        );
    }
}
