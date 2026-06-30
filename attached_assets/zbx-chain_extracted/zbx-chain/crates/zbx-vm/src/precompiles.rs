//! EVM precompiled contracts (addresses 0x01–0x0a).
//!
//! Implementation status (as of 2026-06-27):
//!   0x01 ecrecover          — IMPLEMENTED (zbx_crypto::recover_signer)
//!   0x02 sha256             — IMPLEMENTED (sha2)
//!   0x03 ripemd160          — IMPLEMENTED (ripemd)
//!   0x04 identity           — IMPLEMENTED (copy)
//!   0x05 modexp             — IMPLEMENTED (num-bigint, EIP-198/EIP-2565)
//!   0x06 bn128_add          — IMPLEMENTED (substrate-bn, EIP-196)
//!   0x07 bn128_mul          — IMPLEMENTED (substrate-bn, EIP-196)
//!   0x08 bn128_pairing      — IMPLEMENTED (substrate-bn, EIP-197)
//!   0x09 blake2f            — IMPLEMENTED (inline BLAKE2b-F, EIP-152)
//!   0x0a kzg_point_eval     — IMPLEMENTED (zbx_crypto::kzg, EIP-4844)
//!
//! All implementations are ported from `zbx-evm/src/precompiles.rs` and
//! `zbx-zvm/src/precompiles.rs` so the three execution engines share
//! byte-identical consensus-critical crypto.

use zbx_types::{Address, U256};
use sha2::{Sha256, Digest as Sha2Digest};
use sha3::Keccak256;
use ripemd::Ripemd160;
use num_bigint::BigUint;
use num_traits::Zero;

/// Result of a precompile execution.
pub type PrecompileResult = Result<(u64, Vec<u8>), PrecompileError>;

#[derive(Debug, thiserror::Error)]
pub enum PrecompileError {
    #[error("out of gas")]
    OutOfGas,
    #[error("invalid input")]
    InvalidInput,
    #[error("bn128 pairing failed")]
    BnPairingFailed,
    #[error("precompile error: {0}")]
    Other(String),
}

/// Dispatch a precompile call by address.
pub fn call_precompile(
    address: Address,
    input: &[u8],
    gas_limit: u64,
) -> Option<PrecompileResult> {
    let addr_byte = address.as_bytes()[19];
    if address.as_bytes()[..19] != [0u8; 19] {
        return None;
    }
    match addr_byte {
        0x01 => Some(precompile_ecrecover(input, gas_limit)),
        0x02 => Some(precompile_sha256(input, gas_limit)),
        0x03 => Some(precompile_ripemd160(input, gas_limit)),
        0x04 => Some(precompile_identity(input, gas_limit)),
        0x05 => Some(precompile_modexp(input, gas_limit)),
        0x06 => Some(precompile_bn128_add(input, gas_limit)),
        0x07 => Some(precompile_bn128_mul(input, gas_limit)),
        0x08 => Some(precompile_bn128_pairing(input, gas_limit)),
        0x09 => Some(precompile_blake2f(input, gas_limit)),
        0x0a => Some(precompile_kzg_point_eval(input, gas_limit)),
        _    => None,
    }
}

// ---------------------------------------------------------------------------
// 0x01: ecrecover
// ---------------------------------------------------------------------------

/// 0x01: ecrecover — signature recovery.
///
/// Audit-2026-05-01 S7-VM1: previous body parsed `hash/v/r/s` and then
/// returned `vec![0u8; 32]` regardless. Every Solidity contract on the
/// production VM that called precompile 0x01 received `address(0)`,
/// universally breaking `require(ecrecover(...) == owner)` and silently
/// failing-open whenever `owner` defaulted to zero (uninitialised storage,
/// pre-initialise admin slots, etc.). Wired through to
/// `zbx_crypto::secp256k1::recover_signer` — the same path zbx-evm uses.
/// Per the EVM spec, an invalid signature returns 32 zero bytes (and
/// charges the full 3000 gas) rather than reverting.
fn precompile_ecrecover(input: &[u8], gas_limit: u64) -> PrecompileResult {
    const GAS: u64 = 3_000;
    if gas_limit < GAS { return Err(PrecompileError::OutOfGas); }
    let mut padded = [0u8; 128];
    let n = input.len().min(128);
    padded[..n].copy_from_slice(&input[..n]);

    let hash = zbx_types::H256::from_slice(&padded[0..32]);
    let v_byte = padded[63];
    if padded[32..63].iter().any(|&b| b != 0) {
        return Ok((GAS, vec![0u8; 32]));
    }
    let r = zbx_types::H256::from_slice(&padded[64..96]);
    let s = zbx_types::H256::from_slice(&padded[96..128]);

    let v_norm: u8 = match v_byte {
        27 | 28 => v_byte - 27,
        0 | 1   => v_byte,
        _       => return Ok((GAS, vec![0u8; 32])),
    };

    let sig = zbx_crypto::Signature { v: v_norm, r, s };
    match zbx_crypto::recover_signer(&hash, &sig) {
        Ok(addr) => {
            let mut out = vec![0u8; 32];
            out[12..].copy_from_slice(addr.as_bytes());
            Ok((GAS, out))
        }
        Err(_) => Ok((GAS, vec![0u8; 32])),
    }
}

// ---------------------------------------------------------------------------
// 0x02: sha256
// ---------------------------------------------------------------------------

fn precompile_sha256(input: &[u8], gas_limit: u64) -> PrecompileResult {
    let gas = 60 + 12 * ((input.len() as u64 + 31) / 32);
    if gas_limit < gas { return Err(PrecompileError::OutOfGas); }
    let digest = Sha256::digest(input);
    Ok((gas, digest.to_vec()))
}

// ---------------------------------------------------------------------------
// 0x03: ripemd160
// ---------------------------------------------------------------------------

fn precompile_ripemd160(input: &[u8], gas_limit: u64) -> PrecompileResult {
    let gas = 600 + 120 * ((input.len() as u64 + 31) / 32);
    if gas_limit < gas { return Err(PrecompileError::OutOfGas); }
    let digest = Ripemd160::digest(input);
    let mut out = vec![0u8; 32];
    out[12..32].copy_from_slice(&digest);
    Ok((gas, out))
}

// ---------------------------------------------------------------------------
// 0x04: identity
// ---------------------------------------------------------------------------

fn precompile_identity(input: &[u8], gas_limit: u64) -> PrecompileResult {
    let gas = 15 + 3 * ((input.len() as u64 + 31) / 32);
    if gas_limit < gas { return Err(PrecompileError::OutOfGas); }
    Ok((gas, input.to_vec()))
}

// ---------------------------------------------------------------------------
// 0x05: modexp (EIP-198, gas per EIP-2565)
//
// Ported from zbx-zvm/src/precompiles.rs (Pass-18), which itself tracks the
// zbx-evm reference. All three engines share the same implementation so they
// cannot drift on consensus-critical big-integer exponentiation.
// ---------------------------------------------------------------------------

fn precompile_modexp(input: &[u8], gas_limit: u64) -> PrecompileResult {
    let padded = pad_right(input, 96.max(input.len()));

    let base_len = read_u256_as_usize(&padded[0..32])
        .map_err(|_| PrecompileError::Other("modexp: base_len overflow".into()))?;
    let exp_len = read_u256_as_usize(&padded[32..64])
        .map_err(|_| PrecompileError::Other("modexp: exp_len overflow".into()))?;
    let mod_len = read_u256_as_usize(&padded[64..96])
        .map_err(|_| PrecompileError::Other("modexp: mod_len overflow".into()))?;

    if mod_len == 0 {
        let cost = 200u64;
        if gas_limit < cost { return Err(PrecompileError::OutOfGas); }
        return Ok((cost, vec![]));
    }

    let total = 96usize
        .checked_add(base_len)
        .and_then(|l| l.checked_add(exp_len))
        .and_then(|l| l.checked_add(mod_len))
        .ok_or_else(|| PrecompileError::Other("modexp: length overflow".into()))?;
    let padded = pad_right(&padded, total);

    let base_start = 96;
    let exp_start  = base_start + base_len;
    let mod_start  = exp_start  + exp_len;

    let base    = BigUint::from_bytes_be(&padded[base_start..base_start + base_len]);
    let exp     = BigUint::from_bytes_be(&padded[exp_start..exp_start + exp_len]);
    let modulus = BigUint::from_bytes_be(&padded[mod_start..mod_start + mod_len]);

    let cost = modexp_gas_eip2565(base_len, exp_len, mod_len, &exp);
    if gas_limit < cost { return Err(PrecompileError::OutOfGas); }

    if modulus.is_zero() {
        return Ok((cost, vec![0u8; mod_len]));
    }

    let result = base.modpow(&exp, &modulus);
    let mut result_bytes = result.to_bytes_be();
    while result_bytes.len() < mod_len {
        result_bytes.insert(0, 0u8);
    }
    result_bytes.truncate(mod_len);
    Ok((cost, result_bytes))
}

fn modexp_gas_eip2565(base_len: usize, exp_len: usize, mod_len: usize, exp: &BigUint) -> u64 {
    let max_len = base_len.max(mod_len) as u64;
    // EIP-2565: "words" = count of 8-byte limbs for the larger of base/modulus.
    let words = (max_len + 7) / 8;
    let multiplication_complexity = words * words;

    let iteration_count: u64 = if exp_len <= 32 {
        let exp_bits = exp.bits() as u64;
        if exp_bits == 0 { 0 } else { exp_bits.saturating_sub(1) }
    } else {
        let extra_bits = (8 * (exp_len as u64 - 32)).saturating_sub(1);
        let top_word_bits = {
            let eb = exp.to_bytes_be();
            if eb.len() > 32 {
                BigUint::from_bytes_be(&eb[..32]).bits() as u64
            } else {
                exp.bits() as u64
            }
        };
        extra_bits.saturating_add(top_word_bits.saturating_sub(1))
    };

    ((multiplication_complexity * iteration_count.max(1)) / 3).max(200)
}

// ---------------------------------------------------------------------------
// 0x06: bn128_add (EIP-196)
// ---------------------------------------------------------------------------

/// 0x06: BN128 point addition (EIP-196).
///
/// Input: two G1 affine points P1 and P2, each encoded as (x, y) big-endian
/// 32-byte field elements — total 128 bytes (right-zero-padded if short).
/// Output: 64-byte affine point P1 + P2. Identity (point at infinity) encodes
/// as 64 zero bytes.
fn precompile_bn128_add(input: &[u8], gas_limit: u64) -> PrecompileResult {
    const GAS: u64 = 150;
    if gas_limit < GAS { return Err(PrecompileError::OutOfGas); }

    let padded = pad_right(input, 128);
    let p1 = bn_parse_g1(&padded[0..64])?;
    let p2 = bn_parse_g1(&padded[64..128])?;

    let result = p1 + p2;
    let out = bn_encode_g1(&result)?;
    Ok((GAS, out))
}

// ---------------------------------------------------------------------------
// 0x07: bn128_mul (EIP-196)
// ---------------------------------------------------------------------------

/// 0x07: BN128 scalar multiplication (EIP-196).
///
/// Input: G1 affine point (64 bytes) followed by a 32-byte big-endian scalar.
/// Total 96 bytes (right-zero-padded if short).
/// Output: 64-byte affine point scalar * P.
fn precompile_bn128_mul(input: &[u8], gas_limit: u64) -> PrecompileResult {
    use substrate_bn::Fr;
    const GAS: u64 = 6_000;
    if gas_limit < GAS { return Err(PrecompileError::OutOfGas); }

    let padded = pad_right(input, 96);
    let p = bn_parse_g1(&padded[0..64])?;

    let mut scalar_bytes = [0u8; 32];
    scalar_bytes.copy_from_slice(&padded[64..96]);
    let scalar = Fr::from_slice(&scalar_bytes)
        .map_err(|_| PrecompileError::Other("bn128_mul: invalid Fr scalar".into()))?;

    let result = p * scalar;
    let out = bn_encode_g1(&result)?;
    Ok((GAS, out))
}

// ---------------------------------------------------------------------------
// 0x08: bn128_pairing (EIP-197)
// ---------------------------------------------------------------------------

/// 0x08: BN128 pairing check (EIP-197).
///
/// Input: k × 192-byte tuples (G1 point ‖ G2 point). Empty input → true.
/// Output: 32 bytes — 0x...01 if product of pairings = Gt::one(), else 0x...00.
///
/// **Why this matters:** the previous stub returned `1` for every input,
/// meaning every zk-SNARK / Groth16 verifier on this chain accepted every
/// proof as valid. This implementation computes the real pairing check via
/// `substrate_bn::pairing_batch`.
fn precompile_bn128_pairing(input: &[u8], gas_limit: u64) -> PrecompileResult {
    use substrate_bn::{pairing_batch, AffineG2, Fq2, G1, G2, Group, Gt};

    if input.len() % 192 != 0 {
        return Err(PrecompileError::Other(
            "bn128_pairing: input length not multiple of 192".into(),
        ));
    }
    let k = (input.len() / 192) as u64;
    let gas = 45_000u64.saturating_add(34_000u64.saturating_mul(k));
    if gas_limit < gas { return Err(PrecompileError::OutOfGas); }

    // Empty input → pairing check trivially passes (empty product = identity).
    if k == 0 {
        let mut out = [0u8; 32];
        out[31] = 1;
        return Ok((gas, out.to_vec()));
    }

    let mut pairs: Vec<(G1, G2)> = Vec::with_capacity(k as usize);
    for i in 0..(k as usize) {
        let chunk = &input[i * 192..(i + 1) * 192];

        let p1 = bn_parse_g1(&chunk[0..64])?;

        // G2 point: 128 bytes = (x_im, x_re, y_im, y_re) each 32 bytes.
        // Ethereum encodes Fq2 as (imaginary, real) — Fq2::new(im, re).
        let x_im = bn_parse_fq(&chunk[64..96])?;
        let x_re = bn_parse_fq(&chunk[96..128])?;
        let y_im = bn_parse_fq(&chunk[128..160])?;
        let y_re = bn_parse_fq(&chunk[160..192])?;

        let p2 = if x_im.is_zero() && x_re.is_zero() && y_im.is_zero() && y_re.is_zero() {
            G2::zero()
        } else {
            let x = Fq2::new(x_im, x_re);
            let y = Fq2::new(y_im, y_re);
            AffineG2::new(x, y)
                .map_err(|_| PrecompileError::Other("bn128_pairing: invalid G2 point".into()))?
                .into()
        };

        pairs.push((p1, p2));
    }

    let result = pairing_batch(&pairs);
    let success = result == Gt::one();
    let mut out = [0u8; 32];
    if success { out[31] = 1; }
    Ok((gas, out.to_vec()))
}

// ---------------------------------------------------------------------------
// 0x09: BLAKE2F (EIP-152)
//
// Implements the BLAKE2b-F compression function inline — no external crate.
// Input is exactly 213 bytes:
//   [0..4]     rounds   (u32 big-endian)
//   [4..68]    h[0..8]  (8 × u64 little-endian, initial state)
//   [68..196]  m[0..16] (16 × u64 little-endian, message block)
//   [196..212] t[0..2]  (2 × u64 little-endian, offset counters)
//   [212]      f        (1 byte, final-block flag — 0 or 1)
//
// Ported from zbx-evm/src/precompiles.rs (inline, no extra crate).
// ---------------------------------------------------------------------------

fn precompile_blake2f(input: &[u8], gas_limit: u64) -> PrecompileResult {
    if input.len() != 213 {
        return Err(PrecompileError::Other(format!(
            "blake2f: input must be 213 bytes, got {}",
            input.len()
        )));
    }
    let rounds = u32::from_be_bytes(input[0..4].try_into().unwrap());
    let gas = rounds as u64;
    if gas_limit < gas { return Err(PrecompileError::OutOfGas); }

    let f = match input[212] {
        0 => false,
        1 => true,
        v => return Err(PrecompileError::Other(format!("blake2f: invalid final flag {v}"))),
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
    Ok((gas, out.to_vec()))
}

// BLAKE2b SIGMA message schedule (10 rounds, cycled).
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

// BLAKE2b initialization vector (SHA-512 fractional cube roots of primes 2–19).
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
    if f { v[14] = !v[14]; }
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
// 0x0a: KZG point evaluation (EIP-4844)
//
// Input: 192 bytes
//   [0..32]   versioned_hash   (bytes32 — keccak256 of the KZG commitment)
//   [32..64]  z                (BLS12-381 scalar — evaluation point)
//   [64..96]  y                (BLS12-381 scalar — claimed evaluation)
//   [96..144] commitment       (48-byte compressed G1 — KZG commitment)
//   [144..192] proof           (48-byte compressed G1 — KZG proof)
//
// Output on success: 64 bytes
//   [0..32]   FIELD_ELEMENTS_PER_BLOB as uint256 (4096)
//   [32..64]  BLS_MODULUS as uint256
//
// Backed by `zbx_crypto::kzg::do_kzg_point_eval` — the same path zbx-zvm
// uses, ensuring byte-identical consensus behaviour across all three VMs.
// ---------------------------------------------------------------------------

fn precompile_kzg_point_eval(input: &[u8], gas_limit: u64) -> PrecompileResult {
    const GAS: u64 = 50_000;
    if gas_limit < GAS { return Err(PrecompileError::OutOfGas); }
    if input.len() != 192 { return Err(PrecompileError::InvalidInput); }

    let settings = zbx_crypto::kzg::global_kzg_settings()
        .ok_or_else(|| PrecompileError::Other(
            "kzg_point_eval (0x0a): trusted setup not initialised; \
             call zbx_crypto::kzg::init_global_kzg_settings at node startup"
                .into(),
        ))?;

    zbx_crypto::kzg::do_kzg_point_eval(input, GAS, &settings)
        .map(|(out, gas_used)| (gas_used, out))
        .map_err(|e| {
            use zbx_crypto::kzg::KzgError;
            match e {
                KzgError::OutOfGas { .. } => PrecompileError::OutOfGas,
                other => PrecompileError::Other(format!("kzg: {other}")),
            }
        })
}

// ---------------------------------------------------------------------------
// BN128 helpers (substrate-bn)
// ---------------------------------------------------------------------------

fn bn_parse_fq(bytes: &[u8]) -> Result<substrate_bn::Fq, PrecompileError> {
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes[..32]);
    substrate_bn::Fq::from_slice(&arr)
        .map_err(|_| PrecompileError::Other("bn128: invalid Fq field element".into()))
}

fn bn_parse_g1(bytes: &[u8]) -> Result<substrate_bn::G1, PrecompileError> {
    use substrate_bn::{AffineG1, G1, Group};
    let x = bn_parse_fq(&bytes[0..32])?;
    let y = bn_parse_fq(&bytes[32..64])?;
    if x.is_zero() && y.is_zero() {
        return Ok(G1::zero());
    }
    AffineG1::new(x, y)
        .map(Into::into)
        .map_err(|_| PrecompileError::Other("bn128: point not on G1 curve".into()))
}

fn bn_encode_g1(p: &substrate_bn::G1) -> Result<Vec<u8>, PrecompileError> {
    use substrate_bn::{AffineG1, Group};
    let mut out = vec![0u8; 64];
    if let Some(affine) = AffineG1::from_jacobian(*p) {
        affine.x().to_big_endian(&mut out[0..32])
            .map_err(|_| PrecompileError::Other("bn128: x serialization failed".into()))?;
        affine.y().to_big_endian(&mut out[32..64])
            .map_err(|_| PrecompileError::Other("bn128: y serialization failed".into()))?;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Utility helpers
// ---------------------------------------------------------------------------

fn pad_right(input: &[u8], len: usize) -> Vec<u8> {
    if input.len() >= len {
        return input[..len].to_vec();
    }
    let mut out = input.to_vec();
    out.resize(len, 0);
    out
}

/// Read the first 32 big-endian bytes as a usize.
/// Returns Err if the value exceeds 2^32 (obviously not a valid length).
fn read_u256_as_usize(b: &[u8]) -> Result<usize, ()> {
    if b.len() < 32 { return Err(()); }
    if b[..28].iter().any(|&x| x != 0) { return Err(()); }
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

    // ── sha256 ──────────────────────────────────────────────────────────────

    #[test]
    fn sha256_empty_input() {
        let (gas, out) = precompile_sha256(&[], 1_000).unwrap();
        assert_eq!(out.len(), 32);
        assert_eq!(gas, 60);
        // SHA-256("") = e3b0c442...
        assert_eq!(out[0], 0xe3);
    }

    #[test]
    fn sha256_out_of_gas() {
        assert!(matches!(precompile_sha256(&[], 59), Err(PrecompileError::OutOfGas)));
    }

    // ── identity ────────────────────────────────────────────────────────────

    #[test]
    fn identity_copies_input() {
        let data = b"hello world";
        let (_, out) = precompile_identity(data, 1_000).unwrap();
        assert_eq!(&out, data);
    }

    // ── modexp ──────────────────────────────────────────────────────────────

    #[test]
    fn modexp_two_to_the_power_three_mod_five() {
        // base=2, exp=3, mod=5 → 2^3 mod 5 = 8 mod 5 = 3
        let mut input = vec![0u8; 96];
        // base_len = 1
        input[31] = 1;
        // exp_len = 1
        input[63] = 1;
        // mod_len = 1
        input[95] = 1;
        input.push(2); // base
        input.push(3); // exp
        input.push(5); // modulus
        let (_, out) = precompile_modexp(&input, 1_000_000).unwrap();
        assert_eq!(out, vec![3u8]);
    }

    #[test]
    fn modexp_zero_modulus_returns_empty() {
        // mod_len = 0 → should return empty vec
        let input = vec![0u8; 96];
        let (cost, out) = precompile_modexp(&input, 1_000_000).unwrap();
        assert!(out.is_empty());
        assert_eq!(cost, 200);
    }

    #[test]
    fn modexp_out_of_gas() {
        let input = vec![0u8; 96];
        assert!(matches!(precompile_modexp(&input, 0), Err(PrecompileError::OutOfGas)));
    }

    // ── bn128_add ───────────────────────────────────────────────────────────

    #[test]
    fn bn128_add_generator_plus_identity() {
        // G1 generator (1, 2) + identity (0, 0) = generator (1, 2)
        let mut input = vec![0u8; 128];
        // x = 1
        input[31] = 1;
        // y = 2
        input[63] = 2;
        // p2 = (0,0) identity — already zero
        let (_, out) = precompile_bn128_add(&input, 10_000).unwrap();
        assert_eq!(out.len(), 64);
        // result x should end in 1
        assert_eq!(out[31], 1);
        // result y should end in 2
        assert_eq!(out[63], 2);
    }

    #[test]
    fn bn128_add_identity_plus_identity_is_identity() {
        let input = vec![0u8; 128];
        let (_, out) = precompile_bn128_add(&input, 10_000).unwrap();
        assert_eq!(out, vec![0u8; 64]);
    }

    #[test]
    fn bn128_add_out_of_gas() {
        let input = vec![0u8; 128];
        assert!(matches!(
            precompile_bn128_add(&input, 149),
            Err(PrecompileError::OutOfGas)
        ));
    }

    // ── bn128_mul ───────────────────────────────────────────────────────────

    #[test]
    fn bn128_mul_zero_scalar_yields_identity() {
        // scalar = 0 → any_point * 0 = identity
        let mut input = vec![0u8; 96];
        input[31] = 1; // x = 1
        input[63] = 2; // y = 2
        // scalar bytes [64..96] all zero
        let (_, out) = precompile_bn128_mul(&input, 100_000).unwrap();
        assert_eq!(out, vec![0u8; 64]);
    }

    #[test]
    fn bn128_mul_one_scalar_is_identity_op() {
        // 1 * generator = generator
        let mut input = vec![0u8; 96];
        input[31] = 1; // x = 1
        input[63] = 2; // y = 2
        input[95] = 1; // scalar = 1
        let (_, out) = precompile_bn128_mul(&input, 100_000).unwrap();
        assert_eq!(out[31], 1);
        assert_eq!(out[63], 2);
    }

    // ── bn128_pairing ───────────────────────────────────────────────────────

    #[test]
    fn bn128_pairing_empty_input_returns_true() {
        let (_, out) = precompile_bn128_pairing(&[], 1_000_000).unwrap();
        assert_eq!(out.len(), 32);
        assert_eq!(out[31], 1);
    }

    #[test]
    fn bn128_pairing_bad_length_errors() {
        assert!(matches!(
            precompile_bn128_pairing(&[0u8; 100], 1_000_000),
            Err(PrecompileError::Other(_))
        ));
    }

    // ── blake2f ─────────────────────────────────────────────────────────────

    #[test]
    fn blake2f_wrong_length_errors() {
        assert!(matches!(
            precompile_blake2f(&[0u8; 100], 1_000_000),
            Err(PrecompileError::Other(_))
        ));
    }

    #[test]
    fn blake2f_invalid_final_flag_errors() {
        let mut input = vec![0u8; 213];
        input[212] = 2; // invalid: must be 0 or 1
        assert!(matches!(
            precompile_blake2f(&input, 1_000_000),
            Err(PrecompileError::Other(_))
        ));
    }

    #[test]
    fn blake2f_zero_rounds_returns_initial_state_unchanged() {
        // rounds=0, f=0 → h unchanged (no mixing rounds, no inversion)
        let mut input = vec![0u8; 213];
        // rounds = 0 (bytes 0..4 already zero)
        // h[0..8] = IV (standard initial state)
        let iv: [u64; 8] = [
            0x6A09E667F3BCC908, 0xBB67AE8584CAA73B,
            0x3C6EF372FE94F82B, 0xA54FF53A5F1D36F1,
            0x510E527FADE682D1, 0x9B05688C2B3E6C1F,
            0x1F83D9ABFB41BD6B, 0x5BE0CD19137E2179,
        ];
        for (i, &word) in iv.iter().enumerate() {
            let bytes = word.to_le_bytes();
            input[4 + i * 8..4 + (i + 1) * 8].copy_from_slice(&bytes);
        }
        // m = 0, t = 0, f = 0 — leaves h unchanged across 0 rounds
        let (gas, out) = precompile_blake2f(&input, 1_000_000).unwrap();
        assert_eq!(gas, 0);
        assert_eq!(out.len(), 64);
        // With 0 rounds the XOR step (h[i] ^= v[i] ^ v[i+8]) still runs,
        // but v[..8] = h (IV) and v[8..] = IV, so h[i] ^= IV[i] ^ IV[i] = 0.
        // Result should be the original h = IV.
        for (i, &word) in iv.iter().enumerate() {
            let chunk: [u8; 8] = out[i * 8..(i + 1) * 8].try_into().unwrap();
            assert_eq!(u64::from_le_bytes(chunk), word,
                "IV word {i} mismatch after 0-round compress");
        }
    }

    #[test]
    fn blake2f_out_of_gas_when_rounds_exceed_limit() {
        let mut input = vec![0u8; 213];
        // rounds = 1000 (big-endian u32)
        input[0..4].copy_from_slice(&1000u32.to_be_bytes());
        // gas_limit = 500 < 1000 → OutOfGas
        assert!(matches!(
            precompile_blake2f(&input, 500),
            Err(PrecompileError::OutOfGas)
        ));
    }

    // ── dispatch ────────────────────────────────────────────────────────────

    #[test]
    fn dispatch_non_precompile_returns_none() {
        let addr = zbx_types::Address::from_slice(&[0xde, 0xad, 0xbe, 0xef,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        assert!(call_precompile(addr, &[], 1_000_000).is_none());
    }

    #[test]
    fn dispatch_sha256_at_address_two() {
        let mut bytes = [0u8; 20];
        bytes[19] = 0x02;
        let addr = zbx_types::Address::from_slice(&bytes);
        let result = call_precompile(addr, b"abc", 1_000);
        assert!(result.is_some());
        assert!(result.unwrap().is_ok());
    }
}
