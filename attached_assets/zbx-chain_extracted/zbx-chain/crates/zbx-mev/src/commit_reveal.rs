//! Commit-reveal ordering — prevents last-second frontrunning.
//!
//! Protocol:
//!   Round N:   User submits H(tx) — the hash of the tx (public).
//!   Round N+1: User reveals the full tx content.
//!   Block N+1: Tx is included with position determined by commit order.
//!
//! This means even if an attacker sees the commit, they cannot know
//! the tx content until it's too late to frontrun (next block).

use sha3::{Digest, Keccak256};
use crate::MevError;
use std::collections::HashMap;

/// A commit: hash of a not-yet-revealed transaction.
#[derive(Debug, Clone)]
pub struct TxCommit {
    pub commit_hash: [u8; 32],
    pub sender:      [u8; 20],
    pub block_committed: u64,
    pub max_fee:     u128,
}

/// A reveal: the full transaction content, paired with its commit.
#[derive(Debug, Clone)]
pub struct TxReveal {
    pub commit_hash: [u8; 32],
    pub tx_rlp:      Vec<u8>,
}

/// Commit-reveal pool.
pub struct CommitRevealPool {
    commits: HashMap<[u8; 32], TxCommit>,
    /// Window: commits from block N can only be revealed in [N+1, N+WINDOW].
    reveal_window: u64,
}

impl CommitRevealPool {
    pub fn new(reveal_window: u64) -> Self {
        Self { commits: HashMap::new(), reveal_window }
    }

    pub fn commit(&mut self, commit: TxCommit) {
        self.commits.insert(commit.commit_hash, commit);
    }

    pub fn reveal(
        &mut self,
        reveal: TxReveal,
        current_block: u64,
    ) -> Result<Vec<u8>, MevError> {
        // Verify: hash(tx_rlp) == commit_hash.
        let hash: [u8; 32] = Keccak256::digest(&reveal.tx_rlp).into();
        if hash != reveal.commit_hash {
            return Err(MevError::RevealMismatch);
        }

        let commit = self.commits.remove(&reveal.commit_hash)
            .ok_or(MevError::RevealMismatch)?;

        // M-04 fix: enforce minimum 1-block delay to prevent proposer self-frontrunning.
        // A block proposer who committed in block N cannot reveal until block N+1 at the
        // earliest, ensuring they could not have seen the tx content before committing.
        if current_block <= commit.block_committed {
            return Err(MevError::RevealTooEarly {
                committed_at: commit.block_committed,
                earliest_reveal: commit.block_committed + 1,
            });
        }

        // Check reveal window.
        let reveal_deadline = commit.block_committed + self.reveal_window;
        if current_block > reveal_deadline {
            return Err(MevError::SlotExpired(reveal_deadline));
        }

        Ok(reveal.tx_rlp)
    }

    /// Clean up expired commits.
    pub fn prune(&mut self, current_block: u64) {
        self.commits.retain(|_, c| {
            current_block <= c.block_committed + self.reveal_window
        });
    }
}