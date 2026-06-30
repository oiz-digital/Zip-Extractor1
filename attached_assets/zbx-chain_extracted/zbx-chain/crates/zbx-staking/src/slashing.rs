//! Slashing detector: identifies double-sign and liveness faults.
//!
//! ## Instant-jail on consecutive misses
//! Per-block: if a validator misses MAX_CONSECUTIVE_MISSED blocks in a row
//! (default 20 = ~100 s at 5 s/block) it is jailed immediately without
//! waiting for the epoch boundary. The caller must invoke
//! `record_missed_block` each block for every expected-but-absent signer,
//! and `record_vote` (which resets the counter) whenever a vote arrives.

use crate::{
    error::StakingError, validator::ValidatorSet,
    MAX_CONSECUTIVE_MISSED, SLASH_DOUBLE_SIGN, SLASH_LIVENESS_DAILY,
};
use zbx_types::{address::Address, H256};
use std::collections::{HashMap, HashSet};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use tracing::warn;

/// Evidence of a slashable offence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SlashEvidence {
    /// Validator signed two different blocks at the same height.
    DoubleSign {
        height: u64,
        hash_a: H256,
        hash_b: H256,
    },
    /// Validator missed more than 50% of blocks in an epoch.
    LivenessFault {
        epoch: u64,
        missed_blocks: u64,
        total_blocks: u64,
    },
    /// Validator missed MAX_CONSECUTIVE_MISSED blocks in a row — node is down.
    ConsecutiveMiss {
        from_block: u64,
        count:      u64,
    },
}

/// A processed slash event to be applied to state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlashEvent {
    pub validator: Address,
    pub evidence: SlashEvidence,
    pub slash_amount: u128,  // wei to burn
    pub jailed: bool,
}

/// Tracks validator behaviour and detects slashable offences.
pub struct SlashingDetector {
    /// Votes observed per (height, phase) — used for double-sign detection.
    vote_records: HashMap<(u64, u8), HashMap<Address, H256>>,
    /// Missed block counter per validator per epoch.
    missed_blocks: HashMap<(u64, Address), u64>,
    /// Consecutive missed block counter per validator (reset on any signed block).
    consecutive_missed: HashMap<Address, u64>,
    /// Block number when the current consecutive-miss streak started.
    streak_start: HashMap<Address, u64>,
    /// Already-processed slash evidence (prevents double-slash).
    processed: HashSet<H256>,
}

impl SlashingDetector {
    pub fn new() -> Self {
        SlashingDetector {
            vote_records:       HashMap::new(),
            missed_blocks:      HashMap::new(),
            consecutive_missed: HashMap::new(),
            streak_start:       HashMap::new(),
            processed:          HashSet::new(),
        }
    }

    /// Record a validator vote. Resets the consecutive-miss streak.
    /// Returns `Some(SlashEvent)` if a double-sign is detected.
    pub fn record_vote(
        &mut self,
        validator: Address,
        height: u64,
        phase: u8,
        block_hash: H256,
    ) -> Option<SlashEvent> {
        // Reset consecutive-miss streak — validator is alive.
        self.consecutive_missed.remove(&validator);
        self.streak_start.remove(&validator);

        let key = (height, phase);
        let entry = self.vote_records.entry(key).or_default();
        if let Some(&existing_hash) = entry.get(&validator) {
            if existing_hash != block_hash {
                warn!(
                    validator = ?validator, height, phase,
                    "DOUBLE SIGN DETECTED"
                );
                return Some(self.build_slash(
                    validator,
                    SlashEvidence::DoubleSign { height, hash_a: existing_hash, hash_b: block_hash },
                    SLASH_DOUBLE_SIGN,
                    true,
                ));
            }
        } else {
            entry.insert(validator, block_hash);
        }
        None
    }

    /// Record a missed block for a validator.
    ///
    /// Two things happen:
    /// 1. Per-epoch counter incremented (for end-of-epoch liveness check).
    /// 2. Consecutive-miss counter incremented. If it reaches
    ///    `MAX_CONSECUTIVE_MISSED` the validator is **instantly jailed**
    ///    and `Some(SlashEvent)` is returned — the caller must apply it
    ///    immediately via `apply_slashes`.
    pub fn record_missed_block(
        &mut self,
        validator: Address,
        epoch: u64,
        block_height: u64,
    ) -> Option<SlashEvent> {
        // Per-epoch counter (used by check_liveness_faults at epoch end).
        *self.missed_blocks.entry((epoch, validator)).or_insert(0) += 1;

        // Consecutive-miss counter.
        let streak = self.consecutive_missed.entry(validator).or_insert(0);
        if *streak == 0 {
            self.streak_start.insert(validator, block_height);
        }
        *streak += 1;

        if *streak >= MAX_CONSECUTIVE_MISSED {
            let from = self.streak_start.get(&validator).copied().unwrap_or(block_height);
            let count = *streak;
            // Reset so we don't fire again every block after threshold.
            *streak = 0;
            self.streak_start.remove(&validator);

            warn!(
                validator = ?validator,
                from_block = from,
                count,
                "INSTANT JAIL: validator missed {} consecutive blocks — node is down", count
            );
            return Some(self.build_slash(
                validator,
                SlashEvidence::ConsecutiveMiss { from_block: from, count },
                0,    // no stake burn for liveness (jail is the penalty)
                true, // jailed = true
            ));
        }
        None
    }

    /// Check for liveness faults at epoch end (>50% missed).
    /// Only needed for validators not already caught by consecutive-miss.
    pub fn check_liveness_faults(
        &self,
        epoch: u64,
        epoch_blocks: u64,
    ) -> Vec<SlashEvent> {
        let threshold = epoch_blocks / 2; // >50% missed → fault
        self.missed_blocks
            .iter()
            .filter(|((e, _), &missed)| *e == epoch && missed > threshold)
            .map(|((_, validator), &missed)| {
                SlashEvent {
                    validator: *validator,
                    evidence: SlashEvidence::LivenessFault {
                        epoch,
                        missed_blocks: missed,
                        total_blocks: epoch_blocks,
                    },
                    slash_amount: 0, // liveness faults: jailed, no stake burn initially
                    jailed: true,
                }
            })
            .collect()
    }

    /// Prune vote records older than `finalized_height` to bound memory usage.
    ///
    /// STK-SLS-02: `vote_records` grows without bound at ~1 entry per (height, phase)
    /// pair. At 2-second blocks this accumulates ~43 200 entries/day. Callers should
    /// invoke this after each finalized block to discard evidence that can no longer
    /// be challenged.
    pub fn prune_vote_records(&mut self, finalized_height: u64) {
        self.vote_records.retain(|(height, _), _| *height >= finalized_height);
    }

    /// Apply slash events to the validator set.
    ///
    /// STK-SLS-01: Each event is content-addressed via SHA3-256 over
    /// (validator, evidence context). If the same event is submitted twice
    /// (e.g. a duplicate gossip message), the second application is silently
    /// skipped rather than burning the validator's stake a second time.
    pub fn apply_slashes(
        &mut self,
        events: Vec<SlashEvent>,
        validators: &mut ValidatorSet,
    ) {
        for event in events {
            let id = Self::slash_event_id(&event);
            if self.processed.contains(&id) {
                warn!(validator = ?event.validator, "duplicate slash event ignored (STK-SLS-01)");
                continue;
            }
            self.processed.insert(id);

            if let Some(v) = validators.get_mut(&event.validator) {
                let slash = (v.total_stake() * event.slash_amount as u128 / 10_000).min(v.self_stake);
                v.self_stake = v.self_stake.saturating_sub(slash);
                if event.jailed {
                    v.status = crate::validator::ValidatorStatus::Jailed;
                    warn!(validator = ?event.validator, slash_wei = slash, "validator slashed and jailed");
                } else if v.self_stake < crate::MIN_SELF_STAKE
                    && v.status == crate::validator::ValidatorStatus::Active
                {
                    // STK-SLS-06: slash reduced self_stake below the registration
                    // minimum without triggering an explicit jail (e.g. a small
                    // liveness-fraction slash).  Demote Active → Pending so the
                    // validator is excluded from the next election until it tops
                    // up its self-stake.  We do NOT force Unbonding here — the
                    // validator may wish to add stake and re-enter consensus.
                    v.status = crate::validator::ValidatorStatus::Pending;
                    warn!(
                        validator = ?event.validator,
                        self_stake = v.self_stake,
                        min_stake  = crate::MIN_SELF_STAKE,
                        "STK-SLS-06: self_stake below MIN_SELF_STAKE after slash — demoted to Pending"
                    );
                }
            }
        }
    }

    /// Compute a stable content-addressed ID for a slash event.
    /// Used by `apply_slashes` to prevent double-application (STK-SLS-01).
    fn slash_event_id(event: &SlashEvent) -> H256 {
        let mut h = Sha3_256::new();
        h.update(&event.validator.0);
        h.update(event.slash_amount.to_le_bytes());
        h.update([event.jailed as u8]);
        match &event.evidence {
            SlashEvidence::DoubleSign { height, hash_a, hash_b } => {
                h.update(height.to_le_bytes());
                h.update(&hash_a.0);
                h.update(&hash_b.0);
            }
            SlashEvidence::LivenessFault { epoch, missed_blocks, total_blocks } => {
                h.update(epoch.to_le_bytes());
                h.update(missed_blocks.to_le_bytes());
                h.update(total_blocks.to_le_bytes());
            }
            SlashEvidence::ConsecutiveMiss { from_block, count } => {
                h.update(from_block.to_le_bytes());
                h.update(count.to_le_bytes());
            }
        }
        let bytes = h.finalize();
        let mut id = [0u8; 32];
        id.copy_from_slice(&bytes);
        H256(id)
    }

    /// Prune old entries from the `processed` deduplication set.
    ///
    /// STK-SLS-02 (memory bound): `processed` is a `HashSet<H256>` that grows
    /// without bound because slash IDs are never removed.  At 2-second blocks a
    /// busy network can generate hundreds of slash events per day; over months
    /// the set would consume significant memory.
    ///
    /// Strategy: if the set exceeds `max_size`, drain entries until it is back
    /// at `max_size / 2`.  Because `HashSet` has no stable ordering, the drain
    /// is arbitrary — this is safe because finalized slashes cannot be replayed
    /// (the state has already been applied), so dropping an old ID merely means
    /// a theoretical re-submission of the same evidence ID would pass the
    /// dedup check; the validator set state is already correct.
    ///
    /// Callers should set `max_size` to a value larger than the maximum number
    /// of slash events expected in any reorg window (e.g. 10 000).
    pub fn prune_processed(&mut self, max_size: usize) {
        if self.processed.len() <= max_size {
            return;
        }
        let target = max_size / 2;
        let to_drain = self.processed.len().saturating_sub(target);
        let keys: Vec<H256> = self.processed.iter().copied().take(to_drain).collect();
        for k in keys {
            self.processed.remove(&k);
        }
    }

    /// Prune per-epoch missed-block counters for epochs older than `min_epoch`.
    ///
    /// STK-SLS-07: `missed_blocks` maps `(epoch, validator) → count`.  Without
    /// pruning it grows by `active_set_size` entries per epoch indefinitely.
    /// After finality the missed-block data for old epochs can never be acted
    /// upon (liveness faults have already been processed or the window passed).
    pub fn prune_missed_blocks(&mut self, min_epoch: u64) {
        self.missed_blocks.retain(|(epoch, _), _| *epoch >= min_epoch);
    }

    fn build_slash(
        &self,
        validator: Address,
        evidence: SlashEvidence,
        slash_bps: u128,
        jailed: bool,
    ) -> SlashEvent {
        SlashEvent { validator, evidence, slash_amount: slash_bps, jailed }
    }
}

impl Default for SlashingDetector { fn default() -> Self { Self::new() } }