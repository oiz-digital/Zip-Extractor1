//! Dispute Voting Mechanism — ZBX token holders vote on disputed prices.
//!
//! Inspired by UMA's DVM. ZBX stakers vote on the correct price.
//! Majority wins. Wrong voters are slashed (small %), correct voters rewarded.
//!
//! # Anti-Bribery Design
//!
//! Key insight from UMA: if attacker bribes voters to vote wrong,
//! they must pay more than the value of all ZBX at stake.
//! This makes attacks economically irrational for any dispute < ~1% TVL.

use serde::{Serialize, Deserialize};
use std::collections::HashMap;

/// A vote commitment (commit-reveal to prevent copying).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VoteCommit {
    /// H(price || salt) — hides the vote until reveal
    pub commitment: [u8; 32],
    /// ZBX voting power (stake)
    pub stake:      u128,
}

/// A revealed vote.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RevealedVote {
    pub price:  i128,
    pub salt:   [u8; 32],
    pub stake:  u128,
    pub voter:  [u8; 20],
}

/// DVM voting round state.
#[derive(Debug)]
pub enum DvmState {
    Commit { ends_at: u64 },
    Reveal { ends_at: u64 },
    Resolved { result: i128 },
}

pub struct DisputeVotingMechanism {
    /// Dispute ID → commits
    commits: HashMap<[u8; 32], Vec<([u8; 20], VoteCommit)>>,
    /// Dispute ID → reveals
    reveals: HashMap<[u8; 32], Vec<RevealedVote>>,
}

impl DisputeVotingMechanism {
    pub fn new() -> Self {
        Self { commits: HashMap::new(), reveals: HashMap::new() }
    }

    /// Phase 1: commit a vote (hash of price + salt).
    pub fn commit_vote(
        &mut self,
        dispute_id: [u8; 32],
        voter:      [u8; 20],
        commit:     VoteCommit,
    ) {
        self.commits.entry(dispute_id).or_default().push((voter, commit));
    }

    /// Phase 2: reveal a vote (price + salt — must match commitment).
    pub fn reveal_vote(
        &mut self,
        dispute_id: [u8; 32],
        voter:      [u8; 20],
        price:      i128,
        salt:       [u8; 32],
    ) -> bool {
        use sha2::{Sha256, Digest};
        // Find commitment
        let commits = match self.commits.get(&dispute_id) {
            Some(c) => c, None => return false,
        };
        let commit = match commits.iter().find(|(v, _)| *v == voter) {
            Some((_, c)) => c, None => return false,
        };
        // Verify commitment
        let mut h = Sha256::new();
        h.update(&price.to_le_bytes());
        h.update(&salt);
        let hash: [u8; 32] = h.finalize().into();
        if hash != commit.commitment { return false; }

        self.reveals.entry(dispute_id).or_default().push(
            RevealedVote { price, salt, stake: commit.stake, voter }
        );
        true
    }

    /// Tally votes — stake-weighted median of revealed prices.
    pub fn tally(&self, dispute_id: [u8; 32]) -> Option<i128> {
        let reveals = self.reveals.get(&dispute_id)?;
        if reveals.is_empty() { return None; }

        // Sort by price, find stake-weighted median
        let mut weighted: Vec<(i128, u128)> = reveals.iter()
            .map(|r| (r.price, r.stake))
            .collect();
        weighted.sort_unstable_by_key(|(p, _)| *p);

        let total_stake: u128 = weighted.iter().map(|(_, s)| s).sum();
        let half = total_stake / 2;
        let mut cumulative = 0u128;
        for (price, stake) in &weighted {
            cumulative += stake;
            if cumulative >= half {
                return Some(*price);
            }
        }
        None
    }
}