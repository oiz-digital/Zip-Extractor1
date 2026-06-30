//! Safety rules: prevent double voting and equivocation.
//!
//! Each validator maintains a persistent SafetyRules store that records
//! the highest round voted and the locked QC to prevent safety violations.
//!
//! ## S35 fix (2026-05-03) — AUDIT C-12: multi-phase vote rejection
//!
//! The previous implementation used `block_number <= highest_vote_round` as
//! its sole staleness guard.  Because HotStuff casts votes in THREE phases
//! (Prepare → PreCommit → Commit) for the SAME block number, the second and
//! third phase votes were rejected with `StaleRound` and silently swallowed
//! by the `if let Ok(v)` in `on_qc` — making it impossible to ever form a
//! PreCommit or Commit QC.
//!
//! Fix:
//! * Staleness guard changed to strict `<` (block_number < highest_vote_round).
//! * Per-(block_number, phase) deduplication added via `voted_phases` to
//!   prevent equivocation within a round.
//!
//! The locked-round check (`parent_qc.block_number() < locked_round`) and the
//! cross-epoch lock-preservation (S35 C-11) are unchanged.

use crate::{error::ConsensusError, vote::{Vote, VoteData, QuorumCertificate}};
use zbx_crypto::bls::{BlsPrivKey, BlsPubKey};
use zbx_types::{address::Address, H256};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tracing::{info, warn};

/// Persistent safety state for one validator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyState {
    /// Highest block_number this validator has voted on (any phase).
    /// Prevents voting in rounds we have already surpassed.
    pub highest_vote_round: u64,
    /// Highest round with a 2-chain QC (the "locked" round).
    pub locked_round: u64,
    /// The locked QC we must extend with any new proposal.
    pub locked_qc: Option<QuorumCertificate>,
    /// Current epoch — used to reject cross-epoch votes.
    pub epoch: u64,
    /// Set of (block_number, phase) tuples for which this validator has
    /// already cast a vote.  Prevents equivocation within a round.
    #[serde(default)]
    pub voted_phases: HashSet<(u64, u8)>,
}

impl Default for SafetyState {
    fn default() -> Self {
        SafetyState {
            highest_vote_round: 0,
            locked_round: 0,
            locked_qc: None,
            epoch: 0,
            voted_phases: HashSet::new(),
        }
    }
}

/// Enforces HotStuff safety invariants for voting.
///
/// ## L-05 fix (ZBX-L-05): WAL persistence of SafetyState
///
/// `voted_phases` was an in-memory `HashSet`.  On node restart (crash or
/// scheduled upgrade) the set was empty, allowing a restarted validator to
/// re-vote for the same `(block_number, phase)` tuple in the current epoch,
/// potentially contributing to equivocation.
///
/// Fix: `SafetyRules` now carries an optional `persist_path: Option<PathBuf>`.
/// After every call to `vote()` that mutates state, `persist_state()` is
/// called, which:
///   1. Serialises `SafetyState` to JSON (via `serde_json`).
///   2. Writes the bytes to `<path>.tmp` atomically.
///   3. Renames `<path>.tmp` → `<path>` (POSIX rename is atomic on the
///      same filesystem, preventing partial-write corruption on crash).
///
/// On startup, callers should use `SafetyRules::load_or_new()` to restore the
/// persisted state before entering the consensus loop.
pub struct SafetyRules {
    state:        SafetyState,
    priv_key:     BlsPrivKey,
    pub pub_key:  BlsPubKey,
    pub address:  Address,
    /// Path to the WAL file.  `None` = in-memory only (tests / dev mode).
    persist_path: Option<std::path::PathBuf>,
}

impl SafetyRules {
    /// Create a fresh `SafetyRules` with no disk persistence (dev/test).
    pub fn new(priv_key: BlsPrivKey, address: Address) -> Self {
        let pub_key = priv_key.to_pubkey();
        SafetyRules {
            state: SafetyState::default(),
            priv_key,
            pub_key,
            address,
            persist_path: None,
        }
    }

    /// Create `SafetyRules` with WAL persistence at `path`.
    ///
    /// Loads existing state from `path` if it exists (restart recovery).
    /// If the file is absent or corrupt, starts from a clean default state.
    pub fn new_with_persist(
        priv_key: BlsPrivKey,
        address:  Address,
        path:     std::path::PathBuf,
    ) -> Self {
        let pub_key = priv_key.to_pubkey();
        let state = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        SafetyRules { state, priv_key, pub_key, address, persist_path: Some(path) }
    }

    /// Atomically persist `SafetyState` to disk (write-tmp + rename).
    ///
    /// Silently skips persistence when `persist_path` is `None`.
    /// Logs a warning if the write fails (non-fatal but operator should
    /// investigate — a failed persist means the WAL is stale).
    fn persist_state(&self) {
        if let Some(path) = &self.persist_path {
            let tmp = path.with_extension("tmp");
            match serde_json::to_vec(&self.state) {
                Ok(bytes) => {
                    if let Err(e) = std::fs::write(&tmp, &bytes) {
                        warn!(path = ?tmp, error = %e, "safety_rules: failed to write WAL tmp file");
                        return;
                    }
                    if let Err(e) = std::fs::rename(&tmp, path) {
                        warn!(path = ?path, error = %e, "safety_rules: failed to rename WAL (atomic swap)");
                    }
                }
                Err(e) => {
                    warn!(error = %e, "safety_rules: failed to serialise SafetyState");
                }
            }
        }
    }

    pub fn state(&self) -> &SafetyState {
        &self.state
    }

    /// Attempt to cast a vote for the given VoteData.
    /// Returns an error if voting would violate safety.
    pub fn vote(
        &mut self,
        data: VoteData,
        parent_qc: &QuorumCertificate,
    ) -> Result<Vote, ConsensusError> {
        // Safety check 1: never vote BELOW highest_vote_round.
        //
        // C-12 fix: use strict `<` not `<=`.  Within one round a validator
        // must vote in all three phases (same block_number).  The old `<=`
        // guard rejected the second and third phase votes, making it impossible
        // to ever form a PreCommit or Commit QC.
        if data.block_number < self.state.highest_vote_round {
            return Err(ConsensusError::StaleRound {
                got: data.block_number,
                locked: self.state.highest_vote_round,
            });
        }

        // Safety check 2: no equivocation — never vote twice for the same
        // (block_number, phase).  This catches both the obvious double-sign
        // case AND the `on_qc` re-invocation edge case where a second QC
        // for the same phase arrives unexpectedly.
        let phase_key = (data.block_number, data.phase);
        if self.state.voted_phases.contains(&phase_key) {
            return Err(ConsensusError::DuplicateVote(format!(
                "already voted for block {} phase {}",
                data.block_number, data.phase
            )));
        }

        // Safety check 3: must extend the locked QC.
        if parent_qc.block_number() < self.state.locked_round {
            return Err(ConsensusError::SafetyViolation(format!(
                "parent QC round {} < locked round {}",
                parent_qc.block_number(),
                self.state.locked_round
            )));
        }

        // Safety check 4: epoch must match (or state epoch is 0 = genesis).
        if data.epoch != self.state.epoch && self.state.epoch != 0 {
            return Err(ConsensusError::SafetyViolation(format!(
                "epoch mismatch: expected {}, got {}",
                self.state.epoch, data.epoch
            )));
        }

        // All checks passed — sign the vote with our BLS private key.
        let msg_hash = zbx_crypto::keccak::keccak256(&data.signing_bytes());
        let sig = self.priv_key.sign(&msg_hash);

        // Update in-memory safety state.
        self.state.highest_vote_round = data.block_number;
        self.state.voted_phases.insert(phase_key);

        // L-05 fix: atomically persist state BEFORE returning the vote.
        // If the node crashes after this line, the persisted WAL will prevent
        // re-voting for this (block_number, phase) on restart.
        self.persist_state();

        info!(
            round = data.block_number,
            phase = data.phase,
            "validator cast vote"
        );
        Ok(Vote { data, voter: self.address, signature: sig })
    }

    /// Update locked round when we observe a 2-chain QC.
    pub fn update_locked_qc(&mut self, qc: QuorumCertificate) {
        if qc.block_number() > self.state.locked_round {
            info!(locked_round = qc.block_number(), "updating locked QC");
            self.state.locked_round = qc.block_number();
            self.state.locked_qc = Some(qc);
        }
    }

    /// Advance to a new epoch (validator set rotation).
    ///
    /// **S35-hotstuff-safety / AUDIT C-11 closure (2026-05-02)**:
    /// `locked_round` and `locked_qc` are INTENTIONALLY preserved across
    /// epoch boundaries — resetting them would be a HotStuff safety violation.
    /// See the original method-level docs in the S35 audit for the full
    /// rationale.
    pub fn advance_epoch(&mut self, new_epoch: u64) {
        if new_epoch > self.state.epoch {
            self.state.epoch = new_epoch;
            // C-11: locked_round and locked_qc are NOT reset here.
            // C-12: voted_phases is scoped to rounds; clear it on epoch advance
            //       so the new epoch starts with a clean equivocation log.
            self.state.voted_phases.clear();
        }
    }
}
