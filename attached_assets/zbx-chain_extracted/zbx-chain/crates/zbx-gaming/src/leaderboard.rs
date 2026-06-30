//! On-chain leaderboard — ERC-20 reward distribution for top players.
//!
//! Leaderboard entries are submitted by authorised game contracts.
//! At epoch end, the top-N players are eligible to claim ERC-20 rewards
//! proportional to their score share.

use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};
use zbx_types::address::Address;

/// A single leaderboard entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub player:  Address,
    pub score:   u64,
    pub wins:    u32,
    pub losses:  u32,
}

/// Leaderboard for a single game type / season.
#[derive(Debug, Default)]
pub struct Leaderboard {
    /// player → cumulative score
    scores: BTreeMap<Address, Entry>,
}

impl Leaderboard {
    pub fn new() -> Self { Self::default() }

    /// Record a game result.
    pub fn record_game(
        &mut self,
        winner: Address,
        winner_score: u64,
        loser:  Address,
        loser_score:  u64,
    ) {
        let w = self.scores.entry(winner).or_insert_with(|| Entry {
            player: winner, score: 0, wins: 0, losses: 0,
        });
        w.score += winner_score;
        w.wins  += 1;

        let l = self.scores.entry(loser).or_insert_with(|| Entry {
            player: loser, score: 0, wins: 0, losses: 0,
        });
        l.score += loser_score;
        l.losses += 1;
    }

    /// Return top-N players sorted by score descending.
    pub fn top_n(&self, n: usize) -> Vec<&Entry> {
        let mut entries: Vec<&Entry> = self.scores.values().collect();
        entries.sort_by(|a, b| b.score.cmp(&a.score));
        entries.truncate(n);
        entries
    }

    /// Compute reward shares (in basis points, summing to 10_000) for top-N.
    ///
    /// Uses a simple linear decay: rank 1 gets the most points, rank N the least.
    /// Returns a Vec of (address, share_bps) pairs.
    pub fn reward_shares(&self, n: usize) -> Vec<(Address, u64)> {
        let top = self.top_n(n);
        if top.is_empty() { return vec![]; }
        let total_weight: u64 = (1..=(top.len() as u64)).sum();
        let shares: Vec<(Address, u64)> = top
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let rank_weight = (top.len() - i) as u64;
                let bps = (rank_weight * 10_000) / total_weight;
                (e.player, bps)
            })
            .collect();
        shares
    }

    /// Get a player's current rank (1-indexed).  Returns None if not on board.
    pub fn rank_of(&self, player: &Address) -> Option<usize> {
        let mut entries: Vec<&Entry> = self.scores.values().collect();
        entries.sort_by(|a, b| b.score.cmp(&a.score));
        entries.iter().position(|e| &e.player == player).map(|i| i + 1)
    }

    pub fn entry(&self, player: &Address) -> Option<&Entry> {
        self.scores.get(player)
    }
}
