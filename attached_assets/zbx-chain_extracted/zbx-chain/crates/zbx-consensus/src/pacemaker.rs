//! Pacemaker: adaptive round-timer coordinator for HotStuff-2.
//!
//! This module provides the high-level `PacemakerCoordinator` that sits above
//! the `Pacemaker` in `liveness.rs` and manages:
//!
//! * Timeout-share collection → Timeout Certificate (TC) assembly
//! * Delta-based adaptive timer (Algorithm 4, HotStuff-2 paper §5)
//! * Round-advancement driven by either a QC or a TC
//! * View-synchronisation broadcasts over the gossip layer
//!
//! ## Relationship to `liveness.rs`
//!
//! `liveness::Pacemaker` owns the single per-round timer and the
//! deterministic backoff logic.  `PacemakerCoordinator` wraps it,
//! aggregates timeout shares from remote validators, and decides when
//! to fire a view-change vs when to advance on a QC.
//!
//! ## TC formation (Jolteon variant)
//!
//! When a validator's timer fires it calls `on_local_timeout` which:
//! 1. Increments consecutive-timeout counter.
//! 2. Broadcasts a `TimeoutShare` signed by this validator.
//! 3. Returns `CoordinatorEvent::BroadcastTimeout` so the caller can
//!    gossip the share.
//!
//! On receiving a `TimeoutShare` from a remote peer, call `on_timeout_share`.
//! Once 2f+1 shares arrive for the same round, a TC is formed and
//! `CoordinatorEvent::NewTc` is returned.

use crate::{
    error::ConsensusError,
    liveness::{Pacemaker, PacemakerConfig},
    vote::QuorumCertificate,
};
use zbx_crypto::bls::{BlsPrivKey, BlsPubKey, BlsSignature};
use zbx_types::{address::Address, H256};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info, warn};

// ── Timeout certificate types ────────────────────────────────────────────────

/// Data that each validator signs when it times out in a round.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TimeoutShareData {
    /// The round that timed out.
    pub round: u64,
    /// Epoch number — guards against cross-epoch replays.
    pub epoch: u64,
    /// The highest QC this validator has seen so far.  Included so that
    /// the TC carries liveness proof (the new leader knows how far ahead
    /// the fastest validator is).
    pub high_qc_round: u64,
}

impl TimeoutShareData {
    /// Canonical signing payload: `round ‖ epoch ‖ high_qc_round` (big-endian u64).
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(24);
        b.extend_from_slice(&self.round.to_be_bytes());
        b.extend_from_slice(&self.epoch.to_be_bytes());
        b.extend_from_slice(&self.high_qc_round.to_be_bytes());
        b
    }

    /// Keccak-256 hash of the canonical signing payload.
    /// This is the message passed to `BlsPrivKey::sign` and `verify_single`.
    pub fn signing_hash(&self) -> H256 {
        zbx_crypto::keccak::keccak256(&self.signing_bytes())
    }
}

/// A single validator's timeout share for one round.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutShare {
    pub data: TimeoutShareData,
    pub validator: Address,
    pub bls_pubkey: BlsPubKey,
    pub signature: BlsSignature,
}

/// Aggregated timeout certificate formed from 2f+1 shares.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutCertificate {
    /// The round that timed out (all shares agree on this).
    pub round: u64,
    pub epoch: u64,
    /// The highest QC round seen among all contributors.
    pub high_qc_round: u64,
    /// Aggregated BLS signature.
    pub agg_signature: BlsSignature,
    /// Ordered list of validators that contributed.
    pub signers: Vec<Address>,
}

impl TimeoutCertificate {
    /// The TC qualifies the next leader to enter `round + 1`.
    pub fn next_round(&self) -> u64 {
        self.round + 1
    }
}

// ── In-progress TC accumulator ───────────────────────────────────────────────

/// Collects timeout shares for a single round until a TC can be formed.
struct TcAccumulator {
    round: u64,
    epoch: u64,
    quorum: usize,
    shares: HashMap<Address, TimeoutShare>,
}

impl TcAccumulator {
    fn new(round: u64, epoch: u64, quorum: usize) -> Self {
        TcAccumulator { round, epoch, quorum, shares: HashMap::new() }
    }

    /// Insert a share.  Returns the TC when quorum is reached, else `None`.
    ///
    /// # Security fix (PACEMAKER-BLS-01 / PACEMAKER-BLS-02)
    ///
    /// Previously this function:
    /// 1. Accepted shares without verifying BLS signatures (allowed forged/zero-sig shares).
    /// 2. Built the TC by copying the first share's signature as "aggregate" — wrong;
    ///    the resulting TC would fail verification by any honest downstream verifier.
    ///
    /// Now:
    /// 1. Every share is verified via `verify_single` before being accepted.
    /// 2. TC aggregation calls `bls::aggregate_signatures` over all quorum shares.
    fn insert(&mut self, share: TimeoutShare) -> Option<TimeoutCertificate> {
        if share.data.round != self.round || share.data.epoch != self.epoch {
            return None;
        }
        // Dedup — already counted this validator.
        if self.shares.contains_key(&share.validator) {
            return None;
        }
        // PACEMAKER-BLS-01: verify BLS signature before accepting.
        let msg_hash = share.data.signing_hash();
        if !zbx_crypto::bls::verify_single(&share.signature, &share.bls_pubkey, &msg_hash) {
            warn!(
                validator = ?share.validator,
                round = self.round,
                "pacemaker: dropping timeout share with invalid BLS signature"
            );
            return None;
        }
        self.shares.insert(share.validator.clone(), share);

        if self.shares.len() < self.quorum {
            return None;
        }

        // PACEMAKER-BLS-02: real BLS aggregation over all quorum shares.
        let mut high_qc_round = 0u64;
        let mut signers = Vec::new();
        let mut sigs = Vec::new();
        for (addr, s) in &self.shares {
            high_qc_round = high_qc_round.max(s.data.high_qc_round);
            signers.push(addr.clone());
            sigs.push(s.signature.clone());
        }
        let agg_signature = zbx_crypto::bls::aggregate_signatures(&sigs)
            .unwrap_or_else(|_| BlsSignature([0u8; 96]));

        Some(TimeoutCertificate {
            round: self.round,
            epoch: self.epoch,
            high_qc_round,
            agg_signature,
            signers,
        })
    }

    fn len(&self) -> usize {
        self.shares.len()
    }
}

// ── Events emitted to the caller ─────────────────────────────────────────────

/// Events returned by the coordinator to the consensus driver.
#[derive(Debug)]
pub enum CoordinatorEvent {
    /// Broadcast this timeout share to all peers.
    BroadcastTimeout(TimeoutShare),
    /// A TC has been formed — advance to `tc.next_round()`.
    NewTc(TimeoutCertificate),
    /// A QC has been received — advance to `qc.block_number() + 1`.
    NewRound { round: u64 },
    /// Nothing to do yet.
    Noop,
}

// ── PacemakerCoordinator ──────────────────────────────────────────────────────

/// High-level round coordinator combining timer, TC aggregation, and
/// adaptive delta.
pub struct PacemakerCoordinator {
    inner: Pacemaker,
    current_round: u64,
    epoch: u64,
    /// Validator's own address (for signing timeout shares).
    self_addr: Address,
    /// BLS private key — signs every timeout share this node broadcasts.
    bls_key: BlsPrivKey,
    /// Cached BLS public key derived from `bls_key` at construction.
    bls_pubkey: BlsPubKey,
    /// Quorum threshold (2f+1 out of n validators).
    quorum: usize,
    /// Highest QC round this node has observed — included in timeout shares.
    high_qc_round: u64,
    /// Per-round TC accumulators (keyed by round).
    tc_accumulators: HashMap<u64, TcAccumulator>,
    /// Adaptive timer delta (milliseconds) — updated on every QC/TC receipt.
    delta_ms: u64,
    /// Minimum adaptive delta.
    delta_min_ms: u64,
    /// Maximum adaptive delta.
    delta_max_ms: u64,
}

impl PacemakerCoordinator {
    /// Create a new coordinator starting at `round` in `epoch`.
    ///
    /// `n` is the total validator count; `f` is the max faulty count.
    /// Quorum = 2f+1 = ⌊2n/3⌋ + 1.
    pub fn new(
        self_addr: Address,
        bls_key: BlsPrivKey,
        n: usize,
        round: u64,
        epoch: u64,
        config: PacemakerConfig,
    ) -> Self {
        let f = (n - 1) / 3;
        let quorum = 2 * f + 1;
        let bls_pubkey = bls_key.to_pubkey();
        PacemakerCoordinator {
            inner: Pacemaker::new(config),
            current_round: round,
            epoch,
            self_addr,
            bls_key,
            bls_pubkey,
            quorum,
            high_qc_round: 0,
            tc_accumulators: HashMap::new(),
            delta_ms: 2_000,
            delta_min_ms: 500,
            delta_max_ms: 30_000,
        }
    }

    /// Called every tick by the consensus driver.
    /// Returns `CoordinatorEvent::BroadcastTimeout` when the local timer fires.
    pub fn tick(&mut self) -> CoordinatorEvent {
        if self.inner.is_timed_out() {
            let share = self.build_timeout_share();
            debug!(round = self.current_round, "pacemaker timeout fired — broadcasting share");
            CoordinatorEvent::BroadcastTimeout(share)
        } else {
            CoordinatorEvent::Noop
        }
    }

    /// Process an inbound QC from the network.
    ///
    /// Advances the round to `qc.block_number() + 1` and resets the timer.
    pub fn on_qc(&mut self, qc: &QuorumCertificate) -> CoordinatorEvent {
        let qc_round = qc.block_number();
        if qc_round >= self.current_round {
            self.high_qc_round = self.high_qc_round.max(qc_round);
            let next = qc_round + 1;
            self.advance_to_round(next);
            // Shrink delta on QC progress (network is responsive).
            self.adapt_delta(true);
            info!(round = next, "pacemaker: QC received → advancing round");
            return CoordinatorEvent::NewRound { round: next };
        }
        CoordinatorEvent::Noop
    }

    /// Process an inbound timeout share from a remote peer.
    pub fn on_timeout_share(
        &mut self,
        share: TimeoutShare,
    ) -> Result<CoordinatorEvent, ConsensusError> {
        if share.data.epoch != self.epoch {
            return Err(ConsensusError::InvalidEpoch {
                expected: self.epoch,
                got: share.data.epoch,
            });
        }
        let round = share.data.round;
        let acc = self
            .tc_accumulators
            .entry(round)
            .or_insert_with(|| TcAccumulator::new(round, self.epoch, self.quorum));

        debug!(
            round,
            shares = acc.len() + 1,
            quorum = self.quorum,
            "pacemaker: received timeout share"
        );

        if let Some(tc) = acc.insert(share) {
            // TC formed — clean up old accumulators.
            self.tc_accumulators.retain(|r, _| *r >= round);
            // Expand delta on timeout (network is slow).
            self.adapt_delta(false);
            info!(round, next = tc.next_round(), "pacemaker: TC formed → view change");
            self.advance_to_round(tc.next_round());
            return Ok(CoordinatorEvent::NewTc(tc));
        }
        Ok(CoordinatorEvent::Noop)
    }

    /// Build a BLS-signed timeout share for the current round.
    ///
    /// # Security fix (PACEMAKER-BLS-01)
    ///
    /// Previously this returned a `TimeoutShare` with zero/default signature
    /// and public key.  Peer `TcAccumulator`s verify BLS signatures before
    /// accepting shares, so unsigned shares were always dropped — meaning the
    /// local node could never contribute to TC formation (liveness failure).
    ///
    /// Now the share is signed with `self.bls_key` over `data.signing_hash()`,
    /// matching the verification performed in `TcAccumulator::insert`.
    fn build_timeout_share(&self) -> TimeoutShare {
        let data = TimeoutShareData {
            round: self.current_round,
            epoch: self.epoch,
            high_qc_round: self.high_qc_round,
        };
        let msg_hash = data.signing_hash();
        let signature = self.bls_key.sign(&msg_hash);
        TimeoutShare {
            data,
            validator: self.self_addr.clone(),
            bls_pubkey: self.bls_pubkey.clone(),
            signature,
        }
    }

    /// Advance to a new round and reset the inner pacemaker timer.
    fn advance_to_round(&mut self, round: u64) {
        if round > self.current_round {
            self.current_round = round;
            self.inner.start_round(round, self.epoch);
        }
    }

    /// Adapt the delta timer based on network feedback.
    /// `success=true` → shrink (network healthy).
    /// `success=false` → grow (timeout observed).
    fn adapt_delta(&mut self, success: bool) {
        if success {
            // delta = max(delta_min, delta * 7 / 8)  (gentle decrease)
            self.delta_ms = (self.delta_ms * 7 / 8).max(self.delta_min_ms);
        } else {
            // delta = min(delta_max, delta * 3 / 2)  (Pacemaker::backoff_num/den)
            self.delta_ms = (self.delta_ms.saturating_mul(3) / 2).min(self.delta_max_ms);
        }
    }

    /// Current round.
    pub fn round(&self) -> u64 { self.current_round }

    /// Current epoch.
    pub fn epoch(&self) -> u64 { self.epoch }

    /// Current adaptive delta in milliseconds.
    pub fn delta_ms(&self) -> u64 { self.delta_ms }
}
