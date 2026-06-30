//! EVM 256-bit word stack (max 1024 items) and U256 arithmetic helpers.

use crate::error::EvmError;

const MAX_STACK: usize = 1024;

pub struct Stack {
    data: Vec<[u8; 32]>,
}

impl Stack {
    pub fn new() -> Self { Stack { data: Vec::with_capacity(16) } }

    pub fn len(&self) -> usize { self.data.len() }

    pub fn push(&mut self, val: [u8; 32]) -> Result<(), EvmError> {
        if self.data.len() >= MAX_STACK {
            return Err(EvmError::StackOverflow);
        }
        self.data.push(val);
        Ok(())
    }

    pub fn pop(&mut self) -> Result<[u8; 32], EvmError> {
        self.data.pop().ok_or(EvmError::StackUnderflow)
    }

    pub fn peek(&self, depth: usize) -> Result<&[u8; 32], EvmError> {
        let len = self.data.len();
        if depth >= len { return Err(EvmError::StackUnderflow); }
        Ok(&self.data[len - 1 - depth])
    }

    pub fn swap(&mut self, depth: usize) -> Result<(), EvmError> {
        let len = self.data.len();
        if depth >= len { return Err(EvmError::StackUnderflow); }
        let top = len - 1;
        self.data.swap(top, top - depth);
        Ok(())
    }

    pub fn dup(&mut self, depth: usize) -> Result<(), EvmError> {
        let val = *self.peek(depth - 1)?;
        self.push(val)
    }
}

// ---------------------------------------------------------------------------
// Internal limb helpers (little-endian u64 limbs for arithmetic)
// ---------------------------------------------------------------------------

/// Convert big-endian [u8;32] to four u64 limbs, little-endian limb order.
/// limbs[0] = least-significant 64 bits, limbs[3] = most-significant 64 bits.
#[inline]
fn to_le_limbs(a: &[u8; 32]) -> [u64; 4] {
    [
        u64::from_be_bytes(a[24..32].try_into().unwrap()),
        u64::from_be_bytes(a[16..24].try_into().unwrap()),
        u64::from_be_bytes(a[8..16].try_into().unwrap()),
        u64::from_be_bytes(a[0..8].try_into().unwrap()),
    ]
}

/// Convert four little-endian u64 limbs back to big-endian [u8;32].
#[inline]
fn from_le_limbs(limbs: [u64; 4]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[0..8].copy_from_slice(&limbs[3].to_be_bytes());
    out[8..16].copy_from_slice(&limbs[2].to_be_bytes());
    out[16..24].copy_from_slice(&limbs[1].to_be_bytes());
    out[24..32].copy_from_slice(&limbs[0].to_be_bytes());
    out
}

/// True if the most-significant bit is set (negative in EVM two's complement).
#[inline]
fn is_negative(a: &[u8; 32]) -> bool { a[0] & 0x80 != 0 }

/// Two's complement negation of a U256.
#[inline]
fn negate(a: &[u8; 32]) -> [u8; 32] {
    let not = u256_not(a);
    u256_add(&not, &u256_from_u64(1))
}

// ---------------------------------------------------------------------------
// U256 arithmetic helpers (big-endian 32-byte arrays)
// ---------------------------------------------------------------------------

pub fn u256_add(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut result = [0u8; 32];
    let mut carry: u16 = 0;
    for i in (0..32).rev() {
        let sum = a[i] as u16 + b[i] as u16 + carry;
        result[i] = sum as u8;
        carry = sum >> 8;
    }
    result
}

pub fn u256_sub(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut result = [0u8; 32];
    let mut borrow: i16 = 0;
    for i in (0..32).rev() {
        let diff = a[i] as i16 - b[i] as i16 - borrow;
        result[i] = diff as u8;
        borrow = if diff < 0 { 1 } else { 0 };
    }
    result
}

/// 256-bit multiply, result truncated to 256 bits (mod 2^256).
pub fn u256_mul(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let la = to_le_limbs(a);
    let lb = to_le_limbs(b);
    let mut result = [0u64; 4];
    for i in 0..4 {
        let mut carry: u64 = 0;
        for j in 0..4 {
            if i + j >= 4 { break; }
            let prod = la[i] as u128 * lb[j] as u128
                + result[i + j] as u128
                + carry as u128;
            result[i + j] = prod as u64;
            carry = (prod >> 64) as u64;
        }
    }
    from_le_limbs(result)
}

/// Unsigned 256-bit division. Returns 0 if divisor is zero (EVM spec).
pub fn u256_div(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    if u256_is_zero(b) { return [0u8; 32]; }
    // Simple long-division on big-endian bit representation.
    let mut quotient  = [0u8; 32];
    let mut remainder = [0u8; 32];
    for bit_idx in 0..256usize {
        // Shift remainder left 1, bring in MSB of `a`.
        remainder = u256_shl_one(&remainder);
        let a_bit = (a[bit_idx / 8] >> (7 - bit_idx % 8)) & 1;
        remainder[31] |= a_bit;
        if !u256_lt(&remainder, b) {
            remainder = u256_sub(&remainder, b);
            quotient[bit_idx / 8] |= 1 << (7 - bit_idx % 8);
        }
    }
    quotient
}

/// Shift left by exactly 1 bit (internal helper for long division).
#[inline]
fn u256_shl_one(a: &[u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let mut carry = 0u8;
    for i in (0..32).rev() {
        out[i] = (a[i] << 1) | carry;
        carry = a[i] >> 7;
    }
    out
}

/// Signed 256-bit division (EVM SDIV). Returns 0 if divisor is zero.
/// Special case: -2^255 / -1 = -2^255 (overflow, per Yellow Paper).
pub fn u256_sdiv(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    if u256_is_zero(b) { return [0u8; 32]; }
    let neg_a = is_negative(a);
    let neg_b = is_negative(b);
    // -2^255 / -1 = -2^255 (overflow — return as-is).
    let min_int = {
        let mut m = [0u8; 32];
        m[0] = 0x80;
        m
    };
    if a == &min_int && b == &[0u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1] {
        return min_int;
    }
    let abs_a = if neg_a { negate(a) } else { *a };
    let abs_b = if neg_b { negate(b) } else { *b };
    let q = u256_div(&abs_a, &abs_b);
    if neg_a != neg_b { negate(&q) } else { q }
}

/// Unsigned 256-bit modulo. Returns 0 if divisor is zero (EVM spec).
pub fn u256_mod(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    if u256_is_zero(b) { return [0u8; 32]; }
    let mut remainder = [0u8; 32];
    for bit_idx in 0..256usize {
        remainder = u256_shl_one(&remainder);
        let a_bit = (a[bit_idx / 8] >> (7 - bit_idx % 8)) & 1;
        remainder[31] |= a_bit;
        if !u256_lt(&remainder, b) {
            remainder = u256_sub(&remainder, b);
        }
    }
    remainder
}

/// Signed 256-bit modulo. Result has the sign of the dividend. Returns 0 if divisor is 0.
pub fn u256_smod(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    if u256_is_zero(b) { return [0u8; 32]; }
    let neg_a = is_negative(a);
    let neg_b = is_negative(b);
    let abs_a = if neg_a { negate(a) } else { *a };
    let abs_b = if neg_b { negate(b) } else { *b };
    let r = u256_mod(&abs_a, &abs_b);
    if neg_a && !u256_is_zero(&r) { negate(&r) } else { r }
}

/// (a + b) % N, computed with 512-bit intermediate precision. Returns 0 if N == 0.
pub fn u256_addmod(a: &[u8; 32], b: &[u8; 32], n: &[u8; 32]) -> [u8; 32] {
    if u256_is_zero(n) { return [0u8; 32]; }
    // Use 512-bit add: carry from u256_add indicates the +2^256 term.
    let mut carry: u16 = 0;
    let mut sum = [0u8; 32];
    for i in (0..32).rev() {
        let s = a[i] as u16 + b[i] as u16 + carry;
        sum[i] = s as u8;
        carry = s >> 8;
    }
    // If carry, sum = 2^256 + sum. Reduce mod n.
    // 2^256 mod n = (2^256 - n) mod n (since 2^256 = n * floor(2^256/n) + rem).
    // Simpler: if carry == 1, compute (sum % n) after subtracting n once if possible,
    // accounting for the extra 2^256. We handle this by doing the mod on a 512-bit
    // value represented as (carry_bit, sum_256).
    if carry == 1 {
        // sum + 2^256 mod n: apply one extra reduction via the full 512-bit path.
        // For correctness we use the bit-by-bit long division approach on 33 bytes.
        let mut rem = [0u8; 32];
        // Process the carry bit first.
        rem = u256_shl_one(&rem);
        rem[31] |= 1; // the carry bit is bit 256 (MSB of the 257-bit number)
        if !u256_lt(&rem, n) { rem = u256_sub(&rem, n); }
        // Then process the remaining 256 bits of `sum`.
        for bit_idx in 0..256usize {
            rem = u256_shl_one(&rem);
            let b_bit = (sum[bit_idx / 8] >> (7 - bit_idx % 8)) & 1;
            rem[31] |= b_bit;
            if !u256_lt(&rem, n) { rem = u256_sub(&rem, n); }
        }
        return rem;
    }
    u256_mod(&sum, n)
}

/// (a * b) % N, computed with full 512-bit precision. Returns 0 if N == 0.
pub fn u256_mulmod(a: &[u8; 32], b: &[u8; 32], n: &[u8; 32]) -> [u8; 32] {
    if u256_is_zero(n) { return [0u8; 32]; }
    // Compute a * b mod n bit-by-bit using a running accumulator.
    // acc = 0; for each bit of b (MSB first): acc = (acc * 2) % n; if bit: acc = (acc + a) % n
    let mut acc = [0u8; 32];
    for bit_idx in 0..256usize {
        acc = u256_addmod(&acc, &acc, n);
        let b_bit = (b[bit_idx / 8] >> (7 - bit_idx % 8)) & 1;
        if b_bit == 1 {
            acc = u256_addmod(&acc, a, n);
        }
    }
    acc
}

/// EVM EXP: base^exponent mod 2^256. Returns 1 if exponent is 0 (per spec).
pub fn u256_exp(base: &[u8; 32], exp: &[u8; 32]) -> [u8; 32] {
    if u256_is_zero(exp) { return u256_from_u64(1); }
    let mut result = u256_from_u64(1);
    let mut b = *base;
    let mut e = *exp;
    while !u256_is_zero(&e) {
        if e[31] & 1 == 1 {
            result = u256_mul(&result, &b);
        }
        b = u256_mul(&b, &b);
        // e >>= 1 (logical right shift)
        let mut new_e = [0u8; 32];
        let mut carry = 0u8;
        for i in 0..32 {
            new_e[i] = (e[i] >> 1) | carry;
            carry = e[i] << 7;
        }
        e = new_e;
    }
    result
}

/// Number of significant bytes in a U256 (for EXP gas calculation).
/// Returns 0 if value is zero.
pub fn u256_byte_len(a: &[u8; 32]) -> u64 {
    for i in 0..32 {
        if a[i] != 0 { return (32 - i) as u64; }
    }
    0
}

/// EVM SIGNEXTEND: sign-extend the `b`-th byte (0 = LSB) of `x`.
pub fn u256_signextend(b: &[u8; 32], x: &[u8; 32]) -> [u8; 32] {
    let b_val = u256_to_u64(b) as usize;
    if b_val >= 31 { return *x; }
    let byte_index = 31 - b_val; // index in big-endian array
    let sign_bit = (x[byte_index] & 0x80) != 0;
    let mut out = *x;
    // Mask upper bytes.
    for i in 0..byte_index {
        out[i] = if sign_bit { 0xFF } else { 0x00 };
    }
    // Mask upper bits of the sign byte.
    if sign_bit {
        out[byte_index] |= 0xFF; // already set by mask above — no-op
    } else {
        out[byte_index] &= 0x7F;
    }
    out
}

pub fn u256_lt(a: &[u8; 32], b: &[u8; 32]) -> bool { a < b }
pub fn u256_eq(a: &[u8; 32], b: &[u8; 32]) -> bool { a == b }
pub fn u256_is_zero(a: &[u8; 32]) -> bool { *a == [0u8; 32] }

/// GT: a > b (unsigned).
pub fn u256_gt(a: &[u8; 32], b: &[u8; 32]) -> bool { a > b }

/// SLT: a < b (signed two's complement).
pub fn u256_slt(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let na = is_negative(a);
    let nb = is_negative(b);
    if na != nb { return na; } // negative < positive
    a < b
}

/// SGT: a > b (signed two's complement).
pub fn u256_sgt(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let na = is_negative(a);
    let nb = is_negative(b);
    if na != nb { return nb; } // positive > negative
    a > b
}

/// Bitwise AND.
pub fn u256_and(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for i in 0..32 { out[i] = a[i] & b[i]; }
    out
}

/// Bitwise OR.
pub fn u256_or(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for i in 0..32 { out[i] = a[i] | b[i]; }
    out
}

/// Bitwise XOR.
pub fn u256_xor(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for i in 0..32 { out[i] = a[i] ^ b[i]; }
    out
}

/// Bitwise NOT.
pub fn u256_not(a: &[u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for i in 0..32 { out[i] = !a[i]; }
    out
}

/// EVM BYTE: extract byte `i` from `x` (0 = MSB). Returns 0 if i >= 32.
pub fn u256_byte(i: &[u8; 32], x: &[u8; 32]) -> [u8; 32] {
    let idx = u256_to_u64(i) as usize;
    if idx >= 32 { return [0u8; 32]; }
    u256_from_u64(x[idx] as u64)
}

/// SHL: logical left shift by `shift` bits (shift >= 256 → 0).
pub fn u256_shl(shift: &[u8; 32], val: &[u8; 32]) -> [u8; 32] {
    let s = u256_to_u64(shift) as usize;
    if s >= 256 { return [0u8; 32]; }
    if s == 0   { return *val; }
    let byte_shift = s / 8;
    let bit_shift  = s % 8;
    let mut out = [0u8; 32];
    for i in 0..32 {
        let src = i + byte_shift;
        if src < 32 {
            out[i] = val[src] << bit_shift;
            if bit_shift > 0 && src + 1 < 32 {
                out[i] |= val[src + 1] >> (8 - bit_shift);
            }
        }
    }
    out
}

/// SHR: logical right shift by `shift` bits (shift >= 256 → 0).
pub fn u256_shr(shift: &[u8; 32], val: &[u8; 32]) -> [u8; 32] {
    let s = u256_to_u64(shift) as usize;
    if s >= 256 { return [0u8; 32]; }
    if s == 0   { return *val; }
    let byte_shift = s / 8;
    let bit_shift  = s % 8;
    let mut out = [0u8; 32];
    for i in (0..32).rev() {
        let src = i as isize - byte_shift as isize;
        if src >= 0 {
            out[i] = val[src as usize] >> bit_shift;
            if bit_shift > 0 && src > 0 {
                out[i] |= val[(src - 1) as usize] << (8 - bit_shift);
            }
        }
    }
    out
}

/// SAR: arithmetic right shift (sign-extending). shift >= 256 → 0 or -1.
pub fn u256_sar(shift: &[u8; 32], val: &[u8; 32]) -> [u8; 32] {
    let sign = is_negative(val);
    let s = u256_to_u64(shift) as usize;
    if s >= 256 {
        return if sign { [0xFFu8; 32] } else { [0u8; 32] };
    }
    if s == 0 { return *val; }
    let mut out = u256_shr(shift, val);
    if sign {
        // Fill upper `s` bits with 1.
        for bit in 0..s {
            let idx = bit / 8;
            out[idx] |= 0x80u8 >> (bit % 8);
        }
    }
    out
}

pub fn u256_from_u64(v: u64) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[24..].copy_from_slice(&v.to_be_bytes());
    out
}

pub fn u256_to_u64(v: &[u8; 32]) -> u64 {
    u64::from_be_bytes(v[24..].try_into().unwrap_or([0u8; 8]))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn w(v: u64) -> [u8; 32] { u256_from_u64(v) }

    #[test]
    fn mul_basic() {
        assert_eq!(u256_mul(&w(6), &w(7)), w(42));
        assert_eq!(u256_mul(&w(0), &w(999)), w(0));
        assert_eq!(u256_mul(&w(u64::MAX), &w(1)), w(u64::MAX));
    }

    #[test]
    fn div_basic() {
        assert_eq!(u256_div(&w(100), &w(7)), w(14));
        assert_eq!(u256_div(&w(42), &w(0)), w(0)); // div by zero → 0
    }

    #[test]
    fn mod_basic() {
        assert_eq!(u256_mod(&w(100), &w(7)), w(2));
        assert_eq!(u256_mod(&w(42), &w(0)), w(0)); // mod by zero → 0
    }

    #[test]
    fn exp_basic() {
        assert_eq!(u256_exp(&w(2), &w(10)), w(1024));
        assert_eq!(u256_exp(&w(5), &w(0)),  w(1));    // x^0 = 1
        assert_eq!(u256_exp(&w(0), &w(5)),  w(0));    // 0^n = 0
    }

    #[test]
    fn bitwise_ops() {
        assert_eq!(u256_and(&w(0b1010), &w(0b1100)), w(0b1000));
        assert_eq!(u256_or (&w(0b1010), &w(0b1100)), w(0b1110));
        assert_eq!(u256_xor(&w(0b1010), &w(0b1100)), w(0b0110));
        let not_zero = u256_not(&w(0));
        assert_eq!(not_zero, [0xFFu8; 32]);
    }

    #[test]
    fn shl_shr() {
        assert_eq!(u256_shl(&w(1), &w(1)), w(2));
        assert_eq!(u256_shr(&w(1), &w(2)), w(1));
        assert_eq!(u256_shl(&w(256), &w(1)), w(0)); // shift ≥ 256 → 0
    }

    #[test]
    fn slt_sgt() {
        // -1 < 0 signed
        assert!(u256_slt(&[0xFFu8; 32], &w(0)));
        // 0 > -1 signed
        assert!(u256_sgt(&w(0), &[0xFFu8; 32]));
        // unsigned compare: large number is positive in unsigned
        assert!(u256_lt(&w(0), &w(1)));
    }

    #[test]
    fn signextend_basic() {
        // Sign-extend byte 0 of 0xFF → all 0xFF
        let x = {
            let mut v = [0u8; 32];
            v[31] = 0xFF;
            v
        };
        let result = u256_signextend(&w(0), &x);
        assert_eq!(result, [0xFFu8; 32]);
    }
}
