//! ZVM precompiled contracts — EVM standard (0x01–0x09) + ZBX native (0x0A–0x0F).
//!
//! SEC-2026-05-09 Pass-18: previous Pass-13 audit closed every "fake-success"
//! body by replacing it with `Err(InvalidInput)` (fail-closed). That blocked
//! every modern Solidity contract on testnet — Uniswap (BN128 for fee math),
//! Tornado / zk apps (BN128 pairing + MODEXP), Multisig wallets (RIPEMD-160),
//! cross-chain bridges (BLAKE2F for Equihash / Filecoin proofs), Solana-style
//! bridges (Ed25519 signatures), and any contract using `permit()` with a
//! ledger device that signs Bitcoin-style addresses.
//!
//! Pass-18 wires real implementations for:
//!   * 0x03 — RIPEMD-160        (real `ripemd` crate)
//!   * 0x05 — MODEXP            (`num-bigint`, EIP-198 / EIP-2565 gas)
//!   * 0x06 — BN128_ADD         (`substrate-bn`, EIP-196)
//!   * 0x07 — BN128_MUL         (`substrate-bn`, EIP-196)
//!   * 0x08 — BN128_PAIRING     (`substrate-bn`, EIP-197)
//!   * 0x09 — BLAKE2F           (inline BLAKE2b-F compression, EIP-152)
//!   * 0x0D — ED25519_VERIFY    (real `ed25519-dalek`)
//!
//! Implementations are ported wholesale from `zbx-evm/src/precompiles.rs` so
//! the two execution engines (EVM + ZVM) cannot drift on consensus-critical
//! cryptography. ZVM-native precompiles 0x0A / 0x0B / 0x0C / 0x0E / 0x0F that
//! depend on chain state or external oracles remain fail-closed until the
//! production host pipes them through `zbx-state`.

use crate::error::ZvmError;
use sha2::{Digest as _, Sha256};

/// Precompile address range.
pub const EVM_PRECOMPILE_RANGE: std::ops::RangeInclusive<u8> = 0x01..=0x09;
pub const ZVM_PRECOMPILE_RANGE: std::ops::RangeInclusive<u8> = 0x0A..=0x0F;

/// ZVM precompile addresses.
pub mod addresses {
    // Standard EVM precompiles (same as Ethereum)
    pub const ECRECOVER:         [u8; 20] = addr(0x01);
    pub const SHA256:            [u8; 20] = addr(0x02);
    pub const RIPEMD160:         [u8; 20] = addr(0x03);
    pub const IDENTITY:          [u8; 20] = addr(0x04);
    pub const MODEXP:            [u8; 20] = addr(0x05);
    pub const BN128_ADD:         [u8; 20] = addr(0x06);
    pub const BN128_MUL:         [u8; 20] = addr(0x07);
    pub const BN128_PAIRING:     [u8; 20] = addr(0x08);
    pub const BLAKE2F:           [u8; 20] = addr(0x09);

    // ZVM-native precompiles
    /// 0x0A: Resolve Pay ID. Input: UTF-8 Pay ID string. Output: 20-byte address.
    pub const PAYID_RESOLVE:     [u8; 20] = addr(0x0A);
    /// 0x0B: BLS12-381 KZG verification (for DA layer proofs).
    pub const KZG_VERIFY:        [u8; 20] = addr(0x0B);
    /// 0x0C: ZBX/USD price oracle read. Input: none. Output: uint256 price (18 dec).
    pub const PRICE_ORACLE:      [u8; 20] = addr(0x0C);
    /// 0x0D: Ed25519 signature verification (ZBX uses Ed25519 keys).
    pub const ED25519_VERIFY:    [u8; 20] = addr(0x0D);
    /// 0x0E: VRF (Verifiable Random Function) output verification.
    pub const VRF_VERIFY:        [u8; 20] = addr(0x0E);
    /// 0x0F: ZUSD balance query. Input: 20-byte address. Output: uint256 balance.
    pub const ZUSD_BALANCE:      [u8; 20] = addr(0x0F);

    const fn addr(n: u8) -> [u8; 20] {
        let mut a = [0u8; 20];
        a[19] = n;
        a
    }
}

/// Call a precompile contract.
/// Returns (output_data, gas_used) or Err if the precompile fails.
pub fn call_precompile(
    address: &[u8; 20],
    input: &[u8],
    gas_limit: u64,
) -> Result<(Vec<u8>, u64), ZvmError> {
    let id = address[19];

    match id {
        // ── EVM standard precompiles ──────────────────────────────────────
        0x01 => ecrecover(input, gas_limit),
        0x02 => sha256_hash(input, gas_limit),
        0x03 => ripemd160_hash(input, gas_limit),
        0x04 => identity(input, gas_limit),
        0x05 => modexp(input, gas_limit),
        0x06 => bn128_add(input, gas_limit),
        0x07 => bn128_mul(input, gas_limit),
        0x08 => bn128_pairing(input, gas_limit),
        0x09 => blake2f(input, gas_limit),

        // ── ZVM native precompiles ────────────────────────────────────────
        // 0x0A (PayID) is stateful — dispatched directly by the interpreter
        // via `payid_resolve_with` which has access to the host. This
        // stateless dispatcher path is unreachable in practice; if it is
        // ever hit (e.g. external static-analysis tooling) we fail closed.
        0x0A => Err(ZvmError::InvalidInput(
            "payid (0x0A) requires host context; call via interpreter".into(),
        )),
        0x0B => kzg_verify(input, gas_limit),
        // Task #5: 0x0C is stateful — see `price_oracle_with`. Dispatcher
        // path is unreachable in production; fail-closed for safety.
        0x0C => Err(ZvmError::InvalidInput(
            "price_oracle (0x0C) requires host context; call via interpreter".into(),
        )),
        0x0D => ed25519_verify(input, gas_limit),
        0x0E => vrf_verify(input, gas_limit),
        // Task #7: 0x0F (ZUSD vault state direct-read) is stateful — see
        // `zusd_vault_with`. Dispatcher path is unreachable in production;
        // fail-closed for safety (matches Task #5 0x0C convention).
        0x0F => Err(ZvmError::InvalidInput(
            "zusd_vault (0x0F) requires host context; call via interpreter".into(),
        )),

        _ => Err(ZvmError::PrecompileNotFound(*address)),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 0x01 — ECRECOVER (Audit-2026-05-01 S7-VM2 — already real, kept verbatim)
// ─────────────────────────────────────────────────────────────────────────────

fn ecrecover(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), ZvmError> {
    let cost = 3000u64;
    if gas < cost { return Err(ZvmError::OutOfGas); }
    let mut padded = [0u8; 128];
    let n = input.len().min(128);
    padded[..n].copy_from_slice(&input[..n]);

    let hash = zbx_types::H256::from_slice(&padded[0..32]);
    if padded[32..63].iter().any(|&b| b != 0) {
        return Ok((vec![0u8; 32], cost));
    }
    let v_byte = padded[63];
    let r = zbx_types::H256::from_slice(&padded[64..96]);
    let s = zbx_types::H256::from_slice(&padded[96..128]);

    let v_norm: u8 = match v_byte {
        27 | 28 => v_byte - 27,
        0 | 1   => v_byte,
        _       => return Ok((vec![0u8; 32], cost)),
    };

    let sig = zbx_crypto::Signature { v: v_norm, r, s };
    match zbx_crypto::recover_signer(&hash, &sig) {
        Ok(addr) => {
            let mut out = vec![0u8; 32];
            out[12..].copy_from_slice(addr.as_bytes());
            Ok((out, cost))
        }
        Err(_) => Ok((vec![0u8; 32], cost)),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 0x02 — SHA-256
// ─────────────────────────────────────────────────────────────────────────────

fn sha256_hash(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), ZvmError> {
    let cost = 60 + 12 * ((input.len() as u64 + 31) / 32);
    if gas < cost { return Err(ZvmError::OutOfGas); }
    Ok((Sha256::digest(input).to_vec(), cost))
}

// ─────────────────────────────────────────────────────────────────────────────
// 0x03 — RIPEMD-160 (Pass-18: real ripemd crate; was fail-closed since Pass-13)
// ─────────────────────────────────────────────────────────────────────────────

fn ripemd160_hash(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), ZvmError> {
    use ripemd::{Ripemd160, Digest as _};
    let cost = 600 + 120 * ((input.len() as u64 + 31) / 32);
    if gas < cost { return Err(ZvmError::OutOfGas); }
    let digest = Ripemd160::digest(input);
    // EVM pads the 20-byte hash to 32 bytes (left-padded with zeros).
    let mut out = [0u8; 32];
    out[12..].copy_from_slice(&digest);
    Ok((out.to_vec(), cost))
}

// ─────────────────────────────────────────────────────────────────────────────
// 0x04 — IDENTITY
// ─────────────────────────────────────────────────────────────────────────────

fn identity(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), ZvmError> {
    let cost = 15 + 3 * ((input.len() as u64 + 31) / 32);
    if gas < cost { return Err(ZvmError::OutOfGas); }
    Ok((input.to_vec(), cost))
}

// ─────────────────────────────────────────────────────────────────────────────
// 0x05 — MODEXP (Pass-18: real num-bigint, EIP-198 + EIP-2565 gas schedule)
// ─────────────────────────────────────────────────────────────────────────────

fn modexp(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), ZvmError> {
    use num_bigint::BigUint;
    use num_traits::Zero;

    let padded = pad_right(input, 96.max(input.len()));

    let base_len = read_u256_as_usize(&padded[0..32])
        .map_err(|_| ZvmError::InvalidInput("modexp: base_len overflow".into()))?;
    let exp_len = read_u256_as_usize(&padded[32..64])
        .map_err(|_| ZvmError::InvalidInput("modexp: exp_len overflow".into()))?;
    let mod_len = read_u256_as_usize(&padded[64..96])
        .map_err(|_| ZvmError::InvalidInput("modexp: mod_len overflow".into()))?;

    if mod_len == 0 {
        let cost = 200u64;
        if gas < cost { return Err(ZvmError::OutOfGas); }
        return Ok((vec![], cost));
    }

    let total = 96usize
        .checked_add(base_len).and_then(|l| l.checked_add(exp_len))
        .and_then(|l| l.checked_add(mod_len))
        .ok_or_else(|| ZvmError::InvalidInput("modexp: length overflow".into()))?;
    let padded = pad_right(&padded, total);

    let base_start = 96;
    let exp_start  = base_start + base_len;
    let mod_start  = exp_start  + exp_len;

    let base    = BigUint::from_bytes_be(&padded[base_start..base_start + base_len]);
    let exp     = BigUint::from_bytes_be(&padded[exp_start..exp_start + exp_len]);
    let modulus = BigUint::from_bytes_be(&padded[mod_start..mod_start + mod_len]);

    let cost = modexp_gas_eip2565(base_len, exp_len, mod_len, &exp);
    if gas < cost { return Err(ZvmError::OutOfGas); }

    if modulus.is_zero() {
        return Ok((vec![0u8; mod_len], cost));
    }

    let result = base.modpow(&exp, &modulus);
    let mut result_bytes = result.to_bytes_be();
    while result_bytes.len() < mod_len {
        result_bytes.insert(0, 0u8);
    }
    result_bytes.truncate(mod_len);
    Ok((result_bytes, cost))
}

fn modexp_gas_eip2565(base_len: usize, exp_len: usize, mod_len: usize, exp: &num_bigint::BigUint) -> u64 {
    let max_len = base_len.max(mod_len) as u64;
    // EIP-2565: "words" here is the count of 8-byte limbs needed to represent
    // the larger of base or modulus — NOT the EVM 32-byte machine word. The
    // multiplication-complexity term is `words²` per the spec. (Architect
    // review Pass-18 LOW: name kept for spec parity, comment added to avoid
    // confusion with EVM's 32-byte word concept.)
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

// ─────────────────────────────────────────────────────────────────────────────
// 0x06 — BN128_ADD (Pass-18: real substrate-bn, EIP-196)
// ─────────────────────────────────────────────────────────────────────────────

fn bn128_add(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), ZvmError> {
    let cost = 150u64;
    if gas < cost { return Err(ZvmError::OutOfGas); }

    let padded = pad_right(input, 128);
    let p1 = parse_g1(&padded[0..64])?;
    let p2 = parse_g1(&padded[64..128])?;

    let result = p1 + p2;
    encode_g1(&result).map(|out| (out, cost))
}

// ─────────────────────────────────────────────────────────────────────────────
// 0x07 — BN128_MUL (Pass-18: real substrate-bn, EIP-196)
// ─────────────────────────────────────────────────────────────────────────────

fn bn128_mul(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), ZvmError> {
    use substrate_bn::Fr;
    let cost = 6_000u64;
    if gas < cost { return Err(ZvmError::OutOfGas); }

    let padded = pad_right(input, 96);
    let p = parse_g1(&padded[0..64])?;

    let mut scalar_bytes = [0u8; 32];
    scalar_bytes.copy_from_slice(&padded[64..96]);
    let scalar = Fr::from_slice(&scalar_bytes)
        .map_err(|_| ZvmError::InvalidInput("bn128_mul: invalid scalar".into()))?;

    let result = p * scalar;
    encode_g1(&result).map(|out| (out, cost))
}

// ─────────────────────────────────────────────────────────────────────────────
// 0x08 — BN128_PAIRING (Pass-18: real substrate-bn, EIP-197)
// ─────────────────────────────────────────────────────────────────────────────

fn bn128_pairing(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), ZvmError> {
    use substrate_bn::{pairing_batch, AffineG2, Fq2, G1, G2, Group, Gt};

    if input.len() % 192 != 0 {
        return Err(ZvmError::InvalidInput(
            "bn128_pairing: input length not multiple of 192".into(),
        ));
    }
    let k = (input.len() / 192) as u64;
    let cost = 45_000u64 + 34_000u64 * k;
    if gas < cost { return Err(ZvmError::OutOfGas); }

    let mut pairs: Vec<(G1, G2)> = Vec::with_capacity(k as usize);
    for i in 0..(k as usize) {
        let chunk = &input[i * 192..(i + 1) * 192];

        let p1 = parse_g1(&chunk[0..64])?;

        // Ethereum encodes Fq2 as (imaginary, real) for both x and y.
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
                .map_err(|_| ZvmError::InvalidInput("bn128_pairing: invalid G2 point".into()))?
                .into()
        };

        pairs.push((p1, p2));
    }

    let result = pairing_batch(&pairs);
    let success = result == Gt::one();

    let mut out = [0u8; 32];
    if success {
        out[31] = 1;
    }
    Ok((out.to_vec(), cost))
}

// ─────────────────────────────────────────────────────────────────────────────
// 0x09 — BLAKE2F (Pass-18: inline BLAKE2b-F compression, EIP-152)
//
// Input is exactly 213 bytes:
//   [0..4]    rounds  (u32 big-endian)
//   [4..68]   h[0..8] (8 × u64 little-endian, initial state)
//   [68..196] m[0..16] (16 × u64 little-endian, message block)
//   [196..212] t[0..2] (2 × u64 little-endian, offset counters)
//   [212]     f (1 byte, final block flag — 0 or 1)
// ─────────────────────────────────────────────────────────────────────────────

fn blake2f(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), ZvmError> {
    if input.len() != 213 {
        return Err(ZvmError::InvalidInput(format!(
            "BLAKE2F: input must be 213 bytes, got {}",
            input.len()
        )));
    }
    let rounds = u32::from_be_bytes(input[0..4].try_into().unwrap());
    let cost = rounds as u64;
    if gas < cost { return Err(ZvmError::OutOfGas); }

    let f = match input[212] {
        0 => false,
        1 => true,
        v => return Err(ZvmError::InvalidInput(format!("BLAKE2F: invalid final flag {v}"))),
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

// ─────────────────────────────────────────────────────────────────────────────
// BN128 helpers
// ─────────────────────────────────────────────────────────────────────────────

fn parse_fq(bytes: &[u8]) -> Result<substrate_bn::Fq, ZvmError> {
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes[..32]);
    substrate_bn::Fq::from_slice(&arr)
        .map_err(|_| ZvmError::InvalidInput("bn128: invalid Fq field element".into()))
}

fn parse_g1(bytes: &[u8]) -> Result<substrate_bn::G1, ZvmError> {
    use substrate_bn::{AffineG1, G1, Group};
    let x = parse_fq(&bytes[0..32])?;
    let y = parse_fq(&bytes[32..64])?;
    if x.is_zero() && y.is_zero() {
        return Ok(G1::zero());
    }
    AffineG1::new(x, y)
        .map(Into::into)
        .map_err(|_| ZvmError::InvalidInput("bn128: point not on G1 curve".into()))
}

fn encode_g1(p: &substrate_bn::G1) -> Result<Vec<u8>, ZvmError> {
    use substrate_bn::AffineG1;
    let mut out = vec![0u8; 64];
    if let Some(affine) = AffineG1::from_jacobian(*p) {
        affine.x().to_big_endian(&mut out[0..32])
            .map_err(|_| ZvmError::InvalidInput("bn128: x serialization failed".into()))?;
        affine.y().to_big_endian(&mut out[32..64])
            .map_err(|_| ZvmError::InvalidInput("bn128: y serialization failed".into()))?;
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// Utility
// ─────────────────────────────────────────────────────────────────────────────

fn pad_right(input: &[u8], len: usize) -> Vec<u8> {
    if input.len() >= len {
        return input[..len].to_vec();
    }
    let mut out = input.to_vec();
    out.resize(len, 0);
    out
}

/// Interpret the first 32 bytes of a big-endian U256 as a usize. Returns
/// an error if the value exceeds 2^32 (clearly not a valid length).
fn read_u256_as_usize(b: &[u8]) -> Result<usize, ()> {
    if b.len() < 32 { return Err(()); }
    if b[..28].iter().any(|&x| x != 0) { return Err(()); }
    let low: usize = ((b[28] as usize) << 24)
        | ((b[29] as usize) << 16)
        | ((b[30] as usize) << 8)
        | (b[31] as usize);
    Ok(low)
}

// ─────────────────────────────────────────────────────────────────────────────
// ZVM-native precompiles
// ─────────────────────────────────────────────────────────────────────────────

/// Task #3 (Precompile 0x0A): live PayID resolver.
///
/// Stateful — needs read-only access to the chain's registry, supplied
/// through the [`PayIdLookup`] trait (the production ZVM host wraps
/// itself; tests pass a `HashMap`-backed mock). Replaces the Pass-18
/// fail-closed `Err(InvalidInput)` body.
///
/// **ABI input** (`(uint8 op, bytes name)`, standard Solidity dynamic-tuple
/// encoding):
///
/// ```text
/// [  0..32 ] uint8 op            (right-aligned in a 32-byte word)
/// [ 32..64 ] uint256 head_offset (= 0x40 — points to the bytes block)
/// [ 64..96 ] uint256 length L    (length of the bytes payload)
/// [ 96..96+L] payload            (UTF-8 name for op=0; 20-byte addr for op=1)
/// ```
///
/// The decoder is liberal about the head-offset (only `length` and
/// `payload` fields are actually consulted) so that hand-rolled callers
/// that omit the offset word still resolve as long as the layout above
/// holds. Lengths beyond `input.len() - 96` are rejected.
///
/// **Operations:**
///
/// * `op = 0` (forward): payload is the ASCII PayID name (with optional
///   `@zbx` suffix, stripped before lookup). Output is a 32-byte word
///   with the resolved address right-aligned (bytes `[12..32]`). An
///   unregistered name returns the all-zero word — **not** an error —
///   so Solidity callers can branch on `address(0)` without try/catch.
///
/// * `op = 1` (reverse): payload is exactly 20 bytes of address. Output
///   is the standard Solidity `string` ABI encoding
///   (`offset || length || data_padded`). An unregistered address returns
///   an empty string (length 0).
///
/// * Any other `op` value reverts with `InvalidInput`.
///
/// **Gas:** flat `700 + 50 * payload_len` (matches the order-of-magnitude
/// cost of `EXTCODEHASH` + a SLOAD, which is the actual chain-state work
/// the host does).
///
/// **Validation:** for `op = 0` the payload (post-`@zbx`-strip) must
/// match `^[a-z0-9._-]{3,32}$`; otherwise the precompile reverts (an
/// invalid name is a programmer error, distinct from a well-formed but
/// unregistered one which returns zero).
pub fn payid_resolve_with<L: PayIdLookup + ?Sized>(
    input: &[u8],
    gas: u64,
    lookup: &L,
) -> Result<(Vec<u8>, u64), ZvmError> {
    // Minimum frame: 96 bytes (op-word + offset-word + length-word).
    if input.len() < 96 {
        return Err(ZvmError::InvalidInput(
            "payid: input < 96 bytes (need uint8 op + bytes header)".into(),
        ));
    }

    // op = input[31] (uint8 right-padded in 32-byte word). Reject any
    // non-zero byte in the upper 31 bytes — a defensive check that
    // catches callers that ABI-encoded a wider int by mistake.
    if input[..31].iter().any(|&b| b != 0) {
        return Err(ZvmError::InvalidInput("payid: op must fit in uint8".into()));
    }
    let op = input[31];

    // Length is the last 4 bytes of the 32-byte length word; payloads
    // are bounded to 32 chars (forward) or 20 bytes (reverse), so anything
    // beyond u32 is a malformed call.
    if input[64..92].iter().any(|&b| b != 0) {
        return Err(ZvmError::InvalidInput("payid: length too large".into()));
    }
    let length = u32::from_be_bytes([input[92], input[93], input[94], input[95]]) as usize;
    if length == 0 || input.len() < 96 + length {
        return Err(ZvmError::InvalidInput("payid: payload length out of range".into()));
    }
    let payload = &input[96..96 + length];

    let cost = 700u64.saturating_add(50u64.saturating_mul(payload.len() as u64));
    if gas < cost {
        return Err(ZvmError::OutOfGas);
    }

    match op {
        0 => {
            // Strip optional "@zbx" suffix, lowercase the leading name.
            let raw = std::str::from_utf8(payload)
                .map_err(|_| ZvmError::InvalidInput("payid: name not UTF-8".into()))?;
            let lowered = raw.to_ascii_lowercase();
            let name = lowered.trim_end_matches("@zbx");
            let name_bytes = name.as_bytes();
            if !zbx_types::payid::validate_payid_name(name_bytes) {
                return Err(ZvmError::InvalidInput(
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
                return Err(ZvmError::InvalidInput(
                    "payid: reverse-lookup payload must be exactly 20 bytes".into(),
                ));
            }
            let mut addr = [0u8; 20];
            addr.copy_from_slice(payload);
            let name = lookup.reverse(&addr).unwrap_or_default();
            // ABI-encode `string`: head offset (0x20) || length || data padded to 32.
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
        _ => Err(ZvmError::InvalidInput("payid: unknown op (expected 0 or 1)".into())),
    }
}

// Re-export the canonical trait so the interpreter can adapt its host
// without an extra import path.
pub use zbx_types::payid::PayIdLookup;

/// 0x0B: EIP-4844 KZG point-evaluation precompile.
///
/// Task #4 (replaces Pass-18 fail-closed body). Delegates to
/// [`zbx_crypto::kzg::do_kzg_point_eval`] which is the single source of
/// truth shared with `zbx-evm` so the two engines cannot drift on
/// consensus-critical pairing arithmetic. The trusted setup (only
/// `[s]·G2` is needed) is fetched from the process-wide `OnceLock`
/// installed by the node startup via
/// [`zbx_crypto::kzg::init_global_kzg_settings`]. If no setup has been
/// installed, the precompile **fail-closes** with `InvalidInput` — this
/// is intentional: a node that boots without the trusted setup must
/// not silently accept blob-aware contracts as no-ops.
fn kzg_verify(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), ZvmError> {
    let settings = zbx_crypto::kzg::global_kzg_settings().ok_or_else(|| {
        ZvmError::InvalidInput(
            "kzg (0x0B): trusted setup not initialised; call \
             zbx_crypto::kzg::init_global_kzg_settings at node startup"
                .into(),
        )
    })?;
    do_kzg_with_settings(input, gas, &settings)
}

/// Test/integration entry point — bypasses the global `OnceLock` so each
/// test can supply its own setup. Production callers go through
/// [`kzg_verify`] which reads the global. Both paths share the
/// `do_kzg_point_eval` implementation in `zbx-crypto`.
pub fn do_kzg_with_settings(
    input: &[u8],
    gas: u64,
    settings: &zbx_crypto::kzg::KzgSettings,
) -> Result<(Vec<u8>, u64), ZvmError> {
    use zbx_crypto::kzg::{do_kzg_point_eval, KzgError};
    match do_kzg_point_eval(input, gas, settings) {
        Ok(v) => Ok(v),
        Err(KzgError::OutOfGas { .. }) => Err(ZvmError::OutOfGas),
        Err(e) => Err(ZvmError::InvalidInput(format!("kzg: {}", e))),
    }
}

/// Task #5 (Precompile 0x0C): live price-oracle reader.
///
/// Stateful — must be called from the interpreter with a host adapter
/// that exposes `OracleStateReader::read_slot`. The shared layout +
/// gas schedule live in `zbx_crypto::oracle_state` so this body and
/// the EVM equivalent (`zbx_evm::precompiles::do_price_oracle`)
/// remain byte-identical.
///
/// **ABI (consensus-critical, byte-identical with EVM):**
///   * Input  — exactly 32 bytes: `keccak256(asset_symbol)`
///   * Output — exactly 64 bytes: `int256 price_e8 ‖ uint256 updated_at`
///   * Unknown asset: 64 zero bytes (no revert).
///   * Staleness: caller's responsibility (precompile NEVER reverts on stale).
///   * Gas: flat 1200 (1000 base + 100/slot × 2 slots).
pub fn price_oracle_with<R: zbx_crypto::oracle_state::OracleStateReader + ?Sized>(
    input: &[u8],
    gas: u64,
    reader: &R,
) -> Result<(Vec<u8>, u64), ZvmError> {
    use zbx_crypto::oracle_state::{do_price_oracle, OraclePrecompileError};
    match do_price_oracle(input, gas, reader) {
        Ok(v) => Ok(v),
        Err(OraclePrecompileError::OutOfGas) => Err(ZvmError::OutOfGas),
        Err(e) => Err(ZvmError::InvalidInput(e.to_string())),
    }
}

/// 0x0D: Ed25519 signature verification (Pass-18: real ed25519-dalek).
///
/// Input layout (96 bytes, all big-endian / network-order as ed25519-dalek
/// expects):
///   [0..32]   public key  (32 bytes, compressed Edwards Y)
///   [32..64]  message     (32 bytes — the digest the signer signed)
///   [64..128] signature   (64 bytes, R ‖ s)
///
/// Output: `0x...01` on valid, `0x...00` on invalid. Matches the standard
/// boolean-precompile convention used by ECRECOVER / BN128_PAIRING.
fn ed25519_verify(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), ZvmError> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let cost = 3000u64;
    if gas < cost { return Err(ZvmError::OutOfGas); }
    if input.len() < 128 {
        return Err(ZvmError::InvalidInput("ed25519_verify needs 128 bytes".into()));
    }

    let pk_bytes: [u8; 32] = input[0..32].try_into().unwrap();
    let msg = &input[32..64];
    let sig_bytes: [u8; 64] = input[64..128].try_into().unwrap();

    // Per EVM precompile convention, malformed inputs return false (32-byte zero)
    // rather than reverting. This matches ECRECOVER's behaviour and avoids
    // griefing contracts that accept user-supplied signatures.
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

/// 0x0E: VRF verification — RFC 9381 ECVRF-EDWARDS25519-SHA512-ELL2.
///
/// Input layout (variable length, minimum 112 bytes):
///   [0..32]                  Y         (32-byte compressed Ed25519 pubkey)
///   [32 .. len-80]           alpha     (variable-length input message)
///   [len-80 .. len]          pi        (80-byte VRF proof: Γ ‖ c:16 ‖ s:32)
///
/// Output:
///   * Valid proof   → 64-byte beta (the verifiable random output).
///   * Invalid proof → 32-byte zero (no revert; caller MUST branch on
///     `result.length == 64` or treat zero as "not random"). Matches the
///     ECRECOVER / BN128_PAIRING fail-soft convention.
///
/// Gas: flat 5000 (independent of alpha length — RFC 9381 hash cost is
/// dominated by 4× SHA-512 calls, two BigInt mod-p inversions for
/// Elligator2, and three Edwards-25519 scalar multiplications).
///
/// Backed by `zbx_crypto::vrf::ecvrf_edwards25519::verify`. The crate-level
/// honest-gap note re: RFC 9381 §A.4 vector reproduction also applies here:
/// the precompile is fail-closed, so any input that does not verify
/// produces a 32-byte-zero output — contracts MUST treat that as an
/// invalid beacon and fall back to a safe default.
fn vrf_verify(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), ZvmError> {
    use zbx_crypto::vrf::ecvrf_edwards25519::{verify, BETA_LEN, PROOF_LEN, PUBKEY_LEN};
    let cost = 5000u64;
    if gas < cost {
        return Err(ZvmError::OutOfGas);
    }
    let min_len = PUBKEY_LEN + PROOF_LEN; // 32 + 80 = 112
    if input.len() < min_len {
        // Fail-soft: too-short input → 32-byte zero (matches ECRECOVER).
        return Ok((vec![0u8; 32], cost));
    }
    let mut pk = [0u8; 32];
    pk.copy_from_slice(&input[0..PUBKEY_LEN]);
    let pi_start = input.len() - PROOF_LEN;
    let alpha = &input[PUBKEY_LEN..pi_start];
    let pi = &input[pi_start..];

    match verify(&pk, alpha, pi) {
        Some(beta) => {
            debug_assert_eq!(beta.len(), BETA_LEN);
            Ok((beta.to_vec(), cost))
        }
        // Fail-soft on any verification error.
        None => Ok((vec![0u8; 32], cost)),
    }
}

/// 0x0F: ZUSD balance — superseded by Task #7 (`zusd_vault_with`). Kept
/// only so the dispatcher arm stays callable from existing call sites
/// that haven't migrated; the production interpreter intercepts 0x0F
/// before this function is reached.
#[allow(dead_code)]
fn zusd_balance(input: &[u8], gas: u64) -> Result<(Vec<u8>, u64), ZvmError> {
    let cost = 100u64;
    if gas < cost { return Err(ZvmError::OutOfGas); }
    if input.len() < 32 { return Err(ZvmError::InvalidInput("zusd_balance needs 32 bytes".into())); }
    Err(ZvmError::InvalidInput(
        "zusd_balance precompile superseded by Task #7 zusd_vault_with".into(),
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// Task #7 (Precompile 0x0F — ZUSD vault state direct-read).
//
// Stateful precompile. Intercepted by the interpreter (see
// `interpreter.rs`); the standalone dispatcher fails-closed for safety.
// The body lives in `zbx_crypto::vault_state::do_zusd_vault_read` so
// the EVM and ZVM execution engines stay byte-identical.
// ─────────────────────────────────────────────────────────────────────────────

/// ZVM-side wrapper around `zbx_crypto::vault_state::do_zusd_vault_read`.
/// Maps shared errors to ZVM error variants; the host adapter is built
/// at the interpreter intercept site to bridge `ZvmHost::storage_load`
/// to `OracleStateReader::read_slot`.
pub fn zusd_vault_with<R: zbx_crypto::vault_state::VaultStateReader + ?Sized>(
    input: &[u8],
    gas_limit: u64,
    reader: &R,
) -> Result<(Vec<u8>, u64), ZvmError> {
    use zbx_crypto::vault_state::{do_zusd_vault_read, VaultPrecompileError};
    do_zusd_vault_read(input, gas_limit, reader).map_err(|e| match e {
        VaultPrecompileError::OutOfGas => ZvmError::OutOfGas,
        VaultPrecompileError::BadInputLength { got } => {
            ZvmError::InvalidInput(format!("zusd_vault: input must be 32 bytes, got {got}"))
        }
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ripemd160_known_vector() {
        // RIPEMD-160("") = 9c1185a5c5e9fc54612808977ee8f548b2258d31
        let (out, _) = ripemd160_hash(&[], 10_000).unwrap();
        assert_eq!(out.len(), 32);
        assert_eq!(
            &out[12..],
            &hex::decode("9c1185a5c5e9fc54612808977ee8f548b2258d31").unwrap()[..]
        );
        // Pre-Pass-18: this returned `Err(InvalidInput)` (fail-closed stub).
    }

    #[test]
    fn modexp_basic() {
        // 3^2 mod 5 = 4
        let mut inp = [0u8; 96 + 3];
        inp[31] = 1;    // base_len = 1
        inp[63] = 1;    // exp_len  = 1
        inp[95] = 1;    // mod_len  = 1
        inp[96] = 3;    // base = 3
        inp[97] = 2;    // exp  = 2
        inp[98] = 5;    // mod  = 5
        let (out, _) = modexp(&inp, 1_000_000).unwrap();
        assert_eq!(out, vec![4u8]);
    }

    #[test]
    fn modexp_zero_modulus_returns_zero_padded() {
        let mut inp = [0u8; 96 + 3];
        inp[31] = 1;
        inp[63] = 1;
        inp[95] = 1;
        inp[96] = 7;
        inp[97] = 5;
        inp[98] = 0; // mod = 0
        let (out, _) = modexp(&inp, 1_000_000).unwrap();
        assert_eq!(out, vec![0u8]);
    }

    #[test]
    fn bn128_add_generator_plus_identity() {
        // G1 generator (1, 2) + identity (0, 0) = generator (1, 2)
        let mut input = [0u8; 128];
        input[31] = 1;
        input[63] = 2;
        let (out, cost) = bn128_add(&input, 10_000).unwrap();
        assert_eq!(out.len(), 64);
        assert_eq!(cost, 150);
        assert_eq!(out[31], 1);
        assert_eq!(out[63], 2);
    }

    #[test]
    fn bn128_mul_zero_scalar_yields_identity() {
        let mut input = [0u8; 96];
        input[31] = 1;
        input[63] = 2;
        let (out, _) = bn128_mul(&input, 100_000).unwrap();
        assert_eq!(out, vec![0u8; 64]);
    }

    #[test]
    fn bn128_pairing_empty_input_passes() {
        let (out, cost) = bn128_pairing(&[], 1_000_000).unwrap();
        assert_eq!(out.len(), 32);
        assert_eq!(out[31], 1);
        assert_eq!(cost, 45_000);
    }

    // NOTE (Architect-review Pass-18 LOW deferred): a positive-path test
    // for `bn128_pairing` with a non-trivial valid pair is intentionally
    // omitted from this lib-test suite — it requires hex-encoded G2
    // generator coordinates whose Fq2 component endianness must exactly
    // match Ethereum's `(imaginary, real)` convention, and getting a
    // wrong vector through review would be worse than no positive test.
    // The end-to-end positive-path coverage is provided upstream by
    // `zbx-evm/tests/precompiles_*.rs` (these bodies are byte-identical
    // ports — see consensus-equivalence sign-off in Pass-18 architect
    // review) and by Pass-17 Groth16 oracle verifier tests which
    // exercise BN254 pairings end-to-end via the real `arkworks` stack.

    #[test]
    fn bn128_pairing_bad_length_rejected() {
        let bad = vec![0u8; 100]; // not a multiple of 192
        assert!(bn128_pairing(&bad, 1_000_000).is_err());
    }

    #[test]
    fn blake2f_basic_compression() {
        let mut input = [0u8; 213];
        input[3] = 12; // rounds = 12
        let (out, cost) = blake2f(&input, 1_000).unwrap();
        assert_eq!(out.len(), 64);
        assert_eq!(cost, 12);
        // Pre-Pass-18: this returned `Err(InvalidInput)`.
    }

    #[test]
    fn blake2f_rejects_bad_length() {
        assert!(blake2f(&[0u8; 100], 1_000).is_err());
    }

    #[test]
    fn blake2f_rejects_invalid_final_flag() {
        let mut input = [0u8; 213];
        input[212] = 2;
        assert!(blake2f(&input, 1_000).is_err());
    }

    #[test]
    fn ed25519_verify_real_signature_roundtrip() {
        use ed25519_dalek::{Signer, SigningKey};
        // Deterministic test key — never use this seed for anything real.
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let vk = sk.verifying_key();
        let msg = [0xABu8; 32];
        let sig = sk.sign(&msg);

        let mut input = [0u8; 128];
        input[0..32].copy_from_slice(vk.as_bytes());
        input[32..64].copy_from_slice(&msg);
        input[64..128].copy_from_slice(&sig.to_bytes());

        let (out, cost) = ed25519_verify(&input, 10_000).unwrap();
        assert_eq!(cost, 3000);
        assert_eq!(out.len(), 32);
        assert_eq!(out[31], 1, "valid Ed25519 sig should return 0x...01");
        // Pre-Pass-18: this returned `Err(InvalidInput)` regardless of input.
    }

    #[test]
    fn ed25519_verify_tampered_message_rejected() {
        use ed25519_dalek::{Signer, SigningKey};
        let sk = SigningKey::from_bytes(&[9u8; 32]);
        let vk = sk.verifying_key();
        let msg = [0x01u8; 32];
        let sig = sk.sign(&msg);

        let mut input = [0u8; 128];
        input[0..32].copy_from_slice(vk.as_bytes());
        // Tamper one byte of the message after signing.
        let mut bad_msg = msg;
        bad_msg[0] ^= 0xFF;
        input[32..64].copy_from_slice(&bad_msg);
        input[64..128].copy_from_slice(&sig.to_bytes());

        let (out, _) = ed25519_verify(&input, 10_000).unwrap();
        assert_eq!(out, vec![0u8; 32], "tampered msg must return 0x...00");
    }

    #[test]
    fn ed25519_verify_garbage_pubkey_returns_zero_not_revert() {
        // Convention check: malformed pubkey returns `false` (zeros), does
        // not revert — matches ECRECOVER. Forging a 32-byte string that
        // isn't a valid Edwards-Y compressed point is trivial.
        let mut input = [0xFFu8; 128];
        // Ensure pubkey bytes are unlikely to decode (high bit set + invalid x).
        let (out, _) = ed25519_verify(&input[..], 10_000).unwrap();
        assert_eq!(out.len(), 32);
        // Either decoded as a junk valid point and verify failed, or decode
        // failed → both paths must return 32 zero bytes.
        assert_eq!(out, vec![0u8; 32]);
        let _ = &mut input; // silence unused_mut on some clippy configs
    }

    // ─── Task #3 (Precompile 0x0A — PayID resolution) tests ─────────────

    /// In-memory `PayIdLookup` for unit-testing the precompile body.
    #[derive(Default)]
    struct MockLookup {
        forward: std::collections::HashMap<Vec<u8>, [u8; 20]>,
        reverse: std::collections::HashMap<[u8; 20], Vec<u8>>,
    }
    impl PayIdLookup for MockLookup {
        fn resolve(&self, name: &[u8]) -> Option<[u8; 20]> {
            self.forward.get(name).copied()
        }
        fn reverse(&self, addr: &[u8; 20]) -> Option<Vec<u8>> {
            self.reverse.get(addr).cloned()
        }
    }

    /// Build a standard `(uint8 op, bytes payload)` ABI input.
    fn abi_op_bytes(op: u8, payload: &[u8]) -> Vec<u8> {
        let len = payload.len();
        let padded = (len + 31) / 32 * 32;
        let mut out = vec![0u8; 96 + padded];
        out[31] = op;
        out[63] = 0x40; // head offset to bytes block
        out[60..64][3] = 0x40; // (already set, but explicit)
        out[92..96].copy_from_slice(&(len as u32).to_be_bytes());
        out[96..96 + len].copy_from_slice(payload);
        out
    }

    #[test]
    fn payid_resolves_known_name_to_address() {
        let mut lookup = MockLookup::default();
        let want = [0xAB; 20];
        lookup.forward.insert(b"alice".to_vec(), want);
        let inp = abi_op_bytes(0, b"alice");
        let (out, cost) = payid_resolve_with(&inp, 100_000, &lookup).unwrap();
        assert_eq!(out.len(), 32);
        assert_eq!(&out[12..32], &want, "address must be right-aligned in 32-byte word");
        assert_eq!(out[..12], [0u8; 12]);
        assert_eq!(cost, 700 + 50 * 5);
    }

    #[test]
    fn payid_strips_at_zbx_suffix_before_lookup() {
        let mut lookup = MockLookup::default();
        let want = [0xCD; 20];
        lookup.forward.insert(b"bob".to_vec(), want);
        let inp = abi_op_bytes(0, b"BOB@zbx"); // mixed-case + suffix
        let (out, _) = payid_resolve_with(&inp, 100_000, &lookup).unwrap();
        assert_eq!(&out[12..32], &want);
    }

    #[test]
    fn payid_unregistered_returns_zero_address_not_revert() {
        let lookup = MockLookup::default();
        let inp = abi_op_bytes(0, b"ghost");
        let (out, _) = payid_resolve_with(&inp, 100_000, &lookup).unwrap();
        assert_eq!(out, vec![0u8; 32], "unregistered must be address(0), not revert");
    }

    #[test]
    fn payid_malformed_name_reverts() {
        let lookup = MockLookup::default();
        // Space violates [a-z0-9._-] (and survives `to_ascii_lowercase`).
        let inp = abi_op_bytes(0, b"al ce");
        assert!(payid_resolve_with(&inp, 100_000, &lookup).is_err());
        // Too-short name (2 chars).
        let inp = abi_op_bytes(0, b"al");
        assert!(payid_resolve_with(&inp, 100_000, &lookup).is_err());
        // Too-long name (33 chars).
        let inp = abi_op_bytes(0, &[b'a'; 33]);
        assert!(payid_resolve_with(&inp, 100_000, &lookup).is_err());
    }

    #[test]
    fn payid_gas_exhaustion_returns_oog() {
        let lookup = MockLookup::default();
        let inp = abi_op_bytes(0, b"alice");
        // Cost = 700 + 50*5 = 950; supply 700.
        let err = payid_resolve_with(&inp, 700, &lookup).unwrap_err();
        assert!(matches!(err, ZvmError::OutOfGas));
    }

    #[test]
    fn payid_reverse_known_address() {
        let mut lookup = MockLookup::default();
        let addr = [0x42u8; 20];
        lookup.reverse.insert(addr, b"alice".to_vec());
        let inp = abi_op_bytes(1, &addr);
        let (out, _) = payid_resolve_with(&inp, 100_000, &lookup).unwrap();
        // ABI string layout: head=0x20, len=5, "alice" padded to 32.
        assert_eq!(out.len(), 64 + 32);
        assert_eq!(out[31], 0x20);
        assert_eq!(&out[60..64], &[0, 0, 0, 5]);
        assert_eq!(&out[64..69], b"alice");
        assert!(out[69..96].iter().all(|&b| b == 0));
    }

    #[test]
    fn payid_reverse_unregistered_returns_empty_string() {
        let lookup = MockLookup::default();
        let addr = [0u8; 20];
        let inp = abi_op_bytes(1, &addr);
        let (out, _) = payid_resolve_with(&inp, 100_000, &lookup).unwrap();
        // ABI empty string: head=0x20, len=0, no data.
        assert_eq!(out.len(), 64);
        assert_eq!(out[31], 0x20);
        assert_eq!(&out[60..64], &[0, 0, 0, 0]);
    }

    #[test]
    fn payid_unknown_op_reverts() {
        let lookup = MockLookup::default();
        let inp = abi_op_bytes(2, b"alice");
        assert!(payid_resolve_with(&inp, 100_000, &lookup).is_err());
    }

    #[test]
    fn payid_decoder_ignores_head_offset_value() {
        // Architect-review follow-up: the decoder reads fixed positions and
        // intentionally ignores the head-offset word. Verify a non-canonical
        // offset (anything other than 0x40) still yields the same answer.
        let mut lookup = MockLookup::default();
        let want = [0x9Au8; 20];
        lookup.forward.insert(b"alice".to_vec(), want);
        let canon = abi_op_bytes(0, b"alice");
        let mut weird = canon.clone();
        for b in weird[32..64].iter_mut() { *b = 0xFF; }
        let (a, _) = payid_resolve_with(&canon, 100_000, &lookup).unwrap();
        let (b, _) = payid_resolve_with(&weird, 100_000, &lookup).unwrap();
        assert_eq!(a, b, "decoder must ignore head-offset value");
        assert_eq!(&a[12..32], &want);
    }

    #[test]
    fn payid_reverse_handles_full_32_byte_name_without_terminator() {
        // Architect-review follow-up: production host parses reverse names
        // via `word.iter().position(|&b| b == 0).unwrap_or(32)` — verify a
        // 32-char (no-terminator) name round-trips through the precompile
        // ABI-encoder without truncation.
        let mut lookup = MockLookup::default();
        let addr = [0x55u8; 20];
        let name32 = vec![b'a'; 32];
        lookup.reverse.insert(addr, name32.clone());
        let inp = abi_op_bytes(1, &addr);
        let (out, _) = payid_resolve_with(&inp, 100_000, &lookup).unwrap();
        assert_eq!(out.len(), 64 + 32);
        assert_eq!(&out[60..64], &[0, 0, 0, 32]);
        assert_eq!(&out[64..96], &name32[..]);
    }

    #[test]
    fn payid_reverse_payload_must_be_20_bytes() {
        let lookup = MockLookup::default();
        let inp = abi_op_bytes(1, b"too short");
        assert!(payid_resolve_with(&inp, 100_000, &lookup).is_err());
    }

    /// Cross-VM consensus equivalence: feed the EVM body and the ZVM body
    /// the same input + same lookup, assert byte-identical output.
    #[test]
    fn payid_zvm_evm_consensus_equivalence_forward_and_reverse() {
        struct DualLookup {
            f: std::collections::HashMap<Vec<u8>, [u8; 20]>,
            r: std::collections::HashMap<[u8; 20], Vec<u8>>,
        }
        impl PayIdLookup for DualLookup {
            fn resolve(&self, n: &[u8]) -> Option<[u8; 20]> { self.f.get(n).copied() }
            fn reverse(&self, a: &[u8; 20]) -> Option<Vec<u8>> { self.r.get(a).cloned() }
        }
        // `zbx_evm::precompiles::PayIdLookup` is a re-export of the same
        // `zbx_types::payid::PayIdLookup` trait, so the single impl above
        // satisfies both bounds.
        let addr = [0x77u8; 20];
        let mut lookup = DualLookup { f: Default::default(), r: Default::default() };
        lookup.f.insert(b"alice".to_vec(), addr);
        lookup.r.insert(addr, b"alice".to_vec());

        for input in [
            abi_op_bytes(0, b"alice"),
            abi_op_bytes(0, b"ghost"),       // unregistered → zero
            abi_op_bytes(0, b"BOB@zbx"),     // suffix + case
            abi_op_bytes(1, &addr),          // reverse hit
            abi_op_bytes(1, &[0u8; 20]),     // reverse miss
        ] {
            let z = payid_resolve_with(&input, 100_000, &lookup).unwrap();
            let e = zbx_evm::precompiles::do_payid(&input, 100_000, &lookup).unwrap();
            assert_eq!(z, e, "ZVM and EVM precompile bytes must match");
        }
    }

    #[test]
    fn precompile_dispatcher_routes_correctly() {
        // 0x03 (RIPEMD-160) used to fail-closed; now succeeds.
        let mut addr = [0u8; 20];
        addr[19] = 0x03;
        let (out, _) = call_precompile(&addr, &[], 10_000).unwrap();
        assert_eq!(out.len(), 32);

        // 0x09 (BLAKE2F) used to fail-closed; now succeeds.
        addr[19] = 0x09;
        let mut blake_in = [0u8; 213];
        blake_in[3] = 1;
        let (out, _) = call_precompile(&addr, &blake_in, 10_000).unwrap();
        assert_eq!(out.len(), 64);
    }
}
