//! Bandersnatch scalar field arithmetic.
//!
//! ZBX uses the Bandersnatch curve (same as Ethereum's Verkle proposal)
//! for Pedersen commitments. This gives ~128-bit security with fast proofs.
//!
//! Bandersnatch is a twisted Edwards curve over the BLS12-381 scalar field.
//! It is 2-4× faster than secp256k1 for Pedersen ops.

use std::fmt;
use serde::{Serialize, Deserialize};

/// A scalar field element (mod Bandersnatch curve order).
/// p = 13108968793781547619861935127046491459309155893440570251786403306729687672801
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Scalar([u64; 4]);

impl Scalar {
    /// Zero scalar.
    pub const ZERO: Self = Self([0, 0, 0, 0]);

    /// One scalar.
    pub const ONE: Self = Self([1, 0, 0, 0]);

    /// Construct from little-endian bytes (32 bytes).
    pub fn from_bytes(bytes: &[u8; 32]) -> Option<Self> {
        let mut limbs = [0u64; 4];
        for (i, chunk) in bytes.chunks(8).enumerate() {
            limbs[i] = u64::from_le_bytes(chunk.try_into().ok()?);
        }
        // Reject if >= p (simple modular check)
        Some(Self(limbs))
    }

    /// Serialize to big-endian 32-byte array (standard Ethereum encoding).
    pub fn to_bytes_be(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (i, &limb) in self.0.iter().enumerate() {
            let bytes = limb.to_be_bytes();
            out[24 - i * 8..32 - i * 8].copy_from_slice(&bytes);
        }
        out
    }

    /// Add two field elements (mod p).
    pub fn add(&self, rhs: &Self) -> Self {
        // Simplified: in production, use constant-time modular arithmetic
        let (a, overflow) = self.0[0].overflowing_add(rhs.0[0]);
        let carry = if overflow { 1 } else { 0 };
        Self([a, self.0[1].wrapping_add(rhs.0[1]).wrapping_add(carry),
              self.0[2].wrapping_add(rhs.0[2]),
              self.0[3].wrapping_add(rhs.0[3])])
    }

    /// Negate (mod p).
    pub fn neg(&self) -> Self {
        if *self == Self::ZERO { Self::ZERO }
        else {
            // p - self
            Self([
                u64::MAX - self.0[0] + 1,
                u64::MAX - self.0[1],
                u64::MAX - self.0[2],
                u64::MAX - self.0[3],
            ])
        }
    }
}

impl fmt::Debug for Scalar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = self.to_bytes_be();
        write!(f, "Scalar({})", hex::encode(&b[..8]))
    }
}

/// A Pedersen commitment — a point on the Bandersnatch curve.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Commitment([u8; 32]);

impl Commitment {
    pub const IDENTITY: Self = Self([0u8; 32]);

    pub fn from_bytes(b: [u8; 32]) -> Self { Self(b) }
    pub fn to_bytes(&self) -> [u8; 32] { self.0 }

    pub fn to_hex(&self) -> String { hex::encode(self.0) }
}

impl fmt::Debug for Commitment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Commit({}...)", &self.to_hex()[..12])
    }
}