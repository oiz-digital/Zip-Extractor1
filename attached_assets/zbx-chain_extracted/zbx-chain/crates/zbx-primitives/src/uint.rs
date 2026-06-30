//! 256-bit unsigned integer — LE 64-bit limb representation.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub struct U256(pub [u64; 4]);

impl U256 {
    pub const ZERO: Self = Self([0, 0, 0, 0]);
    pub const ONE:  Self = Self([1, 0, 0, 0]);
    pub const MAX:  Self = Self([u64::MAX; 4]);

    pub fn from_u64(v: u64)   -> Self { Self([v, 0, 0, 0]) }
    pub fn from_u128(v: u128) -> Self { Self([v as u64, (v >> 64) as u64, 0, 0]) }
    pub fn from(v: u64)       -> Self { Self::from_u64(v) }

    pub fn is_zero(&self)  -> bool  { self.0 == [0; 4] }
    /// ⚠️ Returns ONLY the low 64 bits. Silently drops upper 192 bits.
    /// Prefer `as_u128_lossy` or `to_be_bytes` for any value that may exceed u64.
    pub fn as_u64(&self)   -> u64   { self.0[0] }
    pub fn as_usize(&self) -> usize { self.0[0] as usize }

    /// Returns the low 128 bits. Drops upper 128 bits silently — caller must
    /// ensure the value actually fits in u128 (e.g. wei amounts ≤ 2^120).
    pub fn as_u128_lossy(&self) -> u128 {
        (self.0[0] as u128) | ((self.0[1] as u128) << 64)
    }

    /// Saturating-add — clamps to MAX on overflow.
    pub fn saturating_add(self, rhs: Self) -> Self {
        let (r, c) = self.overflowing_add(rhs);
        if c { Self::MAX } else { r }
    }

    /// Saturating-sub — clamps to ZERO on underflow.
    pub fn saturating_sub(self, rhs: Self) -> Self {
        let (r, b) = self.overflowing_sub(rhs);
        if b { Self::ZERO } else { r }
    }

    /// Checked add — None on overflow.
    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        let (r, c) = self.overflowing_add(rhs);
        if c { None } else { Some(r) }
    }

    /// Checked sub — None on underflow.
    pub fn checked_sub(self, rhs: Self) -> Option<Self> {
        let (r, b) = self.overflowing_sub(rhs);
        if b { None } else { Some(r) }
    }
    pub fn zero() -> Self { Self::ZERO }
    pub fn one()  -> Self { Self::ONE }

    pub fn from_be_bytes(b: [u8; 32]) -> Self {
        Self([
            u64::from_be_bytes(b[24..32].try_into().unwrap()),
            u64::from_be_bytes(b[16..24].try_into().unwrap()),
            u64::from_be_bytes(b[ 8..16].try_into().unwrap()),
            u64::from_be_bytes(b[ 0.. 8].try_into().unwrap()),
        ])
    }

    pub fn to_be_bytes(self) -> [u8; 32] {
        let mut b = [0u8; 32];
        b[ 0.. 8].copy_from_slice(&self.0[3].to_be_bytes());
        b[ 8..16].copy_from_slice(&self.0[2].to_be_bytes());
        b[16..24].copy_from_slice(&self.0[1].to_be_bytes());
        b[24..32].copy_from_slice(&self.0[0].to_be_bytes());
        b
    }

    pub fn overflowing_add(self, rhs: Self) -> (Self, bool) {
        let mut r = [0u64; 4]; let mut carry = 0u64;
        for i in 0..4 {
            let (s1, c1) = self.0[i].overflowing_add(rhs.0[i]);
            let (s2, c2) = s1.overflowing_add(carry);
            r[i] = s2; carry = c1 as u64 + c2 as u64;
        }
        (Self(r), carry != 0)
    }

    pub fn overflowing_sub(self, rhs: Self) -> (Self, bool) {
        let mut r = [0u64; 4]; let mut borrow = 0u64;
        for i in 0..4 {
            let (s1, b1) = self.0[i].overflowing_sub(rhs.0[i]);
            let (s2, b2) = s1.overflowing_sub(borrow);
            r[i] = s2; borrow = b1 as u64 + b2 as u64;
        }
        (Self(r), borrow != 0)
    }

    pub fn overflowing_mul(self, rhs: Self) -> (Self, bool) {
        let a = self.0[0] as u128 | ((self.0[1] as u128) << 64);
        let b = rhs.0[0]  as u128 | ((rhs.0[1]  as u128) << 64);
        let r = a.wrapping_mul(b);
        (Self::from_u128(r), false)
    }
}

impl std::fmt::Display for U256 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Display as hex for readability (EVM style)
        let b = self.to_be_bytes();
        write!(f, "0x{}", b.iter().map(|x| format!("{:02x}", x)).collect::<String>())
    }
}

impl std::ops::BitAnd for U256 {
    type Output = Self;
    fn bitand(self, r: Self) -> Self { Self([self.0[0]&r.0[0], self.0[1]&r.0[1], self.0[2]&r.0[2], self.0[3]&r.0[3]]) }
}
impl std::ops::BitOr for U256 {
    type Output = Self;
    fn bitor(self, r: Self) -> Self  { Self([self.0[0]|r.0[0], self.0[1]|r.0[1], self.0[2]|r.0[2], self.0[3]|r.0[3]]) }
}
impl std::ops::BitXor for U256 {
    type Output = Self;
    fn bitxor(self, r: Self) -> Self { Self([self.0[0]^r.0[0], self.0[1]^r.0[1], self.0[2]^r.0[2], self.0[3]^r.0[3]]) }
}
impl std::ops::Div for U256 {
    type Output = Self;
    fn div(self, r: Self) -> Self {
        if r.is_zero() { return Self::ZERO; }
        let a = self.as_u64(); let b = r.as_u64();
        if b == 0 { Self::ZERO } else { Self::from_u64(a / b) }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants() {
        assert!(U256::ZERO.is_zero());
        assert!(!U256::ONE.is_zero());
        assert_eq!(U256::ONE.as_u64(), 1);
    }

    #[test]
    fn from_u64_and_u128() {
        let a = U256::from_u64(999);
        assert_eq!(a.as_u64(), 999);
        let b = U256::from_u128(u128::MAX);
        assert_eq!(b.as_u128_lossy(), u128::MAX);
    }

    #[test]
    fn be_bytes_roundtrip() {
        let v = U256::from_u64(0xdeadbeef);
        let bytes = v.to_be_bytes();
        let back = U256::from_be_bytes(bytes);
        assert_eq!(v, back);
    }

    #[test]
    fn saturating_add_clamps() {
        let result = U256::MAX.saturating_add(U256::ONE);
        assert_eq!(result, U256::MAX);
    }

    #[test]
    fn saturating_sub_clamps() {
        let result = U256::ZERO.saturating_sub(U256::ONE);
        assert_eq!(result, U256::ZERO);
    }

    #[test]
    fn checked_add_none_on_overflow() {
        assert!(U256::MAX.checked_add(U256::ONE).is_none());
    }

    #[test]
    fn checked_sub_none_on_underflow() {
        assert!(U256::ZERO.checked_sub(U256::ONE).is_none());
    }

    #[test]
    fn overflowing_add_carry() {
        let (_, overflow) = U256::MAX.overflowing_add(U256::ONE);
        assert!(overflow);
    }

    #[test]
    fn bitwise_ops() {
        let a = U256::from_u64(0b1010);
        let b = U256::from_u64(0b1100);
        assert_eq!((a & b).as_u64(), 0b1000);
        assert_eq!((a | b).as_u64(), 0b1110);
        assert_eq!((a ^ b).as_u64(), 0b0110);
    }

    #[test]
    fn ordering() {
        assert!(U256::ONE < U256::MAX);
        assert!(U256::ZERO < U256::ONE);
    }

    #[test]
    fn display_is_hex() {
        let s = format!("{}", U256::from_u64(255));
        assert!(s.starts_with("0x"));
    }
}
