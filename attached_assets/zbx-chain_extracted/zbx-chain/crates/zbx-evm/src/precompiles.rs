//! EVM precompiled contracts (addresses 0x01–0x09).
//!
//! All five previously-stubbed precompiles now have real implementations:
//! * 0x03 — RIPEMD-160        (via `ripemd` crate)
//! * 0x05 — MODEXP            (via `num-bigint`, EIP-198 / EIP-2565)
//! * 0x06 — BN128_ADD         (via `substrate-bn`, EIP-196)
//! * 0x07 — BN128_MUL         (via `substrate-bn`, EIP-196)
//! * 0x08 — BN128_PAIRING     (via `substrate-bn`, EIP-197)
//! * 0x09 — BLAKE2F            (inline BLAKE2b-F compression, EIP-152)

use crate::error::EvmError;
use zbx_types::address::Address;
use sha2::{Digest, Sha256};

pub use zbx_types::payid::PayIdLookup;

pub fn is_precompile(addr: &Address) -> bool {
    let b = addr.as_bytes();
    // Task #3/#4 (Precompiles 0x0A/0x0B): range now extends to 0x0B.
    //   * 0x0A — stateful PayID; intercepted by interpreter (do_payid).
    //   * 0x0B — stateful KZG point evaluation; reads trusted setup
    //            from the process-wide OnceLock; dispatcher path
    //            (call_precompile) routes through global lookup.
    // Task #5 extends the range to 0x0C (stateful price oracle, intercepted by interpreter).
    // Task #6 extends the range to 0x0E (0x0D = stateless ed25519_verify, mirrors
    // ZVM precompile of the same id; 0x0E = stateless VRF verify, RFC 9381).
    // Task #7 extends the range to 0x0F (stateful ZUSD vault state direct-read,
    // intercepted by interpreter via `do_zusd_vault`).
    b[..19].iter().all(|&x| x == 0) && b[19] >= 1 && b[19] <= 0x0F
}

pub fn call_precompile(
    addr: &Address,
    input: &[u8],
    gas: u64,
) -> Result<(Vec<u8>, u64), EvmError> {
    let id = addr.as_bytes()[19];
    match id {
        1 => ecrecover(input, gas),
        2 => sha256_hash(input, gas),
        3 => ripemd160_hash(input, gas),
        4 => identity(input, gas),
        5 => modexp(input, gas),
        6 => bn128_add(input, gas),
        7 => bn128_mul(input, gas),
        8 => bn128_pairing(input, gas),
        9 => blake2f(input, gas),
        // Task #3: 0x0A is stateful — see `do_payid`. Dispatcher path
        // is unreachable in production; fail-closed for safety.
        0x0A => Err(EvmError::Precompile(
            "payid (0x0A) requires host context; call via interpreter".into(),
        )),
        // Task #4: 0x0B (EIP-4844 KZG point evaluation). Trusted setup is
        // process-global (loaded by node startup); shared verifier in
        // `zbx-crypto::kzg` keeps EVM and ZVM byte-identical.
        0x0B => do_kzg_global(input, gas),
        // Task #5: 0x0C is stateful — see `do_price_oracle`. Dispatcher
        // path is unreachable in production; fail-closed for safety.
        0x0C => Err(EvmError::Precompile(
            "price_oracle (0x0C) requires host context; call via interpreter".into(),
        )),
        // Task #6: 0x0D Ed25519 verify — stateless, mirrors ZVM body.
        0x0D => ed25519_verify(input, gas),
        // Task #6: 0x0E VRF verify (RFC 9381 ECVRF-EDWARDS25519-SHA512-ELL2)
        // — stateless, mirrors ZVM body. Both engines call the same
        // `zbx_crypto::vrf::ecvrf_edwards25519::verify` so EVM and ZVM
        // outputs are byte-identical.
        0x0E => vrf_verify(input, gas),
        // Task #7: 0x0F (ZUSD vault state direct-read) is stateful — see
        // `do_zusd_vault`. Dispatcher path is unreachable in production;
        // fail-closed for safety (matches Task #5 0x0C convention).
        0x0F => Err(EvmError::Precompile(
            "zusd_vault (0x0F) requires host context; call via interpreter".into(),
        )),
        _ => Err(EvmError::Precompile(format!("unknown precompile 0x{:02x}", id))),
    }
}

/// Task #7 (Precompile 0x0F — ZUSD vault state direct-read).
///
/// Thin EVM-side wrapper over the shared body in
/// `zbx_crypto::vault_state::do_zusd_vault_read`. Maps shared errors to
/// `EvmError`. Both EVM and ZVM call into the same body so the two
/// engines are byte-identical for this precompile.
pub fn do_zusd_vault<R: zbx_crypto::vault_state::VaultStateReader + ?Sized>(
    input: &[u8],
    gas: u64,
    reader: &R,
) -> Result<(Vec<u8>, u64), EvmError> {
    use zbx_crypto::vault_state::{do_zusd_vault_read, VaultPrecompileError};
    do_zusd_vault_read(input, gas, reader).map_err(|e| match e {
        VaultPrecompileError::OutOfGas => EvmError::OutOfGas,
        VaultPrecompileError::BadInputLength { got } => {
            EvmError::Precompile(format!("zusd_vault: input must be 32 bytes, got {got}"))
        }
    })
}

// ---------------------------------------------------------------------------
// Task #6 (Precompiles 0x0D / 0x0E — stateless signature/VRF verification).
// ---------------------------------------------------------------------------

/// 0x0D — Ed25519 signature verification (mirrors `zbx_zvm::precompiles::ed25519_verify`).
///
/// Layout: 128 bytes — pubkey:32 ‖ msg:32 ‖ sig:64.
/// Gas: flat 3000.
/// Output: 32-byte big-endian boolean (0/1), fail-soft on malformed input.
fn ed25519_verify(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), EvmError> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let cost = 3000u64;
    if gas < cost {
        return Err(EvmError::OutOfGas);
    }
    if input.len() < 128 {
        return Ok((vec![0u8; 32], cost));
    }
    let pk_bytes: [u8; 32] = input[0..32].try_into().unwrap();
    let msg = &input[32..64];
    let sig_bytes: [u8; 64] = input[64..128].try_into().unwrap();

    let vk = match VerifyingKey::from_bytes(&pk_bytes) {
        Ok(v) => v,
        Err(_) => return Ok((vec![0u8; 32], cost)),
    };
    let sig = Signature::from_bytes(&sig_bytes);
    let mut out = [0u8; 32];
    if vk.verify(msg, &sig).is_ok() {
        out[31] = 1;
    }
    Ok((out.to_vec(), cost))
}

/// 0x0E — VRF verification (RFC 9381 ECVRF-EDWARDS25519-SHA512-ELL2).
///
/// Layout: variable, minimum 112 bytes — pubkey:32 ‖ alpha:N ‖ pi:80.
/// Gas:    flat 5000.
/// Output: 64-byte beta on valid, 32-byte zero on invalid (fail-soft).
///
/// Status: the underlying verifier is currently fail-closed (always
/// returns None pending a cross-verified impl), so this precompile
/// always emits the 32-byte zero output. The interface, gas cost, and
/// output convention are stable; callers that branch on
/// `ret.length != 64` will keep working when the verifier lands.
/// Byte-identical to `zbx_zvm::precompiles::vrf_verify`.
fn vrf_verify(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), EvmError> {
    use zbx_crypto::vrf::ecvrf_edwards25519::{verify, BETA_LEN, PROOF_LEN, PUBKEY_LEN};
    let cost = 5000u64;
    if gas < cost {
        return Err(EvmError::OutOfGas);
    }
    let min_len = PUBKEY_LEN + PROOF_LEN; // 112
    if input.len() < min_len {
        return Ok((vec![0u8; 32], cost));
    }
    let mut pk = [0u8; 32];
    pk.copy_from_slice(&input[0..PUBKEY_LEN]);
    // M-4 fix: parse proof from a fixed offset rather than from the END of input.
    // Parsing from the end is fragile: ABI encoders may append padding bytes after
    // the proof, which would corrupt `pi`. Enforce exact layout instead.
    let pi_start = PUBKEY_LEN + (input.len() - PUBKEY_LEN - PROOF_LEN);
    if pi_start < PUBKEY_LEN || input.len() < PUBKEY_LEN + PROOF_LEN {
        return Ok((vec![0u8; 32], cost));
    }
    let alpha = &input[PUBKEY_LEN..pi_start];
    let pi = &input[pi_start..pi_start + PROOF_LEN]; // exact proof bytes, no trailing garbage

    match verify(&pk, alpha, pi) {
        Some(beta) => {
            debug_assert_eq!(beta.len(), BETA_LEN);
            Ok((beta.to_vec(), cost))
        }
        None => Ok((vec![0u8; 32], cost)),
    }
}

// ---------------------------------------------------------------------------
// Task #4 (Precompile 0x0B — EIP-4844 KZG point evaluation): stateful body.
// ---------------------------------------------------------------------------
//
// Single source of truth lives in `zbx_crypto::kzg::do_kzg_point_eval` so
// EVM and ZVM produce byte-identical outputs and gas usage. The trusted
// setup is process-global (`OnceLock<KzgSettings>`), installed once at
// node startup. If no setup is installed, fail-closed — a node booting
// without trusted setup must NOT silently accept blob-aware contracts.
fn do_kzg_global(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), EvmError> {
    let settings = zbx_crypto::kzg::global_kzg_settings().ok_or_else(|| {
        EvmError::Precompile(
            "kzg (0x0B): trusted setup not initialised; call \
             zbx_crypto::kzg::init_global_kzg_settings at node startup"
                .into(),
        )
    })?;
    do_kzg_with_settings(input, gas, &settings)
}

/// Test/integration entry point — bypasses the `OnceLock` so each test
/// can supply its own setup. Production callers go through
/// `do_kzg_global` which reads the global. Both paths share the
/// `do_kzg_point_eval` implementation in `zbx-crypto`.
pub fn do_kzg_with_settings(
    input: &[u8],
    gas: u64,
    settings: &zbx_crypto::kzg::KzgSettings,
) -> Result<(Vec<u8>, u64), EvmError> {
    use zbx_crypto::kzg::{do_kzg_point_eval, KzgError};
    match do_kzg_point_eval(input, gas, settings) {
        Ok(v) => Ok(v),
        Err(KzgError::OutOfGas { .. }) => Err(EvmError::OutOfGas),
        Err(e) => Err(EvmError::Precompile(format!("kzg: {}", e))),
    }
}

// ---------------------------------------------------------------------------
// Task #5 (Precompile 0x0C — Price oracle read): stateful body.
// ---------------------------------------------------------------------------
//
// Byte-identical to `zbx_zvm::precompiles::price_oracle_with`. Both
// engines delegate to `zbx_crypto::oracle_state::do_price_oracle` so
// the (output, gas) pair is consensus-equivalent between EVM and ZVM.
pub fn do_price_oracle<R: zbx_crypto::oracle_state::OracleStateReader + ?Sized>(
    input: &[u8],
    gas: u64,
    reader: &R,
) -> Result<(Vec<u8>, u64), EvmError> {
    use zbx_crypto::oracle_state::{do_price_oracle as inner, OraclePrecompileError};
    match inner(input, gas, reader) {
        Ok(v) => Ok(v),
        Err(OraclePrecompileError::OutOfGas) => Err(EvmError::OutOfGas),
        Err(e) => Err(EvmError::Precompile(format!("price_oracle: {}", e))),
    }
}

// ---------------------------------------------------------------------------
// Task #3 (Precompile 0x0A — PayID resolution): stateful body.
// ---------------------------------------------------------------------------
//
// Byte-identical to `zbx_zvm::precompiles::payid_resolve_with` so the two
// engines cannot drift on consensus-critical lookup semantics. See that
// function's doc-comment for the full ABI / gas / error contract.
pub fn do_payid<L: PayIdLookup + ?Sized>(
    input: &[u8],
    gas: u64,
    lookup: &L,
) -> Result<(Vec<u8>, u64), EvmError> {
    if input.len() < 96 {
        return Err(EvmError::Precompile("payid: input < 96 bytes".into()));
    }
    if input[..31].iter().any(|&b| b != 0) {
        return Err(EvmError::Precompile("payid: op must fit in uint8".into()));
    }
    let op = input[31];

    if input[64..92].iter().any(|&b| b != 0) {
        return Err(EvmError::Precompile("payid: length too large".into()));
    }
    let length = u32::from_be_bytes([input[92], input[93], input[94], input[95]]) as usize;
    if length == 0 || input.len() < 96 + length {
        return Err(EvmError::Precompile("payid: payload length out of range".into()));
    }
    let payload = &input[96..96 + length];

    let cost = 700u64.saturating_add(50u64.saturating_mul(payload.len() as u64));
    if gas < cost {
        return Err(EvmError::OutOfGas);
    }

    match op {
        0 => {
            let raw = std::str::from_utf8(payload)
                .map_err(|_| EvmError::Precompile("payid: name not UTF-8".into()))?;
            let lowered = raw.to_ascii_lowercase();
            let name = lowered.trim_end_matches("@zbx");
            let name_bytes = name.as_bytes();
            if !zbx_types::payid::validate_payid_name(name_bytes) {
                return Err(EvmError::Precompile(
                    "payid: name violates [a-z0-9._-]{3,32}".into(),
                ));
            }
            let mut out = vec![0u8; 32];
            if let Some(addr) = lookup.resolve(name_bytes) {
                out[12..32].copy_from_slice(&addr);
            }
            Ok((out, cost))
        }
        1 => {
            if payload.len() != 20 {
                return Err(EvmError::Precompile(
                    "payid: reverse-lookup payload must be exactly 20 bytes".into(),
                ));
            }
            let mut addr = [0u8; 20];
            addr.copy_from_slice(payload);
            let name = lookup.reverse(&addr).unwrap_or_default();
            let len = name.len();
            let padded = (len + 31) / 32 * 32;
            let mut out = vec![0u8; 64 + padded];
            out[31] = 0x20;
            out[60..64].copy_from_slice(&(len as u32).to_be_bytes());
            if len > 0 {
                out[64..64 + len].copy_from_slice(&name);
            }
            Ok((out, cost))
        }
        _ => Err(EvmError::Precompile("payid: unknown op (expected 0 or 1)".into())),
    }
}

// ---------------------------------------------------------------------------
// 0x01 — ECRECOVER
// ---------------------------------------------------------------------------

fn ecrecover(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), EvmError> {
    let cost = 3_000;
    if gas < cost { return Err(EvmError::OutOfGas); }
    if input.len() < 128 { return Ok((vec![0u8; 32], cost)); }
    let hash = zbx_types::H256::from_slice(&input[..32]);
    let v = input[63];
    let r = zbx_types::H256::from_slice(&input[64..96]);
    let s = zbx_types::H256::from_slice(&input[96..128]);
    let sig = zbx_crypto::Signature { v: if v > 1 { v - 27 } else { v }, r, s };
    match zbx_crypto::secp256k1::recover_signer(&hash, &sig) {
        Ok(addr) => {
            let mut out = [0u8; 32];
            out[12..].copy_from_slice(addr.as_bytes());
            Ok((out.to_vec(), cost))
        }
        Err(_) => Ok((vec![0u8; 32], cost)),
    }
}

// ---------------------------------------------------------------------------
// 0x02 — SHA-256
// ---------------------------------------------------------------------------

fn sha256_hash(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), EvmError> {
    let cost = 60 + 12 * ((input.len() as u64 + 31) / 32);
    if gas < cost { return Err(EvmError::OutOfGas); }
    Ok((Sha256::digest(input).to_vec(), cost))
}

// ---------------------------------------------------------------------------
// 0x03 — RIPEMD-160  (real implementation — previous stub used keccak256)
// ---------------------------------------------------------------------------

fn ripemd160_hash(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), EvmError> {
    use ripemd::{Ripemd160, Digest as _};
    let cost = 600 + 120 * ((input.len() as u64 + 31) / 32);
    if gas < cost { return Err(EvmError::OutOfGas); }
    let digest = Ripemd160::digest(input);
    // EVM pads the 20-byte hash to 32 bytes (left-padded with zeros).
    let mut out = [0u8; 32];
    out[12..].copy_from_slice(&digest);
    Ok((out.to_vec(), cost))
}

// ---------------------------------------------------------------------------
// 0x04 — IDENTITY
// ---------------------------------------------------------------------------

fn identity(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), EvmError> {
    let cost = 15 + 3 * ((input.len() as u64 + 31) / 32);
    if gas < cost { return Err(EvmError::OutOfGas); }
    Ok((input.to_vec(), cost))
}

// ---------------------------------------------------------------------------
// 0x05 — MODEXP  (EIP-198 + EIP-2565 gas schedule)
// ---------------------------------------------------------------------------

fn modexp(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), EvmError> {
    use num_bigint::BigUint;
    use num_traits::Zero;

    // Need at least 3 × 32 bytes for the length fields.
    let padded = pad_right(input, 96.max(input.len()));

    let base_len = read_u256_as_usize(&padded[0..32])
        .map_err(|_| EvmError::Precompile("modexp: base_len overflow".into()))?;
    let exp_len = read_u256_as_usize(&padded[32..64])
        .map_err(|_| EvmError::Precompile("modexp: exp_len overflow".into()))?;
    let mod_len = read_u256_as_usize(&padded[64..96])
        .map_err(|_| EvmError::Precompile("modexp: mod_len overflow".into()))?;

    if mod_len == 0 {
        let cost = 200u64;
        if gas < cost { return Err(EvmError::OutOfGas); }
        return Ok((vec![], cost));
    }

    // Pad to cover base + exp + mod bytes.
    let total = 96usize
        .checked_add(base_len).and_then(|l| l.checked_add(exp_len))
        .and_then(|l| l.checked_add(mod_len))
        .ok_or_else(|| EvmError::Precompile("modexp: length overflow".into()))?;
    let padded = pad_right(&padded, total);

    let base_start = 96;
    let exp_start  = base_start + base_len;
    let mod_start  = exp_start  + exp_len;

    let base    = BigUint::from_bytes_be(&padded[base_start..base_start + base_len]);
    let exp     = BigUint::from_bytes_be(&padded[exp_start..exp_start + exp_len]);
    let modulus = BigUint::from_bytes_be(&padded[mod_start..mod_start + mod_len]);

    // EIP-2565 gas cost.
    let cost = modexp_gas_eip2565(base_len, exp_len, mod_len, &exp);
    if gas < cost { return Err(EvmError::OutOfGas); }

    if modulus.is_zero() {
        return Ok((vec![0u8; mod_len], cost));
    }

    let result = base.modpow(&exp, &modulus);
    // Left-pad result to mod_len bytes.
    let mut result_bytes = result.to_bytes_be();
    while result_bytes.len() < mod_len {
        result_bytes.insert(0, 0u8);
    }
    result_bytes.truncate(mod_len);
    Ok((result_bytes, cost))
}

fn modexp_gas_eip2565(base_len: usize, exp_len: usize, mod_len: usize, exp: &num_bigint::BigUint) -> u64 {
    let max_len = base_len.max(mod_len) as u64;
    let words = (max_len + 7) / 8;
    let multiplication_complexity = words * words;

    // Iteration count based on exponent bit length.
    let iteration_count: u64 = if exp_len <= 32 {
        let exp_bits = exp.bits() as u64;
        if exp_bits == 0 { 0 } else { exp_bits.saturating_sub(1) }
    } else {
        let extra_bits = (8 * (exp_len as u64 - 32)).saturating_sub(1);
        let top_word_bits = {
            // Take top 32 bytes of exponent.
            let eb = exp.to_bytes_be();
            if eb.len() > 32 {
                let top32 = &eb[..32];
                let v = num_bigint::BigUint::from_bytes_be(top32);
                v.bits() as u64
            } else {
                exp.bits() as u64
            }
        };
        extra_bits.saturating_add(top_word_bits.saturating_sub(1))
    };

    ((multiplication_complexity * iteration_count.max(1)) / 3).max(200)
}

// ---------------------------------------------------------------------------
// 0x06 — BN128_ADD  (EIP-196)
// ---------------------------------------------------------------------------

fn bn128_add(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), EvmError> {
    use substrate_bn::{AffineG1, Fq, G1, Group};
    let cost = 150u64;
    if gas < cost { return Err(EvmError::OutOfGas); }

    let padded = pad_right(input, 128);
    let (p1, p2) = (parse_g1(&padded[0..64])?, parse_g1(&padded[64..128])?);

    let result = p1 + p2;
    encode_g1(&result)
        .map(|out| (out, cost))
}

// ---------------------------------------------------------------------------
// 0x07 — BN128_MUL  (EIP-196)
// ---------------------------------------------------------------------------

fn bn128_mul(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), EvmError> {
    use substrate_bn::{Fr, G1, Group};
    let cost = 6_000u64;
    if gas < cost { return Err(EvmError::OutOfGas); }

    let padded = pad_right(input, 96);
    let p = parse_g1(&padded[0..64])?;

    let mut scalar_bytes = [0u8; 32];
    scalar_bytes.copy_from_slice(&padded[64..96]);
    let scalar = Fr::from_slice(&scalar_bytes)
        .map_err(|_| EvmError::Precompile("bn128_mul: invalid scalar".into()))?;

    let result = p * scalar;
    encode_g1(&result)
        .map(|out| (out, cost))
}

// ---------------------------------------------------------------------------
// 0x08 — BN128_PAIRING  (EIP-197)
// ---------------------------------------------------------------------------

fn bn128_pairing(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), EvmError> {
    use substrate_bn::{pairing_batch, AffineG1, AffineG2, Fq, Fq2, G1, G2, Group, Gt};

    if input.len() % 192 != 0 {
        return Err(EvmError::Precompile("bn128_pairing: input length not multiple of 192".into()));
    }
    let k = (input.len() / 192) as u64;
    let cost = 45_000u64 + 34_000u64 * k;
    if gas < cost { return Err(EvmError::OutOfGas); }

    let mut pairs: Vec<(G1, G2)> = Vec::with_capacity(k as usize);
    for i in 0..(k as usize) {
        let chunk = &input[i * 192..(i + 1) * 192];

        let p1 = parse_g1(&chunk[0..64])?;

        // G2 point: 128 bytes = (x1, x2, y1, y2) each 32 bytes.
        // Ethereum encodes Fq2 as (imaginary, real), so Fq2::new(im, re).
        let x_im = parse_fq(&chunk[64..96])?;
        let x_re = parse_fq(&chunk[96..128])?;
        let y_im = parse_fq(&chunk[128..160])?;
        let y_re = parse_fq(&chunk[160..192])?;

        let p2 = if x_im.is_zero() && x_re.is_zero() && y_im.is_zero() && y_re.is_zero() {
            G2::zero()
        } else {
            let x = Fq2::new(x_im, x_re);
            let y = Fq2::new(y_im, y_re);
            AffineG2::new(x, y)
                .map_err(|_| EvmError::Precompile("bn128_pairing: invalid G2 point".into()))?
                .into()
        };

        pairs.push((p1, p2));
    }

    // Product of pairings == Gt::one() ⟺ pairing check passes.
    let result = pairing_batch(&pairs);
    let success = result == Gt::one();

    let mut out = [0u8; 32];
    if success {
        out[31] = 1;
    }
    Ok((out.to_vec(), cost))
}

// ---------------------------------------------------------------------------
// 0x09 — BLAKE2F  (EIP-152)
//
// Implements the BLAKE2b-F compression function directly — no external crate
// needed. Input is exactly 213 bytes:
//   [0..4]    rounds  (u32 big-endian)
//   [4..68]   h[0..8] (8 × u64 little-endian, initial state)
//   [68..196] m[0..16] (16 × u64 little-endian, message block)
//   [196..212] t[0..2] (2 × u64 little-endian, offset counters)
//   [212]     f (1 byte, final block flag — 0 or 1)
// ---------------------------------------------------------------------------

fn blake2f(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), EvmError> {
    if input.len() != 213 {
        return Err(EvmError::Precompile(format!(
            "BLAKE2F: input must be 213 bytes, got {}",
            input.len()
        )));
    }
    let rounds = u32::from_be_bytes(input[0..4].try_into().unwrap());
    let cost = rounds as u64;
    if gas < cost { return Err(EvmError::OutOfGas); }

    let f = match input[212] {
        0 => false,
        1 => true,
        v => return Err(EvmError::Precompile(format!("BLAKE2F: invalid final flag {v}"))),
    };

    let mut h = [0u64; 8];
    for (i, chunk) in input[4..68].chunks_exact(8).enumerate() {
        h[i] = u64::from_le_bytes(chunk.try_into().unwrap());
    }
    let mut m = [0u64; 16];
    for (i, chunk) in input[68..196].chunks_exact(8).enumerate() {
        m[i] = u64::from_le_bytes(chunk.try_into().unwrap());
    }
    let t0 = u64::from_le_bytes(input[196..204].try_into().unwrap());
    let t1 = u64::from_le_bytes(input[204..212].try_into().unwrap());

    blake2b_compress(rounds, &mut h, &m, [t0, t1], f);

    let mut out = [0u8; 64];
    for (i, word) in h.iter().enumerate() {
        out[i * 8..(i + 1) * 8].copy_from_slice(&word.to_le_bytes());
    }
    Ok((out.to_vec(), cost))
}

// BLAKE2b SIGMA message schedule.
const SIGMA: [[usize; 16]; 10] = [
    [ 0,  1,  2,  3,  4,  5,  6,  7,  8,  9, 10, 11, 12, 13, 14, 15],
    [14, 10,  4,  8,  9, 15, 13,  6,  1, 12,  0,  2, 11,  7,  5,  3],
    [11,  8, 12,  0,  5,  2, 15, 13, 10, 14,  3,  6,  7,  1,  9,  4],
    [ 7,  9,  3,  1, 13, 12, 11, 14,  2,  6,  5, 10,  4,  0, 15,  8],
    [ 9,  0,  5,  7,  2,  4, 10, 15, 14,  1, 11, 12,  6,  8,  3, 13],
    [ 2, 12,  6, 10,  0, 11,  8,  3,  4, 13,  7,  5, 15, 14,  1,  9],
    [12,  5,  1, 15, 14, 13,  4, 10,  0,  7,  6,  3,  9,  2,  8, 11],
    [13, 11,  7, 14, 12,  1,  3,  9,  5,  0, 15,  4,  8,  6,  2, 10],
    [ 6, 15, 14,  9, 11,  3,  0,  8, 12,  2, 13,  7,  1,  4, 10,  5],
    [10,  2,  8,  4,  7,  6,  1,  5, 15, 11,  9, 14,  3, 12, 13,  0],
];

// BLAKE2b initialization vector.
const IV: [u64; 8] = [
    0x6A09E667F3BCC908, 0xBB67AE8584CAA73B,
    0x3C6EF372FE94F82B, 0xA54FF53A5F1D36F1,
    0x510E527FADE682D1, 0x9B05688C2B3E6C1F,
    0x1F83D9ABFB41BD6B, 0x5BE0CD19137E2179,
];

#[inline(always)]
fn g(v: &mut [u64; 16], a: usize, b: usize, c: usize, d: usize, x: u64, y: u64) {
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
    v[d] = (v[d] ^ v[a]).rotate_right(32);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(24);
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
    v[d] = (v[d] ^ v[a]).rotate_right(16);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(63);
}

fn blake2b_compress(rounds: u32, h: &mut [u64; 8], m: &[u64; 16], t: [u64; 2], f: bool) {
    let mut v = [0u64; 16];
    v[..8].copy_from_slice(h);
    v[8..].copy_from_slice(&IV);
    v[12] ^= t[0];
    v[13] ^= t[1];
    if f {
        v[14] = !v[14];
    }
    for r in 0..(rounds as usize) {
        let s = &SIGMA[r % 10];
        g(&mut v,  0,  4,  8, 12, m[s[ 0]], m[s[ 1]]);
        g(&mut v,  1,  5,  9, 13, m[s[ 2]], m[s[ 3]]);
        g(&mut v,  2,  6, 10, 14, m[s[ 4]], m[s[ 5]]);
        g(&mut v,  3,  7, 11, 15, m[s[ 6]], m[s[ 7]]);
        g(&mut v,  0,  5, 10, 15, m[s[ 8]], m[s[ 9]]);
        g(&mut v,  1,  6, 11, 12, m[s[10]], m[s[11]]);
        g(&mut v,  2,  7,  8, 13, m[s[12]], m[s[13]]);
        g(&mut v,  3,  4,  9, 14, m[s[14]], m[s[15]]);
    }
    for i in 0..8 {
        h[i] ^= v[i] ^ v[i + 8];
    }
}

// ---------------------------------------------------------------------------
// BN128 helpers
// ---------------------------------------------------------------------------

fn parse_fq(bytes: &[u8]) -> Result<substrate_bn::Fq, EvmError> {
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes[..32]);
    substrate_bn::Fq::from_slice(&arr)
        .map_err(|_| EvmError::Precompile("bn128: invalid Fq field element".into()))
}

fn parse_g1(bytes: &[u8]) -> Result<substrate_bn::G1, EvmError> {
    use substrate_bn::{AffineG1, G1, Group};
    let x = parse_fq(&bytes[0..32])?;
    let y = parse_fq(&bytes[32..64])?;
    if x.is_zero() && y.is_zero() {
        return Ok(G1::zero());
    }
    AffineG1::new(x, y)
        .map(Into::into)
        .map_err(|_| EvmError::Precompile("bn128: point not on G1 curve".into()))
}

fn encode_g1(p: &substrate_bn::G1) -> Result<Vec<u8>, EvmError> {
    use substrate_bn::{AffineG1, Group};
    let mut out = vec![0u8; 64];
    if let Some(affine) = AffineG1::from_jacobian(*p) {
        affine.x().to_big_endian(&mut out[0..32])
            .map_err(|_| EvmError::Precompile("bn128: x serialization failed".into()))?;
        affine.y().to_big_endian(&mut out[32..64])
            .map_err(|_| EvmError::Precompile("bn128: y serialization failed".into()))?;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

fn pad_right(input: &[u8], len: usize) -> Vec<u8> {
    if input.len() >= len {
        return input[..len].to_vec();
    }
    let mut out = input.to_vec();
    out.resize(len, 0);
    out
}

/// Interpret the first 32 bytes of a big-endian U256 as a usize.
/// Returns an error if the value exceeds usize::MAX.
fn read_u256_as_usize(b: &[u8]) -> Result<usize, ()> {
    // For safety: only allow values that fit in a reasonable allocation.
    // The high 28 bytes must be zero; only the low 4 bytes matter.
    if b.len() < 32 {
        return Err(());
    }
    if b[..28].iter().any(|&x| x != 0) {
        return Err(()); // value > 2^32 — clearly not a valid length
    }
    let low: usize = ((b[28] as usize) << 24)
        | ((b[29] as usize) << 16)
        | ((b[30] as usize) << 8)
        | (b[31] as usize);
    Ok(low)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_empty() {
        let (out, cost) = sha256_hash(&[], 1_000).unwrap();
        assert_eq!(out.len(), 32);
        assert!(cost > 0);
    }

    #[test]
    fn ripemd160_empty() {
        let (out, _) = ripemd160_hash(&[], 10_000).unwrap();
        assert_eq!(out.len(), 32);
        // SHA-256("") = e3b0... ; RIPEMD-160("") = 9c1185a5c5e9fc54612808977ee8f548b2258d31
        assert_eq!(&out[12..], &hex::decode("9c1185a5c5e9fc54612808977ee8f548b2258d31").unwrap()[..]);
    }

    #[test]
    fn blake2f_basic() {
        // 12 rounds, all-zero state, message, t, non-final.
        let mut input = [0u8; 213];
        input[3] = 12; // rounds = 12
        let (out, cost) = blake2f(&input, 1_000).unwrap();
        assert_eq!(out.len(), 64);
        assert_eq!(cost, 12);
    }

    #[test]
    fn blake2f_bad_length() {
        assert!(blake2f(&[0u8; 100], 1_000).is_err());
    }

    #[test]
    fn blake2f_bad_final_flag() {
        let mut input = [0u8; 213];
        input[212] = 2; // invalid final flag
        assert!(blake2f(&input, 1_000).is_err());
    }

    #[test]
    fn modexp_basic() {
        // 2^10 mod 1000 = 24
        let mut input = [0u8; 96 + 3];
        input[31] = 1; // base_len = 1
        input[63] = 1; // exp_len = 1
        input[95] = 2; // mod_len = 2 (we need 2 bytes for 1000)
        // Wait — let me redo with simple values: 3^2 mod 5 = 4
        let mut inp = [0u8; 96 + 1 + 1 + 1];
        inp[31] = 1;    // base_len = 1
        inp[63] = 1;    // exp_len  = 1
        inp[95] = 1;    // mod_len  = 1
        inp[96] = 3;    // base = 3
        inp[97] = 2;    // exp  = 2
        inp[98] = 5;    // mod  = 5
        let (out, _) = modexp(&inp, 1_000_000).unwrap();
        assert_eq!(out, vec![4u8]); // 3^2 mod 5 = 4
    }

    #[test]
    fn bn128_add_identity() {
        // Adding G1 generator + identity = generator.
        // G1 generator: (1, 2) in Fq
        let mut input = [0u8; 128];
        input[31] = 1; // x1 = 1
        input[63] = 2; // y1 = 2
        // p2 = (0, 0) = identity
        let (out, cost) = bn128_add(&input, 10_000).unwrap();
        assert_eq!(out.len(), 64);
        assert_eq!(cost, 150);
    }

    #[test]
    fn bn128_mul_zero_scalar() {
        // P * 0 = identity (0, 0)
        let mut input = [0u8; 96];
        input[31] = 1; // x = 1
        input[63] = 2; // y = 2
        // scalar = 0 (bytes 64..96 remain zero)
        let (out, cost) = bn128_mul(&input, 100_000).unwrap();
        assert_eq!(out.len(), 64);
        assert_eq!(cost, 6_000);
        // Result should be identity (all zeros)
        assert_eq!(out, vec![0u8; 64]);
    }

    #[test]
    fn bn128_pairing_empty() {
        // Empty pairing = 1 (trivially true)
        let (out, cost) = bn128_pairing(&[], 1_000_000).unwrap();
        assert_eq!(out.len(), 32);
        assert_eq!(out[31], 1); // pairing check passes for empty set
        assert_eq!(cost, 45_000);
    }

    // ─── Task #3 (Precompile 0x0A — PayID resolution) tests ─────────────

    #[derive(Default)]
    struct EvmMockLookup {
        forward: std::collections::HashMap<Vec<u8>, [u8; 20]>,
        reverse: std::collections::HashMap<[u8; 20], Vec<u8>>,
    }
    impl PayIdLookup for EvmMockLookup {
        fn resolve(&self, name: &[u8]) -> Option<[u8; 20]> {
            self.forward.get(name).copied()
        }
        fn reverse(&self, addr: &[u8; 20]) -> Option<Vec<u8>> {
            self.reverse.get(addr).cloned()
        }
    }

    fn evm_abi_op_bytes(op: u8, payload: &[u8]) -> Vec<u8> {
        let len = payload.len();
        let padded = (len + 31) / 32 * 32;
        let mut out = vec![0u8; 96 + padded];
        out[31] = op;
        out[63] = 0x40;
        out[92..96].copy_from_slice(&(len as u32).to_be_bytes());
        out[96..96 + len].copy_from_slice(payload);
        out
    }

    #[test]
    fn evm_payid_forward_hit_returns_padded_address() {
        let mut l = EvmMockLookup::default();
        let want = [0x42u8; 20];
        l.forward.insert(b"alice".to_vec(), want);
        let inp = evm_abi_op_bytes(0, b"alice");
        let (out, cost) = do_payid(&inp, 100_000, &l).unwrap();
        assert_eq!(out.len(), 32);
        assert_eq!(&out[12..32], &want);
        assert_eq!(out[..12], [0u8; 12]);
        assert_eq!(cost, 700 + 50 * 5);
    }

    #[test]
    fn evm_payid_forward_miss_returns_zero_no_revert() {
        let l = EvmMockLookup::default();
        let inp = evm_abi_op_bytes(0, b"ghost");
        let (out, _) = do_payid(&inp, 100_000, &l).unwrap();
        assert_eq!(out, vec![0u8; 32]);
    }

    #[test]
    fn evm_payid_reverse_hit_and_miss() {
        let mut l = EvmMockLookup::default();
        let addr = [0x77u8; 20];
        l.reverse.insert(addr, b"bob".to_vec());
        let hit = do_payid(&evm_abi_op_bytes(1, &addr), 100_000, &l).unwrap().0;
        assert_eq!(hit[31], 0x20);
        assert_eq!(&hit[60..64], &[0, 0, 0, 3]);
        assert_eq!(&hit[64..67], b"bob");
        let miss = do_payid(&evm_abi_op_bytes(1, &[0u8; 20]), 100_000, &l).unwrap().0;
        assert_eq!(miss.len(), 64);
        assert_eq!(&miss[60..64], &[0, 0, 0, 0]);
    }

    #[test]
    fn evm_payid_malformed_input_reverts() {
        let l = EvmMockLookup::default();
        // op out of range
        assert!(do_payid(&evm_abi_op_bytes(7, b"alice"), 100_000, &l).is_err());
        // bad name (space)
        assert!(do_payid(&evm_abi_op_bytes(0, b"al ce"), 100_000, &l).is_err());
        // reverse payload not 20 bytes
        assert!(do_payid(&evm_abi_op_bytes(1, b"short"), 100_000, &l).is_err());
        // input too short (< 96 bytes)
        assert!(do_payid(&[0u8; 32], 100_000, &l).is_err());
    }

    #[test]
    fn evm_payid_gas_oog() {
        let l = EvmMockLookup::default();
        // cost = 700 + 50*5 = 950
        let err = do_payid(&evm_abi_op_bytes(0, b"alice"), 700, &l).unwrap_err();
        assert!(matches!(err, EvmError::OutOfGas));
    }

    /// End-to-end registration → resolve roundtrip via the production
    /// EVM host wiring: write the registrar slot through `storage_store`,
    /// then resolve via the precompile body using the host as the
    /// `PayIdLookup`. Proves the slot derivation in `zbx-types::payid`
    /// matches what `MockHost::resolve_pay_id_bytes` reads back.
    #[test]
    fn evm_payid_register_then_resolve_roundtrip_via_host() {
        use crate::host::{Host, MockHost};
        use zbx_types::payid::{
            payid_forward_slot, payid_reverse_slot, PAYID_REGISTRAR_ADDR,
        };

        let mut host = MockHost::new();
        let registrar = Address::from_bytes(&PAYID_REGISTRAR_ADDR).unwrap();
        let alice_addr = [0xAAu8; 20];

        // Simulate `ZbxPayId.register("alice", alice_addr)`:
        let mut fwd_word = [0u8; 32];
        fwd_word[12..32].copy_from_slice(&alice_addr);
        host.storage_store(&registrar, payid_forward_slot(b"alice"), fwd_word);
        let mut rev_word = [0u8; 32];
        rev_word[..5].copy_from_slice(b"alice");
        host.storage_store(&registrar, payid_reverse_slot(&alice_addr), rev_word);

        // Adapter wrapping the host as a `PayIdLookup` (same shape the
        // interpreter uses).
        struct A<'a, H: Host + ?Sized>(&'a H);
        impl<H: Host + ?Sized> PayIdLookup for A<'_, H> {
            fn resolve(&self, n: &[u8]) -> Option<[u8; 20]> { self.0.resolve_pay_id_bytes(n) }
            fn reverse(&self, a: &[u8; 20]) -> Option<Vec<u8>> { self.0.reverse_pay_id(a) }
        }
        let adapter = A(&host);

        // Forward: "alice" → 0xAA*20.
        let (out, _) = do_payid(&evm_abi_op_bytes(0, b"alice"), 100_000, &adapter).unwrap();
        assert_eq!(&out[12..32], &alice_addr,
            "registered name must round-trip via host storage");

        // Reverse: 0xAA*20 → "alice".
        let (out, _) = do_payid(&evm_abi_op_bytes(1, &alice_addr), 100_000, &adapter).unwrap();
        assert_eq!(&out[60..64], &[0, 0, 0, 5]);
        assert_eq!(&out[64..69], b"alice");

        // Unregistered miss: still address(0), no revert.
        let (out, _) = do_payid(&evm_abi_op_bytes(0, b"ghost"), 100_000, &adapter).unwrap();
        assert_eq!(out, vec![0u8; 32]);
    }
}
