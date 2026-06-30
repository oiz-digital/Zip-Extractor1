//! VRF (Verifiable Random Function) — commit-reveal scheme.
//!
//! Mirrors the on-chain ZbxVRF.sol logic for off-chain simulation,
//! game server seed generation, and validator-side verification.

use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use zbx_types::address::Address;

/// A pending VRF commitment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VrfCommitment {
    /// keccak256 of the secret seed submitted by the requester.
    pub seed_hash:    [u8; 32],
    /// Address of the requester.
    pub requester:    Address,
    /// Block number at which the commitment was made.
    pub commit_block: u64,
    /// Whether the seed has been revealed.
    pub fulfilled:    bool,
}

/// Minimum blocks between commit and reveal (mirrors Solidity constant).
pub const MIN_REVEAL_DELAY: u64 = 1;

/// Maximum blocks before a commitment expires.
pub const COMMITMENT_WINDOW: u64 = 256;

/// Off-chain VRF engine — used by game servers and CLI tooling.
#[derive(Debug, Default)]
pub struct VrfEngine {
    commitments: HashMap<[u8; 32], VrfCommitment>,
}

impl VrfEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a new commitment.  Returns the request_id.
    pub fn commit(
        &mut self,
        requester:    Address,
        seed_hash:    [u8; 32],
        commit_block: u64,
        prevrandao:   [u8; 32],
    ) -> [u8; 32] {
        let request_id = keccak_encode(&[
            requester.as_bytes(),
            &seed_hash,
            &commit_block.to_be_bytes(),
            &prevrandao,
        ]);

        self.commitments.insert(request_id, VrfCommitment {
            seed_hash,
            requester,
            commit_block,
            fulfilled: false,
        });

        request_id
    }

    /// Reveal a seed and derive the VRF output.
    ///
    /// # Errors
    /// Returns an error string if the request does not exist, has already been
    /// fulfilled, is too early or expired, or if the seed does not match the
    /// committed hash.
    pub fn reveal(
        &mut self,
        request_id:   [u8; 32],
        seed:         [u8; 32],
        current_block: u64,
        prevrandao:   [u8; 32],
    ) -> Result<[u8; 32], &'static str> {
        let c = self.commitments.get_mut(&request_id)
            .ok_or("VRF: no commitment")?;

        if c.fulfilled {
            return Err("VRF: already fulfilled");
        }
        if current_block < c.commit_block + MIN_REVEAL_DELAY {
            return Err("VRF: reveal too early");
        }
        if current_block > c.commit_block + COMMITMENT_WINDOW {
            return Err("VRF: commitment expired");
        }
        if keccak_single(&seed) != c.seed_hash {
            return Err("VRF: seed mismatch");
        }

        c.fulfilled = true;

        let randomness = keccak_encode(&[
            &prevrandao,
            &seed,
            &request_id,
            &c.commit_block.to_be_bytes(),
        ]);

        Ok(randomness)
    }

    /// Generate a combined random value from two independent seeds.
    /// Used for fair 2-player games.
    pub fn combined_random(
        seed0:      [u8; 32],
        seed1:      [u8; 32],
        nonce:      [u8; 32],
        prevrandao: [u8; 32],
    ) -> [u8; 32] {
        let xored: [u8; 32] = {
            let mut buf = [0u8; 32];
            for i in 0..32 { buf[i] = seed0[i] ^ seed1[i]; }
            buf
        };
        keccak_encode(&[&prevrandao, &xored, &nonce])
    }

    /// Check whether a requestId is ready to reveal.
    pub fn is_revealable(&self, request_id: &[u8; 32], current_block: u64) -> bool {
        if let Some(c) = self.commitments.get(request_id) {
            !c.fulfilled
                && current_block >= c.commit_block + MIN_REVEAL_DELAY
                && current_block <= c.commit_block + COMMITMENT_WINDOW
        } else {
            false
        }
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Compute keccak256 over a single byte slice, returns 32-byte array.
pub fn keccak_single(data: &[u8]) -> [u8; 32] {
    zbx_crypto::keccak256(data).into()
}

/// Compute keccak256 over a concatenation of multiple byte slices.
pub fn keccak_encode(parts: &[&[u8]]) -> [u8; 32] {
    let mut buf = Vec::new();
    for p in parts { buf.extend_from_slice(p); }
    zbx_crypto::keccak256(&buf).into()
}

/// Convert a 32-byte VRF output to a u64 in the range [0, range).
pub fn random_in_range(randomness: &[u8; 32], range: u64) -> u64 {
    let high = u64::from_be_bytes(randomness[..8].try_into().unwrap());
    high % range
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_reveal_roundtrip() {
        let mut engine  = VrfEngine::new();
        let requester   = Address::default();
        let seed        = [0x42u8; 32];
        let seed_hash   = keccak_single(&seed);
        let prevrandao  = [0xabu8; 32];
        let commit_block = 100u64;

        let req_id = engine.commit(requester, seed_hash, commit_block, prevrandao);
        let result = engine.reveal(req_id, seed, commit_block + 1, prevrandao);
        assert!(result.is_ok(), "reveal should succeed");
    }

    #[test]
    fn reveal_too_early() {
        let mut engine  = VrfEngine::new();
        let requester   = Address::default();
        let seed        = [0x01u8; 32];
        let seed_hash   = keccak_single(&seed);
        let prevrandao  = [0x00u8; 32];
        let commit_block = 10u64;

        let req_id = engine.commit(requester, seed_hash, commit_block, prevrandao);
        let result = engine.reveal(req_id, seed, commit_block, prevrandao);
        assert_eq!(result, Err("VRF: reveal too early"));
    }
}
