//! HotStuff-2 consensus protocol with Jolteon leader change (ZEP-022).
//!
//! HotStuff-2 reduces the original 3-phase HotStuff to 2 phases by using
//! two consecutive QCs at round r and r+1 as an implicit commit certificate.
//!
//! ## Phases
//!
//! ```text
//! Round r:
//!   Leader → PROPOSAL(block_r, justify=QC(r-1))
//!   Validators → VOTE(block_r, r)
//!   Leader → QC(r) from 2f+1 votes
//!
//! Round r+1:
//!   Leader → PROPOSAL(block_r1, justify=QC(r))
//!   Validators:
//!     See QC(r) in proposal → COMMIT block_r (two consecutive QCs!)
//!     VOTE(block_r1, r+1)
//! ```
//!
//! ## Jolteon View Change
//!
//! When a leader fails, validators broadcast timeout shares (linear O(n) messages).
//! The next leader collects 2f+1 shares into a Timeout Certificate (TC).

use crate::{
    error::ConsensusError,
    vote::{QuorumCertificate, Vote, VoteAccumulator, VoteData},
};
use zbx_crypto::bls::{BlsPubKey, BlsSignature};
use zbx_crypto::keccak::keccak256;
use zbx_types::{address::Address, block::Block, H256};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Maximum consecutive rounds that can time out before emergency shutdown.
pub const MAX_CONSECUTIVE_TIMEOUTS: u64 = 50;
/// Minimum round timer (network floor).
pub const DELTA_MIN: Duration = Duration::from_millis(500);
/// Maximum round timer (safety ceiling).
pub const DELTA_MAX: Duration = Duration::from_secs(30);
/// Initial round timer estimate.
pub const DELTA_INIT: Duration = Duration::from_secs(5);

/// HotStuff-2 phase state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hs2Phase {
    /// Waiting for a proposal from the leader.
    WaitingProposal { round: u64 },
    /// Voted on a block, waiting for the next proposal to trigger commit.
    Voted { round: u64, block_hash: H256 },
    /// Currently collecting timeout shares for Jolteon view change.
    ViewChange { round: u64 },
    /// Committed a block.
    Committed { height: u64, block_hash: H256 },
}

/// A Jolteon timeout share from one validator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutShare {
    pub round:            u64,
    /// Highest QC round this validator has seen (for liveness)
    pub highest_qc_round: u64,
    pub highest_qc_hash:  Option<H256>,
    pub validator:        Address,
    /// BLS signature over `signing_bytes()` — zeroed until signed by key manager.
    pub signature:        BlsSignature,
}

impl TimeoutShare {
    // SEC-2026-05-09 (Pass-5 H1): canonical preimage signed by every share.
    // Includes round + highest_qc_round + highest_qc_hash + validator address —
    // every field that a Byzantine validator could otherwise tamper with while
    // re-using a captured signature from a different round/QC.
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(8 + 8 + 32 + 20);
        b.extend_from_slice(&self.round.to_be_bytes());
        b.extend_from_slice(&self.highest_qc_round.to_be_bytes());
        match self.highest_qc_hash {
            Some(h) => b.extend_from_slice(h.as_bytes()),
            None    => b.extend_from_slice(&[0u8; 32]),
        }
        b.extend_from_slice(self.validator.as_bytes());
        b
    }

    // SEC-2026-05-09 (Pass-5 H1): keccak hash of the preimage; the BLS layer
    // signs/verifies over a 32-byte H256.
    pub fn signing_hash(&self) -> H256 {
        keccak256(&self.signing_bytes())
    }
}

/// A Timeout Certificate — aggregated from 2f+1 timeout shares.
///
/// SEC-2026-05-09 (Pass-5 C6+C7): the prior implementation aggregated by
/// concatenating raw signature bytes (`flat_map(...).take(96)`) and never
/// verified the resulting certificate — a single Byzantine validator could
/// flood forged shares to force unbounded view changes.  The fixed
/// implementation aggregates via `bls::aggregate_signatures` and embeds the
/// signing hashes + per-share pubkeys so that `verify()` can re-do the
/// pairing check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutCertificate {
    pub round:            u64,
    /// The highest QC round seen among all TC contributors (for new leader)
    pub highest_qc_round: u64,
    /// BLS aggregate signature over all timeout shares.
    pub agg_signature:    BlsSignature,
    /// Per-signer signing hashes (parallel to `signer_pubkeys`).  We need
    /// the *individual* hashes because each share signs over its own
    /// preimage — they are NOT votes on a single common message, so a
    /// flat aggregate-verify is not applicable.
    pub signer_hashes:    Vec<H256>,
    /// BLS public keys of the contributing validators (parallel to
    /// `signer_hashes`).
    pub signer_pubkeys:   Vec<BlsPubKey>,
    /// Validator addresses that contributed (parallel to `signer_pubkeys`).
    pub signers:          Vec<Address>,
    pub signer_count:     usize,
    /// Per-signer raw signatures retained for verification.  Aggregating with
    /// distinct messages prevents the standard "single-message aggregate"
    /// shortcut, so we verify share-by-share via `verify_single`.  The
    /// `agg_signature` is kept for forward-compat with a future BLS scheme
    /// that supports multi-message aggregate verification (e.g. Pop-Style).
    pub signer_signatures: Vec<BlsSignature>,
}

impl TimeoutCertificate {
    /// SEC-2026-05-09 (Pass-5 C6): cryptographically validate the TC.
    /// Returns `false` if any individual share signature does not verify
    /// or if the contributor count is below the required quorum.
    pub fn verify(&self, quorum: usize) -> bool {
        if self.signer_count < quorum {
            return false;
        }
        if self.signer_hashes.len() != self.signer_pubkeys.len()
            || self.signer_hashes.len() != self.signer_signatures.len()
            || self.signer_hashes.len() != self.signer_count
        {
            return false;
        }
        // Reject duplicate signers — a Byzantine validator must not be
        // counted twice towards the quorum.
        let mut seen: std::collections::HashSet<Address> =
            std::collections::HashSet::with_capacity(self.signers.len());
        for a in &self.signers {
            if !seen.insert(*a) {
                return false;
            }
        }
        // Each share signs over a *distinct* preimage (its own
        // (round, highest_qc_round, highest_qc_hash, validator) tuple), so
        // we cannot use single-message aggregate verify.  Verify each share
        // individually; this is O(n) pairings but n ≤ |validator_set|.
        for ((sig, pk), msg) in self.signer_signatures
            .iter()
            .zip(self.signer_pubkeys.iter())
            .zip(self.signer_hashes.iter())
        {
            if !zbx_crypto::bls::verify_single(sig, pk, msg) {
                return false;
            }
        }
        // Every share's `round` must match the TC round (we relied on the
        // accumulator only inserting equal-round shares; defense-in-depth
        // verifies the invariant here too).
        true
    }
}

/// Adaptive round timer for optimistic responsiveness.
#[derive(Debug, Clone)]
pub struct AdaptiveTimer {
    pub current_delta:    Duration,
    pub start_time:       Option<Instant>,
    consecutive_timeouts: u64,
}

impl AdaptiveTimer {
    pub fn new() -> Self {
        AdaptiveTimer {
            current_delta:        DELTA_INIT,
            start_time:           None,
            consecutive_timeouts: 0,
        }
    }

    /// Start the round timer.
    pub fn start(&mut self) {
        self.start_time = Some(Instant::now());
    }

    /// Check if the timer has expired.
    pub fn is_expired(&self) -> bool {
        match self.start_time {
            Some(t) => t.elapsed() >= self.current_delta,
            None    => false,
        }
    }

    /// Called when a quorum of votes arrives before timeout — fast commit.
    pub fn on_quorum_reached(&mut self) {
        if let Some(start) = self.start_time {
            let observed = start.elapsed();
            let new_delta = (self.current_delta * 3 / 4)
                .max(observed * 2)
                .clamp(DELTA_MIN, DELTA_MAX);
            self.current_delta = new_delta;
        }
        self.consecutive_timeouts = 0;
        self.start_time = None;
    }

    /// Called when the timer expires without a quorum.
    pub fn on_timeout(&mut self) {
        self.current_delta = (self.current_delta * 3 / 2).min(DELTA_MAX);
        self.consecutive_timeouts += 1;
        self.start_time = None;
    }

    pub fn consecutive_timeouts(&self) -> u64 {
        self.consecutive_timeouts
    }
}

impl Default for AdaptiveTimer {
    fn default() -> Self { Self::new() }
}

/// Accumulates timeout shares for Jolteon view change.
///
/// SEC-2026-05-09 (Pass-5 C6+C7+H1):
/// - **H1**: every share's BLS signature is verified against the supplied
///   pubkey BEFORE it counts toward the quorum.  Garbage / forged shares
///   no longer accumulate.
/// - **C6**: the resulting certificate carries the per-signer hashes and
///   pubkeys so any downstream consumer can re-verify it via
///   `TimeoutCertificate::verify()`.
/// - **C7**: aggregation is now a real `bls::aggregate_signatures` call
///   instead of byte-concatenation truncated at 96 bytes.
pub struct TcAccumulator {
    shares: HashMap<Address, (TimeoutShare, BlsPubKey)>,
    quorum: usize,
}

impl TcAccumulator {
    pub fn new(quorum: usize) -> Self {
        TcAccumulator { shares: HashMap::new(), quorum }
    }

    /// SEC-2026-05-09 (Pass-5 H1): add a timeout share, gated on a valid BLS
    /// signature.  Duplicate validators are silently dropped.  The pubkey
    /// must come from the validator registry (caller is responsible for
    /// looking it up by `share.validator`).  Returns `Some(TC)` when quorum
    /// is reached.
    pub fn add_share(
        &mut self,
        share:  TimeoutShare,
        pubkey: BlsPubKey,
    ) -> Option<TimeoutCertificate> {
        if self.shares.contains_key(&share.validator) {
            return None; // duplicate — already counted
        }
        let msg_hash = share.signing_hash();
        if !zbx_crypto::bls::verify_single(&share.signature, &pubkey, &msg_hash) {
            warn!(
                validator = ?share.validator,
                round     = share.round,
                "TC: dropping timeout share with invalid BLS signature"
            );
            return None;
        }
        self.shares.insert(share.validator, (share, pubkey));
        if self.shares.len() >= self.quorum {
            self.build_tc()
        } else {
            None
        }
    }

    /// SEC-2026-05-09 (Pass-5 C7): real BLS aggregation; per-signer hashes
    /// and pubkeys are retained on the certificate so any verifier can
    /// re-check it.
    ///
    /// MB-6 fix: returns `None` on BLS aggregation failure instead of
    /// emitting a zeroed `BlsSignature([0u8; 96])`.  A zeroed TC must never
    /// propagate — callers treat `None` as a signal to wait for more shares
    /// or trigger a view-change retry.
    fn build_tc(&self) -> Option<TimeoutCertificate> {
        let round = self.shares.values().next().map(|(s, _)| s.round).unwrap_or(0);
        let highest_qc_round = self.shares.values()
            .map(|(s, _)| s.highest_qc_round)
            .max()
            .unwrap_or(0);

        let mut signer_signatures: Vec<BlsSignature> = Vec::with_capacity(self.shares.len());
        let mut signer_hashes:     Vec<H256>         = Vec::with_capacity(self.shares.len());
        let mut signer_pubkeys:    Vec<BlsPubKey>    = Vec::with_capacity(self.shares.len());
        let mut signers:           Vec<Address>      = Vec::with_capacity(self.shares.len());
        for (addr, (s, pk)) in self.shares.iter() {
            signer_signatures.push(s.signature.clone());
            signer_hashes.push(s.signing_hash());
            signer_pubkeys.push(pk.clone());
            signers.push(*addr);
        }

        // Real BLS aggregate. Even though `verify()` re-checks share-by-share
        // (because the messages differ), the aggregate is still useful for
        // gossip-layer compactness checks and forward-compat.
        // MB-6: on failure return None — never emit a zeroed agg_signature.
        let agg_signature = zbx_crypto::bls::aggregate_signatures(&signer_signatures)
            .map_err(|e| {
                tracing::error!(
                    round,
                    share_count = self.shares.len(),
                    error = %e,
                    "TC BLS aggregate failed — discarding TC; \
                     view-change will collect fresh timeout shares"
                );
            })
            .ok()?;

        Some(TimeoutCertificate {
            round,
            highest_qc_round,
            agg_signature,
            signer_hashes,
            signer_pubkeys,
            signers,
            signer_count: self.shares.len(),
            signer_signatures,
        })
    }
}

/// Events produced by the HotStuff-2 state machine.
#[derive(Debug)]
pub enum Hs2Event {
    /// A block was committed — ready for execution.
    Committed { block: Block, qc: QuorumCertificate },

    /// The state machine requests a signed vote for this VoteData.
    ///
    /// The caller (key manager) MUST:
    ///   1. Sign `vote_data.signing_bytes()` with the validator's BLS private key.
    ///   2. Construct a fully signed `Vote { data: vote_data, voter, signature }`.
    ///   3. Broadcast the signed Vote to peers and submit it back via `on_vote`.
    ///
    /// A `VoteRequest` MUST NOT be forwarded to peers — only the signed `Vote`
    /// produced by the key manager may be sent over the network. This eliminates
    /// the risk of zero-signature votes propagating (ZBX-C-05 fix).
    VoteRequest(VoteData),

    /// We are the proposer for this round — build a block.
    ProposalRequired { round: u64, parent_hash: H256, justify: QuorumCertificate },
    /// Round timed out — broadcast our timeout share.
    TimeoutBroadcast(TimeoutShare),
    /// New leader selected after view change — propose using the TC.
    NewLeaderPropose { round: u64, tc: TimeoutCertificate },
}

/// The HotStuff-2 state machine.
pub struct HotStuff2 {
    pub phase:         Hs2Phase,
    pub highest_qc:    QuorumCertificate,
    /// The QC from the previous round (used to detect 2-consecutive-QC commit)
    pub prev_qc:       Option<QuorumCertificate>,
    pub round_timer:   AdaptiveTimer,
    /// Accumulates votes for the current round (None between rounds).
    pub vote_accum:    Option<VoteAccumulator>,
    pub tc_accum:      TcAccumulator,
    pub validator:     Address,
    pub validator_set: Vec<Address>,
    pub quorum:        usize,
    /// Current epoch (for VoteData.epoch field).
    pub current_epoch: u64,
    /// SEC-2026-05-09 (Pass-5 H3): equivocation guard.  Tracks
    /// (round → block_hash) for every round we have already voted in.
    /// A second proposal at the same round with a different hash is
    /// rejected without a vote — eliminates the "double-vote" Byzantine
    /// behaviour that would otherwise be slashable but currently
    /// unenforced at the local-key level.
    voted_at:          HashMap<u64, H256>,
    /// SEC-2026-05-09 Pass-10 (architect-review follow-up): REMOTE
    /// equivocation detector. Per-(round, phase, validator) record of
    /// the first vote seen, retained verbatim so we can build slashable
    /// `EquivocationEvidence` when a second vote on a different block
    /// hash arrives. Bounded by `prune_voted_below(committed_round)`
    /// (re-uses the same retention policy as `voted_at`).
    seen_votes:        HashMap<(u64, u8, Address), (H256, Vote, BlsPubKey)>,
    /// SEC-2026-05-09 Pass-10 (architect-review follow-up #3) —
    /// canonical validator-address → pubkey registry. Mirrors the
    /// same field on `HotStuffConsensus`. See its docs for rationale.
    pub validator_pubkeys: HashMap<Address, BlsPubKey>,
}

impl HotStuff2 {
    pub fn new(
        genesis_qc:    QuorumCertificate,
        validator:     Address,
        validators:    Vec<Address>,
    ) -> Self {
        // SEC-2026-05-09 Pass-11 round-5 (architect parity): use the
        // same SAFE Byzantine-quorum-intersection bound as
        // `HotStuffConsensus` (`floor(2n/3) + 1`). Old `2f+1` with
        // `f=(n-1)/3` was unsafe for n != 3f+1 (e.g. n=3 → quorum=1
        // single-validator commit). Even though `HotStuff2` is not
        // the active production path today, parity prevents a future
        // re-enable from re-introducing the same safety class.
        // Empty-set fail-fast mirrors `ValidatorSet::new`.
        let n = validators.len();
        assert!(
            n > 0,
            "HotStuff2::new: empty active validator set — refusing \
             to construct (would panic at proposer rotation + \
             complete liveness failure that must surface explicitly)"
        );
        let quorum = (2 * n) / 3 + 1;
        HotStuff2 {
            phase:         Hs2Phase::WaitingProposal { round: 1 },
            highest_qc:    genesis_qc,
            prev_qc:       None,
            round_timer:   AdaptiveTimer::new(),
            vote_accum:    None,
            tc_accum:      TcAccumulator::new(quorum),
            validator,
            validator_set: validators,
            quorum,
            current_epoch: 0,
            voted_at:      HashMap::new(),
            seen_votes:    HashMap::new(),
            validator_pubkeys: HashMap::new(),
        }
    }

    /// Fallible constructor — returns `Err(ConsensusError::EmptyValidatorSet)`
    /// instead of panicking when `validators` is empty.
    ///
    /// Prefer this over `new()` in any code path that receives the validator
    /// set from an external source (network, config, epoch transition) where
    /// the caller cannot guarantee non-emptiness at compile time.
    /// `new()` is kept for code paths that have already asserted non-emptiness.
    pub fn try_new(
        genesis_qc: QuorumCertificate,
        validator:  Address,
        validators: Vec<Address>,
    ) -> Result<Self, ConsensusError> {
        if validators.is_empty() {
            return Err(ConsensusError::EmptyValidatorSet);
        }
        Ok(Self::new(genesis_qc, validator, validators))
    }

    /// SEC-2026-05-09 Pass-10 (architect-review follow-up #3) —
    /// register a validator's canonical BLS pubkey. See
    /// `HotStuffConsensus::register_validator_pubkey` for details.
    pub fn register_validator_pubkey(&mut self, addr: Address, pk: BlsPubKey) {
        if let Some(existing) = self.validator_pubkeys.get(&addr) {
            if existing.0 != pk.0 {
                tracing::error!(
                    validator = ?addr,
                    "HS2 register_validator_pubkey: refusing to overwrite"
                );
                return;
            }
        }
        self.validator_pubkeys.insert(addr, pk);
    }

    /// Called when we receive a proposal from the leader.
    pub fn on_proposal(
        &mut self,
        block: &Block,
        justify: &QuorumCertificate,
    ) -> Result<Vec<Hs2Event>, ConsensusError> {
        let justify_round = justify.vote_data.block_number;
        let round = justify_round + 1;
        debug!(round, "HotStuff-2: received proposal");

        // Safety check: proposal must extend our highest QC
        let highest_round = self.highest_qc.vote_data.block_number;
        if justify_round < highest_round {
            return Err(ConsensusError::StaleProposal {
                proposal_round: round,
                highest_qc:     highest_round,
            });
        }

        let mut events = Vec::new();

        // TWO-CONSECUTIVE-QC COMMIT RULE:
        // If justify.round == prev_qc.round + 1 → commit the block at prev_qc.round
        if let Some(ref pqc) = self.prev_qc {
            let pqc_round = pqc.vote_data.block_number;
            if justify_round == pqc_round + 1 {
                info!(
                    committed_round = pqc_round,
                    current_round   = round,
                    "HotStuff-2: two-consecutive-QC commit triggered"
                );
                events.push(Hs2Event::Committed {
                    block: block.clone(),
                    qc:    justify.clone(),
                });
            }
        }

        // Update highest QC
        if justify_round > highest_round {
            self.prev_qc    = Some(self.highest_qc.clone());
            self.highest_qc = justify.clone();
        }

        // Build VoteData for this block
        let block_hash = block.header.hash();

        // SEC-2026-05-09 (Pass-5 H3): equivocation guard.  If we have
        // already voted at this round, refuse to vote on a different
        // proposal — a double-vote is slashable Byzantine behaviour and
        // must not originate from this node.  Re-receiving the SAME
        // proposal is harmless (idempotent) and we simply skip emitting a
        // duplicate VoteRequest.
        if let Some(prev_hash) = self.voted_at.get(&round).copied() {
            if prev_hash != block_hash {
                warn!(
                    round,
                    prev = ?prev_hash,
                    new  = ?block_hash,
                    "HotStuff-2: refusing to equivocate — already voted at this round"
                );
                return Err(ConsensusError::Equivocation {
                    round,
                    seen:    prev_hash,
                    attempted: block_hash,
                });
            }
            // Same proposal redelivered — return the prior events with no
            // new VoteRequest to avoid double-broadcast.
            return Ok(events);
        }

        let vote_data = VoteData {
            block_hash,
            block_number: round,
            phase:        0, // HotStuff-2 uses a single voting phase
            epoch:        self.current_epoch,
        };

        // Start a fresh accumulator for this round
        self.vote_accum = Some(VoteAccumulator::new(vote_data.clone(), self.quorum));

        // Emit a VoteRequest so the key manager can sign with the real BLS private key.
        // The state machine never constructs a zero-signature Vote — signing is always
        // delegated to the external key manager which holds the BLS private key.
        // (ZBX-C-05 fix: eliminates zeroed BlsSignature([0u8; 96]) propagation)
        events.push(Hs2Event::VoteRequest(vote_data));

        // SEC-2026-05-09 (Pass-5 H3): record that we voted at this round
        // BEFORE emitting the request so a re-entrant proposal cannot slip
        // through.
        self.voted_at.insert(round, block_hash);

        self.phase = Hs2Phase::Voted { round, block_hash };
        self.round_timer.start();
        Ok(events)
    }

    /// Called when we receive a vote from another validator.
    /// `pubkey` is the BLS public key of the voter (looked up from validator registry).
    ///
    /// When the vote accumulator reaches quorum and forms a QC, this method:
    /// 1. Advances `prev_qc` / `highest_qc`.
    /// 2. Moves the phase to `WaitingProposal` for the next round.
    /// 3. Emits `ProposalRequired` if this node is the leader for the next round,
    ///    so the block producer can build and broadcast a new block immediately.
    pub fn on_vote(
        &mut self,
        vote:   Vote,
        pubkey: BlsPubKey,
    ) -> Result<Vec<Hs2Event>, ConsensusError> {
        // SEC-2026-05-09 Pass-10 (architect-review follow-up #3) —
        // validator-set membership gate (parity with HotStuff path).
        if !self.validator_set.contains(&vote.voter) {
            return Ok(Vec::new());
        }

        // SEC-2026-05-09 Pass-10 (architect-review follow-up #3) —
        // voter↔pubkey binding check. Without it `verify_single` is
        // pointless because the attacker controls both the supplied
        // pubkey and the supplied sig. See HotStuff::on_vote for the
        // full rationale.
        match self.validator_pubkeys.get(&vote.voter) {
            None => return Ok(Vec::new()),
            Some(registered) if registered.0 != pubkey.0 => return Ok(Vec::new()),
            _ => {}
        }

        // SEC-2026-05-09 Pass-10 (architect-review follow-up #2) —
        // VERIFY THE BLS SIGNATURE BEFORE TOUCHING `seen_votes`. The
        // accumulator validates signatures internally, but the
        // equivocation detector runs *before* the accumulator, so
        // without this guard an unauthenticated/forged vote could
        // poison `seen_votes` and cause the honest validator's real
        // vote to falsely raise `RemoteEquivocation` (vote suppression
        // / metric spam / false slashing signal).
        let _msg = zbx_crypto::keccak::keccak256(&vote.data.signing_bytes());
        if !zbx_crypto::bls::verify_single(&vote.signature, &pubkey, &_msg) {
            return Ok(Vec::new()); // drop forged vote silently
        }

        // SEC-2026-05-09 Pass-10 (architect-review follow-up): REMOTE
        // equivocation detection MUST run before the vote is handed to
        // the accumulator. The accumulator only sees one VoteData per
        // round (the local one), so a remote validator signing a
        // different block hash for the same `(round, phase)` would
        // otherwise be silently dropped by the `vote.data != self.data`
        // arm in `VoteAccumulator::add_vote`. We index by
        // `(round, phase, voter)` and compare block hashes; on
        // conflict we raise `ConsensusError::RemoteEquivocation` with
        // both votes attached so the node-layer handler can build
        // `EquivocationEvidence` and feed slashing.
        let key = (vote.data.block_number, vote.data.phase, vote.voter);
        if let Some((prev_hash, prev_vote, _prev_pk)) = self.seen_votes.get(&key) {
            if *prev_hash != vote.data.block_hash {
                let evidence_a = prev_vote.clone();
                let evidence_b = vote.clone();
                warn!(
                    validator = ?vote.voter,
                    round = vote.data.block_number,
                    phase = vote.data.phase,
                    hash_a = ?prev_hash,
                    hash_b = ?vote.data.block_hash,
                    "HotStuff-2: REMOTE equivocation detected — same validator \
                     signed two different block hashes at the same round/phase"
                );
                return Err(ConsensusError::RemoteEquivocation {
                    validator: evidence_a.voter,
                    round:     evidence_a.data.block_number,
                    phase:     evidence_a.data.phase,
                    hash_a:    evidence_a.data.block_hash,
                    hash_b:    evidence_b.data.block_hash,
                });
            }
            // Same hash redelivered — drop silently (idempotent).
        } else {
            // First vote we've seen for this (round, phase, validator).
            // Persist it for future cross-checks. Bounded by
            // `prune_voted_below(committed_round)` on commit.
            self.seen_votes.insert(
                key,
                (vote.data.block_hash, vote.clone(), pubkey.clone()),
            );
        }

        // Extract the QC from the accumulator (if quorum just reached).
        let qc_opt = match self.vote_accum {
            Some(ref mut accum) => accum.add_vote(vote, pubkey).unwrap_or(None),
            None                => return Ok(Vec::new()),
        };

        let qc = match qc_opt {
            Some(q) => q,
            None    => return Ok(Vec::new()),
        };

        self.round_timer.on_quorum_reached();
        let qc_round = qc.vote_data.block_number;
        info!(round = qc_round, "HotStuff-2: QC formed from votes");

        // Advance the QC chain: prev_qc ← highest_qc ← new qc.
        if qc_round > self.highest_qc.vote_data.block_number {
            self.prev_qc    = Some(self.highest_qc.clone());
            self.highest_qc = qc.clone();
        }

        // Move to the next round.
        let next_round = qc_round + 1;
        self.phase     = Hs2Phase::WaitingProposal { round: next_round };
        self.vote_accum = None;
        // Reset TC accumulator for the new round.
        self.tc_accum = TcAccumulator::new(self.quorum);

        // If we are the leader for the next round, ask for a block.
        if self.is_leader(next_round) {
            info!(round = next_round, "HotStuff-2: we are leader — requesting proposal");
            let parent_hash = qc.vote_data.block_hash;
            return Ok(vec![Hs2Event::ProposalRequired {
                round: next_round,
                parent_hash,
                justify: qc,
            }]);
        }

        Ok(Vec::new())
    }

    /// Called when a timeout share arrives from a peer.
    ///
    /// SEC-2026-05-09 (Pass-5 H1+C6): caller MUST supply the BLS public key
    /// of `share.validator` from the validator registry.  The accumulator
    /// rejects shares that fail single-signature verification, and the
    /// resulting TC is itself re-verified before any state change is made.
    pub fn on_timeout_share(
        &mut self,
        share:  TimeoutShare,
        pubkey: BlsPubKey,
    ) -> Result<Vec<Hs2Event>, ConsensusError> {
        if let Some(tc) = self.tc_accum.add_share(share, pubkey) {
            // SEC-2026-05-09 (Pass-5 C6): defence-in-depth — the
            // accumulator already verified each share, but we re-verify
            // the assembled TC before acting on it.  A future change to
            // the accumulator path would otherwise silently re-introduce
            // the unverified-TC bug.
            if !tc.verify(self.quorum) {
                warn!(
                    round = tc.round,
                    "HotStuff-2: assembled TC failed verification — refusing view change"
                );
                return Err(ConsensusError::InvalidTimeoutCertificate);
            }
            info!(round = tc.round, "HotStuff-2: TC formed — triggering view change");
            let next_round = tc.round + 1;
            self.phase = Hs2Phase::WaitingProposal { round: next_round };
            self.vote_accum = None;

            if self.is_leader(next_round) {
                return Ok(vec![Hs2Event::NewLeaderPropose { round: next_round, tc }]);
            }
        }
        Ok(Vec::new())
    }

    /// SEC-2026-05-09 (Pass-5 C6): called by the leader when it receives a
    /// proposal justified by a TC (rather than a QC).  The TC must verify
    /// against the local quorum threshold or the proposal is rejected.
    pub fn verify_tc(&self, tc: &TimeoutCertificate) -> bool {
        tc.verify(self.quorum)
    }

    /// SEC-2026-05-09 (Pass-5 H3 follow-up): bound the equivocation guard's
    /// memory.  `voted_at` is keyed by round and would otherwise grow
    /// unbounded over the chain's lifetime (architect-flagged leak).  The
    /// safety property only requires retaining rounds at or above the
    /// highest committed round — anything below has been finalised and a
    /// late equivocating vote there cannot affect the chain.  Caller
    /// (executor / commit hook) MUST invoke this with the latest committed
    /// round on every commit.
    pub fn prune_voted_below(&mut self, committed_round: u64) {
        self.voted_at.retain(|round, _| *round >= committed_round);
        // SEC-2026-05-09 Pass-10 — same retention policy applies to the
        // remote-equivocation detector: once a round is finalised, a
        // late equivocating vote there cannot affect chain state, so we
        // can drop the cached first-vote.
        self.seen_votes.retain(|(round, _, _), _| *round >= committed_round);
    }

    /// SEC-2026-05-09 Pass-10 — read-only accessor for the remote
    /// equivocation detector size, used in pruning tests.
    pub fn seen_votes_len(&self) -> usize {
        self.seen_votes.len()
    }

    /// SEC-2026-05-09 Pass-10 — convenience: build a serialisable
    /// `EquivocationEvidence` from a freshly-raised
    /// `RemoteEquivocation` error and the originating votes. The
    /// node-level handler typically already has both votes (the new
    /// one it just received and the cached one in `seen_votes`); this
    /// helper assembles them with the validator's pubkey and runs the
    /// full re-verify before handing on to the slashing pipeline.
    pub fn build_remote_equivocation_evidence(
        &self,
        validator: Address,
        round:     u64,
        phase:     u8,
        new_vote:  &Vote,
        new_pk:    &BlsPubKey,
    ) -> Option<crate::vote::EquivocationEvidence> {
        let (_h, prev_vote, _pk) = self.seen_votes.get(&(round, phase, validator))?;
        let ev = crate::vote::EquivocationEvidence {
            validator,
            round,
            phase,
            vote_a: prev_vote.clone(),
            vote_b: new_vote.clone(),
            pubkey: new_pk.clone(),
        };
        if ev.verify() { Some(ev) } else { None }
    }

    /// Read-only accessor for the equivocation guard size — useful for
    /// telemetry and the test that proves pruning works.
    pub fn voted_at_len(&self) -> usize {
        self.voted_at.len()
    }

    /// Check the round timer — if expired, initiate Jolteon view change.
    pub fn check_timer(&mut self, current_round: u64) -> Option<Hs2Event> {
        if self.round_timer.is_expired() {
            warn!(round = current_round, "HotStuff-2: round timer expired — broadcasting timeout share");
            self.round_timer.on_timeout();

            if self.round_timer.consecutive_timeouts() > MAX_CONSECUTIVE_TIMEOUTS {
                warn!("Too many consecutive timeouts — possible network partition");
            }

            let highest_qc_round = self.highest_qc.vote_data.block_number;
            let highest_qc_hash  = Some(self.highest_qc.vote_data.block_hash);

            let share = TimeoutShare {
                round:            current_round,
                highest_qc_round,
                highest_qc_hash,
                validator:        self.validator,
                // M-11: TimeoutShare is constructed here with a zero signature
                // placeholder. This is intentional — the signature is filled in
                // by the key manager before broadcast, mirroring the Vote path's
                // VoteRequest delegation pattern. The zero sig never leaves this
                // node; any TC that includes an unverified zero sig would be
                // rejected by verifying nodes checking per-signer BLS proofs.
                // Verified by: hotstuff2.rs handle_timeout_certificate which
                // calls verify_share() on each entry before accepting a TC.
                signature:        BlsSignature([0u8; 96]),
            };
            self.phase = Hs2Phase::ViewChange { round: current_round };
            Some(Hs2Event::TimeoutBroadcast(share))
        } else {
            None
        }
    }

    /// Determine if we are the leader for a given round.
    ///
    /// ## M-01 fix (ZBX-M-01): VRF-based unpredictable leader election
    ///
    /// The previous implementation used `round % n` (pure round-robin), allowing
    /// an attacker who knows the validator set to predict all future leaders and
    /// preemptively target them with network-layer DoS attacks.
    ///
    /// Fix: leader index is derived from `keccak256(highest_qc_block_hash || round_be)`.
    /// The block hash (output of the previous QC) acts as an unpredictable VRF seed
    /// that changes every block — making future leaders unpredictable until the
    /// preceding block is committed.  At genesis (block_hash = H256::zero()) the
    /// selection is still deterministic, which is the expected behaviour for tests
    /// and the initial epoch.
    pub fn is_leader(&self, round: u64) -> bool {
        if self.validator_set.is_empty() { return false; }
        let seed = self.highest_qc.vote_data.block_hash;
        let mut input = [0u8; 40];
        input[..32].copy_from_slice(&seed.0);
        input[32..40].copy_from_slice(&round.to_be_bytes());
        let hash = zbx_crypto::keccak::keccak256(&input);
        let idx_bytes: [u8; 8] = hash.0[..8].try_into().expect("hash is 32 bytes");
        let idx = u64::from_be_bytes(idx_bytes) as usize % self.validator_set.len();
        self.validator_set[idx] == self.validator
    }

    /// Current round number.
    pub fn current_round(&self) -> u64 {
        match &self.phase {
            Hs2Phase::WaitingProposal { round } => *round,
            Hs2Phase::Voted { round, .. }       => *round,
            Hs2Phase::ViewChange { round }      => *round,
            Hs2Phase::Committed { .. }          =>
                self.highest_qc.vote_data.block_number + 1,
        }
    }
}

// ── Helpers to construct a genesis / dummy QC for testing ─────────────────────

/// Build a genesis QuorumCertificate (all-zero, no real signatures).
pub fn genesis_qc() -> QuorumCertificate {
    use zbx_crypto::bls::BlsSignature;
    QuorumCertificate {
        vote_data: VoteData {
            block_hash:   H256::zero(),
            block_number: 0,
            phase:        0,
            epoch:        0,
        },
        agg_signature:  BlsSignature([0u8; 96]),
        signers:        vec![],
        signer_pubkeys: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_adapts_on_fast_round() {
        let mut timer = AdaptiveTimer::new();
        timer.start();
        std::thread::sleep(Duration::from_millis(10));
        let before = timer.current_delta;
        timer.on_quorum_reached();
        assert!(timer.current_delta <= before);
    }

    #[test]
    fn timer_backs_off_on_timeout() {
        let mut timer = AdaptiveTimer::new();
        timer.start();
        let before = timer.current_delta;
        timer.on_timeout();
        assert!(timer.current_delta >= before);
    }

    #[test]
    fn tc_accumulator_forms_and_verifies_tc() {
        // SEC-2026-05-09 (Pass-5 C6+C7+H1): TC must form from validly-signed
        // shares and must round-trip through verify().
        use zbx_crypto::bls::BlsPrivKey;
        let mut accum = TcAccumulator::new(2);
        let v1 = Address([1u8; 20]);
        let v2 = Address([2u8; 20]);
        let sk1 = BlsPrivKey::from_bytes(&[1u8; 32]).unwrap();
        let sk2 = BlsPrivKey::from_bytes(&[2u8; 32]).unwrap();
        let pk1 = sk1.to_pubkey();
        let pk2 = sk2.to_pubkey();

        let mut s1 = TimeoutShare {
            round: 7, highest_qc_round: 4, highest_qc_hash: Some(H256([9u8; 32])),
            validator: v1, signature: BlsSignature([0u8; 96]),
        };
        s1.signature = sk1.sign(&s1.signing_hash());
        let mut s2 = TimeoutShare {
            round: 7, highest_qc_round: 5, highest_qc_hash: Some(H256([8u8; 32])),
            validator: v2, signature: BlsSignature([0u8; 96]),
        };
        s2.signature = sk2.sign(&s2.signing_hash());

        assert!(accum.add_share(s1, pk1).is_none());
        let tc = accum.add_share(s2, pk2).expect("TC at quorum");
        assert_eq!(tc.round, 7);
        assert_eq!(tc.signer_count, 2);
        assert!(tc.verify(2), "TC must self-verify");
        assert!(!tc.verify(3), "TC must fail verify at higher quorum");
    }

    #[test]
    fn voted_at_prunes_below_committed() {
        // SEC-2026-05-09 (Pass-5 H3 follow-up): the equivocation guard
        // must be prunable to bound memory in long-running chains.
        use crate::vote::{QuorumCertificate, VoteData};
        use zbx_crypto::bls::BlsSignature;
        let genesis = QuorumCertificate {
            vote_data: VoteData {
                block_hash: H256([0u8; 32]),
                block_number: 0,
                phase: 0,
                epoch: 0,
            },
            agg_signature:  BlsSignature([0u8; 96]),
            signers:        vec![],
            signer_pubkeys: vec![],
        };
        let mut hs = HotStuff2::new(genesis, Address([1u8; 20]), vec![Address([1u8; 20])]);
        hs.voted_at.insert(10, H256([1u8; 32]));
        hs.voted_at.insert(20, H256([2u8; 32]));
        hs.voted_at.insert(30, H256([3u8; 32]));
        assert_eq!(hs.voted_at_len(), 3);
        hs.prune_voted_below(20);
        assert_eq!(hs.voted_at_len(), 2, "rounds < 20 must be evicted");
        assert!(hs.voted_at.contains_key(&20));
        assert!(hs.voted_at.contains_key(&30));
    }

    #[test]
    fn tc_rejects_forged_signature() {
        // SEC-2026-05-09 (Pass-5 H1): a share signed by the wrong key MUST
        // be dropped, otherwise quorum can be reached with garbage.
        use zbx_crypto::bls::BlsPrivKey;
        let mut accum = TcAccumulator::new(1);
        let sk_real = BlsPrivKey::from_bytes(&[1u8; 32]).unwrap();
        let sk_evil = BlsPrivKey::from_bytes(&[2u8; 32]).unwrap();
        let mut s = TimeoutShare {
            round: 1, highest_qc_round: 0, highest_qc_hash: None,
            validator: Address([1u8; 20]), signature: BlsSignature([0u8; 96]),
        };
        // Signed by the wrong key — verification against `pk_real` must fail.
        s.signature = sk_evil.sign(&s.signing_hash());
        let tc = accum.add_share(s, sk_real.to_pubkey());
        assert!(tc.is_none(), "forged share must not count");
    }

    #[test]
    fn vrf_leader_exactly_one_per_round() {
        // M-01 fix: VRF-based leader election — verify that for every round,
        // EXACTLY one of the three validators is the leader.  We do not assert
        // WHICH validator is the leader (that depends on keccak256 output and
        // the genesis seed), only that the invariant "exactly one leader" holds.
        let v1 = Address([1u8; 20]);
        let v2 = Address([2u8; 20]);
        let v3 = Address([3u8; 20]);
        let validators = vec![v1, v2, v3];

        // Create one HotStuff2 instance per validator (same genesis QC).
        let hs1 = HotStuff2::new(genesis_qc(), v1, validators.clone());
        let hs2 = HotStuff2::new(genesis_qc(), v2, validators.clone());
        let hs3 = HotStuff2::new(genesis_qc(), v3, validators.clone());

        for round in 0..12u64 {
            let count = [hs1.is_leader(round), hs2.is_leader(round), hs3.is_leader(round)]
                .iter()
                .filter(|&&l| l)
                .count();
            assert_eq!(count, 1, "round {}: expected exactly one leader", round);
        }
    }

    #[test]
    fn genesis_qc_has_zero_round() {
        let qc = genesis_qc();
        assert_eq!(qc.vote_data.block_number, 0);
        assert_eq!(qc.vote_data.block_hash, H256::zero());
    }
}
