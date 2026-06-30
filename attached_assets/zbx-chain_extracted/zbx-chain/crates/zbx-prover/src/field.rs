//! Prime field arithmetic for ZBX STARK proofs.
//!
//! We use the Goldilocks field: p = 2^64 - 2^32 + 1
//! - 64-bit field elements (fast on modern CPUs)
//! - Efficient reduction using the special prime structure
//! - Widely used in STARK systems (Plonky2, Polygon Zero)
//!
//! For BN254-based SNARKs (Groth16/PLONK), we use the BN254 scalar field
//! via the `ark-ff` crate.

/// Goldilocks prime: p = 2^64 - 2^32 + 1
pub const GOLDILOCKS_PRIME: u64 = 0xFFFF_FFFF_0000_0001;

/// A single element in the Goldilocks field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub struct GoldilocksField(pub u64);

impl GoldilocksField {
    pub const ZERO: Self = Self(0);
    pub const ONE:  Self = Self(1);

    #[inline]
    pub fn new(val: u64) -> Self {
        Self(val % GOLDILOCKS_PRIME)
    }

    #[inline]
    pub fn add(self, rhs: Self) -> Self {
        let (sum, carry) = self.0.overflowing_add(rhs.0);
        let result = if carry || sum >= GOLDILOCKS_PRIME {
            sum.wrapping_sub(GOLDILOCKS_PRIME)
        } else {
            sum
        };
        Self(result)
    }

    #[inline]
    pub fn sub(self, rhs: Self) -> Self {
        let (diff, borrow) = self.0.overflowing_sub(rhs.0);
        let result = if borrow { diff.wrapping_add(GOLDILOCKS_PRIME) } else { diff };
        Self(result)
    }

    #[inline]
    pub fn mul(self, rhs: Self) -> Self {
        // Use u128 to avoid overflow, then reduce mod p.
        let prod = (self.0 as u128) * (rhs.0 as u128);
        Self(Self::reduce128(prod))
    }

    /// Montgomery reduction for Goldilocks (efficient mod p via special prime structure).
    #[inline]
    fn reduce128(x: u128) -> u64 {
        // p = 2^64 - 2^32 + 1
        // x = x_hi * 2^64 + x_lo
        // x mod p = x_hi * (2^32 - 1) + x_lo  (then reduce once more if needed)
        let x_lo  = x as u64;
        let x_hi  = (x >> 64) as u64;
        let (result, carry) = x_lo.overflowing_add(x_hi.wrapping_mul(u32::MAX as u64 + 1).wrapping_sub(x_hi));
        if carry || result >= GOLDILOCKS_PRIME {
            result.wrapping_sub(GOLDILOCKS_PRIME)
        } else {
            result
        }
    }

    /// Modular exponentiation (square-and-multiply).
    pub fn pow(self, mut exp: u64) -> Self {
        let mut base   = self;
        let mut result = Self::ONE;
        while exp > 0 {
            if exp & 1 == 1 { result = result.mul(base); }
            base = base.mul(base);
            exp >>= 1;
        }
        result
    }

    /// Multiplicative inverse via Fermat's little theorem: a^(p-2) mod p.
    pub fn inverse(self) -> Option<Self> {
        if self == Self::ZERO { return None; }
        Some(self.pow(GOLDILOCKS_PRIME - 2))
    }

    pub fn to_bytes(self) -> [u8; 8] {
        self.0.to_le_bytes()
    }

    pub fn from_bytes(b: [u8; 8]) -> Self {
        Self::new(u64::from_le_bytes(b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn additive_identity() {
        let a = GoldilocksField::new(12345);
        assert_eq!(a.add(GoldilocksField::ZERO), a);
    }

    #[test]
    fn multiplicative_identity() {
        let a = GoldilocksField::new(99999);
        assert_eq!(a.mul(GoldilocksField::ONE), a);
    }

    #[test]
    fn inverse_roundtrip() {
        let a = GoldilocksField::new(42);
        let inv = a.inverse().unwrap();
        assert_eq!(a.mul(inv), GoldilocksField::ONE);
    }

    #[test]
    fn prime_has_no_inverse() {
        assert!(GoldilocksField::ZERO.inverse().is_none());
    }

    #[test]
    fn sub_wraps_correctly() {
        let a = GoldilocksField::new(5);
        let b = GoldilocksField::new(10);
        // 5 - 10 mod p = p - 5
        let result = a.sub(b);
        assert_eq!(result.0, GOLDILOCKS_PRIME - 5);
    }
}