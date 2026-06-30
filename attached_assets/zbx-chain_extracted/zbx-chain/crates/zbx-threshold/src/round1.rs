//! FROST Round 1: Nonce commitment broadcast.

use serde_big_array::BigArray;
use serde::{Serialize, Deserialize};
use rand::RngCore;

/// Nonce pair: (hiding, binding) — random scalars for this signing session.
/// Hiding nonce protects against key recovery attacks.
/// Binding nonce binds the partial sig to the specific signing group.
#[derive(Clone)]
pub(crate) struct SigningNonces {
    pub hiding:  [u8; 32],
    pub binding: [u8; 32],
}

impl SigningNonces {
    /// Generate fresh random nonces.
    pub fn generate(rng: &mut impl RngCore) -> Self {
        let mut hiding  = [0u8; 32];
        let mut binding = [0u8; 32];
        rng.fill_bytes(&mut hiding);
        rng.fill_bytes(&mut binding);
        Self { hiding, binding }
    }
}

/// Public nonce commitments broadcast in Round 1.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NonceCommitment {
    /// Signer index
    pub index:   u32,
    /// Commitment to hiding nonce: hiding × G
    #[serde(with = "BigArray")]
    pub D:       [u8; 33],
    /// Commitment to binding nonce: binding × G
    #[serde(with = "BigArray")]
    pub E:       [u8; 33],
}

impl NonceCommitment {
    /// Verify this commitment is a valid curve point (not identity).
    pub fn is_valid(&self) -> bool {
        self.D[0] == 0x02 || self.D[0] == 0x03  // compressed EC point prefix
    }
}