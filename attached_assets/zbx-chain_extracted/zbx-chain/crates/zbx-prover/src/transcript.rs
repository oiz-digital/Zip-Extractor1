//! Fiat-Shamir transcript — makes interactive STARK non-interactive.
//!
//! The prover commits to the execution trace, then the transcript generates
//! "random" challenges deterministically from those commitments using a
//! hash function (Keccak-256). This turns the interactive STARK proof into
//! a non-interactive proof (NIZK) via the Fiat-Shamir heuristic.
//!
//! All parties (prover and verifier) must use identical transcript inputs
//! in identical order for the challenges to match.

use sha3::{Digest, Keccak256};
use crate::field::GoldilocksField;

/// Keccak-256-based Fiat-Shamir transcript.
pub struct Transcript {
    state: Keccak256,
    counter: u64,
}

impl Transcript {
    /// Create a new transcript with a domain separator.
    pub fn new(domain: &[u8]) -> Self {
        let mut t = Self { state: Keccak256::new(), counter: 0 };
        t.absorb(b"zbx-stark-v2-transcript:");
        t.absorb(domain);
        t
    }

    /// Absorb data into the transcript (updates prover state).
    pub fn absorb(&mut self, data: &[u8]) {
        // Include a length prefix to prevent extension attacks.
        self.state.update(&(data.len() as u64).to_le_bytes());
        self.state.update(data);
    }

    /// Absorb a field element.
    pub fn absorb_field(&mut self, f: GoldilocksField) {
        self.absorb(&f.to_bytes());
    }

    /// Absorb a byte commitment (e.g. Merkle root).
    pub fn absorb_commitment(&mut self, commitment: &[u8; 32]) {
        self.absorb(commitment);
    }

    /// Squeeze a pseudo-random field element (deterministic challenge).
    pub fn squeeze_field(&mut self) -> GoldilocksField {
        let bytes = self.squeeze_bytes();
        // Take 8 bytes from the hash output and reduce mod p.
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&bytes[..8]);
        GoldilocksField::new(u64::from_le_bytes(buf))
    }

    /// Squeeze pseudo-random bytes (deterministic).
    pub fn squeeze_bytes(&mut self) -> [u8; 32] {
        self.counter += 1;
        // Fork the state: hash(current_state || counter) to get challenge.
        let mut fork = self.state.clone();
        fork.update(b"squeeze:");
        fork.update(&self.counter.to_le_bytes());
        let result: [u8; 32] = fork.finalize_reset().into();
        // Absorb the challenge back to advance the transcript state.
        self.absorb(&result);
        result
    }

    /// Squeeze a random index in [0, domain_size).
    pub fn squeeze_index(&mut self, domain_size: usize) -> usize {
        let bytes = self.squeeze_bytes();
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&bytes[..8]);
        (u64::from_le_bytes(buf) as usize) % domain_size
    }

    /// Squeeze multiple random indices (for FRI queries).
    pub fn squeeze_indices(&mut self, count: usize, domain_size: usize) -> Vec<usize> {
        (0..count).map(|_| self.squeeze_index(domain_size)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squeeze_is_deterministic() {
        let mut t1 = Transcript::new(b"test");
        let mut t2 = Transcript::new(b"test");
        t1.absorb(b"hello");
        t2.absorb(b"hello");
        assert_eq!(t1.squeeze_bytes(), t2.squeeze_bytes());
    }

    #[test]
    fn different_domains_give_different_challenges() {
        let mut t1 = Transcript::new(b"domain-a");
        let mut t2 = Transcript::new(b"domain-b");
        t1.absorb(b"data");
        t2.absorb(b"data");
        assert_ne!(t1.squeeze_bytes(), t2.squeeze_bytes());
    }

    #[test]
    fn absorption_changes_output() {
        let mut t = Transcript::new(b"test");
        let r1 = t.squeeze_bytes();
        t.absorb(b"extra data");
        let r2 = t.squeeze_bytes();
        assert_ne!(r1, r2);
    }
}