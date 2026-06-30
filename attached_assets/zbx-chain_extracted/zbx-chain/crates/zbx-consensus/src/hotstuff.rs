//! Core HotStuff-BFT state machine.
//!
//! Three-phase protocol: Prepare → PreCommit → Commit → Decide.
//! Each phase requires a 2f+1 quorum (f = max Byzantine faults).

use crate::{
    block_store::BlockStore,
    error::ConsensusError,
    liveness::Pacemaker,
    safety_rules::SafetyRules,
    vote::{QuorumCertificate, Vote, VoteAccumulator, VoteData},
};
use zbx_types::{address::Address, block::Block, H256};
use tracing::{debug, info, warn};
use std::collections::HashMap;

/// The three HotStuff message phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Prepare   = 0,
    PreCommit = 1,
    Commit    = 2,
}

/// Events produced by the HotStuff state machine.
#[derive(Debug)]
pub enum ConsensusEvent {
    /// A new block has been committed and is ready for execution.
    Committed { block: Block, qc: QuorumCertificate },
    /// A vote was produced and should be broadcast to the proposer.
    VoteCast(Vote),
    /// Proposer selected for this round.
    ProposalRequired { round: u64, parent_hash: H256 },
    /// Round timed out.
    Timeout(u64),
}

/// Validator set configuration.
pub struct ValidatorSet {
    pub validators: Vec<Address>,
    pub quorum: usize, // floor(2n/3) + 1 — Byzantine-quorum-intersection bound
    /// SEC-2026-05-09 Pass-15 (HIGH-R03): per-epoch shuffle seed.
    /// Default `H256::zero()` preserves the legacy round-robin so
    /// existing tests/callers don't break; the consensus driver sets
    /// the real seed (parent block hash at epoch boundary) via
    /// `set_epoch_seed`. See `proposer_for_round` below.
    pub epoch_seed: zbx_types::H256,
}

impl ValidatorSet {
    /// SEC-2026-05-09 Pass-11 (architect-review round 4) — SAFE BFT
    /// QUORUM under DYNAMIC SHRINK.
    ///
    /// Previous formula `f = (n-1)/3; quorum = 2f+1` was correct only
    /// for `n = 3f+1` and silently produced unsafe thresholds for
    /// other cardinalities — e.g. n=3 → quorum=1 (single-validator
    /// commit!), n=4→3 (correct), n=5→3 (should be 4), n=6→3 (should
    /// be 5). Static-set chains never reached the bad cases at
    /// runtime, but Pass-11's `update_validator_set` makes them
    /// reachable via slashing-driven shrink (e.g. 4 → 3 → 2).
    ///
    /// Adopted: standard Byzantine-quorum-intersection bound
    /// `quorum = floor(2n/3) + 1`. Also guarantees:
    /// * n=1 → quorum=1 (legitimate single-validator devnet/testnet)
    /// * n=2 → quorum=2 (unanimous; safe, low liveness)
    /// * n=3 → quorum=3 (unanimous; safe, no fault tolerance)
    /// * n=4 → quorum=3 (BFT minimum; tolerates f=1)
    /// * n=5 → quorum=4 (tolerates f=1)
    /// * n=6 → quorum=5 (tolerates f=1)
    /// * n=7 → quorum=5 (tolerates f=2)
    ///
    /// Empty set (`n=0`) is rejected — proposer rotation modulo zero
    /// would panic mid-round and an empty active set is itself a
    /// liveness failure that must surface explicitly.
    pub fn new(validators: Vec<Address>) -> Self {
        let n = validators.len();
        assert!(
            n > 0,
            "ValidatorSet::new: empty active validator set — refusing \
             to construct (would panic at proposer_for_round + represent \
             a complete liveness failure that must be surfaced explicitly)"
        );
        let quorum = (2 * n) / 3 + 1;
        ValidatorSet { validators, quorum, epoch_seed: zbx_types::H256::zero() }
    }

    /// SEC-2026-05-09 Pass-15 (HIGH-R03 leader-bias): proposer
    /// rotation now uses an `epoch_seed`-keyed shuffle index instead
    /// of plain `round % n`. Pre-fix the leader for every future
    /// round was publicly knowable from genesis — an attacker could
    /// pre-compute the next 1000 leaders, target each one's network
    /// stack ahead of their slot, and force liveness failures on the
    /// proposer side at zero cost. Now `proposer_for_round` keys off
    /// `epoch_seed` (set at every epoch boundary by the consensus
    /// driver, typically the parent block hash at the epoch start)
    /// so an attacker only knows the proposer schedule for the
    /// current epoch — and can only learn the next epoch's schedule
    /// after that block is sealed.
    ///
    /// `epoch_seed = H256::zero()` (the default) cleanly degenerates
    /// to the legacy round-robin so existing callers / tests that
    /// don't bother to set the seed get exactly the old behaviour.
    pub fn proposer_for_round(&self, round: u64) -> Address {
        let n = self.validators.len();
        if self.epoch_seed == zbx_types::H256::zero() {
            return self.validators[(round as usize) % n];
        }
        // keccak256(epoch_seed || round) % n — same primitive used by
        // `proposer::vrf_index`. Uses the workspace `zbx_crypto::keccak`
        // helper that the rest of the consensus crate already depends on.
        let mut input = [0u8; 40];
        input[..32].copy_from_slice(&self.epoch_seed.0);
        input[32..40].copy_from_slice(&round.to_be_bytes());
        let h = zbx_crypto::keccak::keccak256(&input);
        let idx_raw = u64::from_be_bytes(h.0[..8].try_into().expect("32-byte hash"));
        self.validators[(idx_raw as usize) % n]
    }

    /// SEC-2026-05-09 Pass-15 (HIGH-R03): set per-epoch shuffle seed.
    /// Called by the consensus driver at every epoch boundary with
    /// the parent block hash at the epoch's first round.
    pub fn set_epoch_seed(&mut self, seed: zbx_types::H256) {
        self.epoch_seed = seed;
    }

    pub fn contains(&self, addr: &Address) -> bool {
        self.validators.contains(addr)
    }
}

/// The main HotStuff consensus driver.
pub struct HotStuffConsensus {
    pub my_address: Address,
    pub validator_set: ValidatorSet,
    pub safety_rules: SafetyRules,
    pub pacemaker: Pacemaker,
    pub block_store: BlockStore,
    /// Vote accumulators indexed by (round, phase).
    vote_accumulators: HashMap<(u64, u8), VoteAccumulator>,
    /// Highest committed block.
    pub committed_height: u64,
    /// Current highest QC we know about.
    pub highest_qc: Option<QuorumCertificate>,
    /// SEC-2026-05-09 Pass-10 (architect-review follow-up) — REMOTE
    /// equivocation detector. Per (round, phase, validator) record of
    /// the first vote seen, retained verbatim so we can build slashable
    /// `EquivocationEvidence` when a second vote on a different block
    /// hash arrives. Bounded by `prune_seen_votes_below(committed)` —
    /// the node-level commit hook MUST call this on every commit, or
    /// the map grows unbounded.
    seen_votes: HashMap<(u64, u8, Address), (H256, Vote, zbx_crypto::bls::BlsPubKey)>,
    /// SEC-2026-05-09 Pass-10 (architect-review follow-up #3) —
    /// canonical validator-address → BLS pubkey registry. The
    /// equivocation detector and accumulator both receive a pubkey
    /// alongside each `Vote`, but without binding `vote.voter` to a
    /// known pubkey an attacker can sign with their own key while
    /// setting `vote.voter = victim_addr`; the BLS verify trivially
    /// passes (because the supplied pubkey matches the supplied sig)
    /// and the detector is poisoned for the victim.
    /// `register_validator_pubkey` populates this; `on_vote` rejects
    /// any vote whose `vote.voter` is unregistered or whose supplied
    /// pubkey doesn't match the registered one.
    validator_pubkeys: HashMap<Address, zbx_crypto::bls::BlsPubKey>,
    /// SEC-2026-05-09 Pass-10 (architect-review follow-up #4) —
    /// observability counters for vote drops in `on_vote`. Silent
    /// drops on registry-mismatch / unregistered / invalid-sig were
    /// flagged as a liveness risk because misconfiguration could
    /// suppress honest votes without operator visibility. Operators
    /// scrape these via `dropped_vote_counters()` and graph against
    /// `equivocations_total`. Cheap u64 atomics in struct; not on hot
    /// path beyond what metric scraping does.
    pub dropped_unregistered:    u64,
    pub dropped_pubkey_mismatch: u64,
    pub dropped_invalid_sig:     u64,
    pub dropped_non_validator:   u64,
}

impl HotStuffConsensus {
    pub fn new(
        my_address: Address,
        safety_rules: SafetyRules,
        validator_set: ValidatorSet,
    ) -> Self {
        HotStuffConsensus {
            my_address,
            validator_set,
            safety_rules,
            pacemaker: Pacemaker::new(Default::default()),
            block_store: BlockStore::new(1024),
            vote_accumulators: HashMap::new(),
            committed_height: 0,
            highest_qc: None,
            seen_votes: HashMap::new(),
            validator_pubkeys: HashMap::new(),
            dropped_unregistered:    0,
            dropped_pubkey_mismatch: 0,
            dropped_invalid_sig:     0,
            dropped_non_validator:   0,
        }
    }

    /// SEC-2026-05-09 Pass-11 (architect-review round 3) — DYNAMIC
    /// ACTIVE-SET HOT-SWAP. Replace the consensus validator set with
    /// a freshly-computed list (e.g. configured set minus jailed
    /// addresses). Recomputes `quorum = floor(2n/3) + 1` (safe BFT
    /// bound — see `ValidatorSet::new` doc comment). Called at every epoch
    /// boundary by the node-layer driver after the staking layer
    /// flips slashed validators to `Jailed`. Without this hook, a
    /// jailed validator continues to count toward quorum + retains
    /// proposer slots inside the same process — burns are then
    /// economically real but enforcement is one-epoch-late.
    ///
    /// Pubkeys are NOT removed from the registry: a jailed validator
    /// failing the membership check above is dropped at `on_vote` /
    /// `on_proposal` BEFORE the registry is consulted, and keeping
    /// the registry stable avoids the false-positive
    /// `dropped_unregistered` spike if the same validator is later
    /// unjailed.
    pub fn update_validator_set(&mut self, addrs: Vec<Address>) {
        // SEC-2026-05-09 Pass-15 architect-review (HIGH-R03 wiring):
        // preserve the active `epoch_seed` across hot-swaps so the
        // keccak-keyed proposer rotation isn't silently demoted to
        // legacy round-robin (the predictable, DoS-targetable path).
        let preserved_seed = self.validator_set.epoch_seed;
        let mut new_set = ValidatorSet::new(addrs);
        new_set.epoch_seed = preserved_seed;
        info!(
            validators = new_set.validators.len(),
            quorum = new_set.quorum,
            "HotStuff: validator_set hot-swapped (slashing-driven; epoch_seed preserved)"
        );
        self.validator_set = new_set;
    }

    /// SEC-2026-05-09 Pass-15 architect-review (HIGH-R03 wiring):
    /// re-key the proposer rotation at every epoch boundary. Callers
    /// (driver / commit path) MUST invoke this with a high-entropy seed
    /// (e.g. `keccak256(parent_block_hash || epoch_number)`) on every
    /// epoch transition. Falling back to `H256::zero()` re-enables the
    /// predictable round-robin path that HIGH-R03 closed.
    pub fn rotate_epoch_seed(&mut self, seed: zbx_types::H256) {
        self.validator_set.set_epoch_seed(seed);
    }

    /// SEC-2026-05-09 Pass-10 (architect-review follow-up #4) —
    /// snapshot of the four observability counters. Returned tuple is
    /// `(unregistered, pubkey_mismatch, invalid_sig, non_validator)`.
    /// Operators / Prometheus exporter use this to alert on
    /// configuration drift (sustained `unregistered` or `mismatch`
    /// growth means the registry doesn't match the active validator
    /// set — a real liveness risk, not a security failure).
    pub fn dropped_vote_counters(&self) -> (u64, u64, u64, u64) {
        (
            self.dropped_unregistered,
            self.dropped_pubkey_mismatch,
            self.dropped_invalid_sig,
            self.dropped_non_validator,
        )
    }

    /// SEC-2026-05-09 Pass-10 (architect-review follow-up #4) —
    /// startup invariant. Asserts that every member of the validator
    /// set has a registered pubkey. Returns the missing addresses so
    /// the caller can fail-fast at node init. Calling this in steady
    /// state catches operator pubkey-rotation drift before it
    /// silently brownouts the chain.
    pub fn check_pubkey_registry_invariant(&self) -> Vec<Address> {
        self.validator_set
            .validators
            .iter()
            .filter(|a| !self.validator_pubkeys.contains_key(a))
            .copied()
            .collect()
    }

    /// SEC-2026-05-09 Pass-10 (architect-review follow-up #3) —
    /// register the canonical BLS pubkey for a validator address. MUST
    /// be called for every member of the validator set before any
    /// vote ingestion, otherwise `on_vote` will drop their votes (no
    /// auth basis). Re-registering the same address with a different
    /// pubkey is rejected as a programming error to prevent silent
    /// pubkey rotation that would defeat the binding check.
    pub fn register_validator_pubkey(
        &mut self,
        addr: Address,
        pk:   zbx_crypto::bls::BlsPubKey,
    ) {
        if let Some(existing) = self.validator_pubkeys.get(&addr) {
            if existing.0 != pk.0 {
                tracing::error!(
                    validator = ?addr,
                    "register_validator_pubkey: refusing to overwrite \
                     existing pubkey — programming error or attempted \
                     pubkey rotation"
                );
                return;
            }
        }
        self.validator_pubkeys.insert(addr, pk);
    }

    /// SEC-2026-05-09 Pass-10 — drop remote-equivocation detector
    /// entries below the latest committed round. The node commit hook
    /// MUST call this; otherwise the map grows unbounded over the
    /// chain's lifetime.
    pub fn prune_seen_votes_below(&mut self, committed_round: u64) {
        self.seen_votes.retain(|(round, _, _), _| *round >= committed_round);
    }

    /// SEC-2026-05-09 Pass-10 — read-only accessor for tests/telemetry.
    pub fn seen_votes_len(&self) -> usize {
        self.seen_votes.len()
    }

    /// SEC-2026-05-09 Pass-10 — assemble a verified
    /// `EquivocationEvidence` from a freshly-raised
    /// `RemoteEquivocation` error and the new vote. Returns `None`
    /// when the cached prior vote is missing or the evidence fails
    /// re-verification (which would indicate a fabricated report).
    pub fn build_remote_equivocation_evidence(
        &self,
        validator: Address,
        round:     u64,
        phase:     u8,
        new_vote:  &Vote,
        new_pk:    &zbx_crypto::bls::BlsPubKey,
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

    pub fn current_round(&self) -> u64 {
        self.pacemaker.current_round()
    }

    /// Process an inbound block proposal from the network.
    ///
    /// ## CSN-QC-01 fix (2026-05-05) — QC signature verification before acceptance
    ///
    /// The previous implementation added the block to the block store and passed
    /// `parent_qc` to `SafetyRules::vote()` without first verifying the QC's
    /// aggregated BLS signature.  A Byzantine leader could craft a proposal
    /// carrying a forged QC with an all-zero `agg_signature`; the node would
    /// store the block and cast a vote, potentially progressing to commit a block
    /// that no quorum ever actually certified.
    ///
    /// Fix: call `parent_qc.verify()` before touching the block store.  An
    /// invalid QC returns `ConsensusError::InvalidQC` and the proposal is
    /// discarded without any state mutation.
    pub fn on_proposal(
        &mut self,
        block: Block,
        parent_qc: QuorumCertificate,
    ) -> Result<Vec<ConsensusEvent>, ConsensusError> {
        // CSN-QC-01: reject proposals whose justify QC carries an invalid
        // aggregated BLS signature.  Must run BEFORE any state mutation.
        if !parent_qc.verify() {
            warn!(
                round = block.number(),
                "on_proposal: invalid parent_qc BLS signature — proposal rejected"
            );
            return Err(ConsensusError::InvalidQC(block.number()));
        }

        let round = block.number();
        let proposer = self.validator_set.proposer_for_round(round);
        if block.coinbase() != proposer {
            return Err(ConsensusError::InvalidProposer(round));
        }
        if !self.validator_set.contains(&proposer) {
            return Err(ConsensusError::InvalidProposer(round));
        }
        self.block_store.add(block.clone());
        let vote_data = VoteData {
            block_hash: block.hash(),
            block_number: round,
            phase: Phase::Prepare as u8,
            epoch: block.header.epoch,
        };
        let vote = self.safety_rules.vote(vote_data, &parent_qc)?;
        info!(round, "cast Prepare vote");
        Ok(vec![ConsensusEvent::VoteCast(vote)])
    }

    /// Process an inbound vote.
    pub fn on_vote(
        &mut self,
        vote: Vote,
        voter_pubkey: zbx_crypto::bls::BlsPubKey,
    ) -> Result<Vec<ConsensusEvent>, ConsensusError> {
        if !self.validator_set.contains(&vote.voter) {
            self.dropped_non_validator = self.dropped_non_validator.saturating_add(1);
            return Ok(Vec::new()); // ignore non-validator vote
        }

        // SEC-2026-05-09 Pass-10 (architect-review follow-up #3) —
        // VOTER↔PUBKEY BINDING. `verify_single(sig, supplied_pk, msg)`
        // only proves the *supplied* pubkey signed `msg`; it does NOT
        // prove that pubkey belongs to the address in `vote.voter`.
        // Without this lookup an attacker could sign with their own
        // key while setting `vote.voter = victim`, sail through BLS
        // verify, and poison the detector. We require an explicit
        // registry entry per validator address.
        match self.validator_pubkeys.get(&vote.voter) {
            None => {
                self.dropped_unregistered = self.dropped_unregistered.saturating_add(1);
                tracing::warn!(
                    validator = ?vote.voter,
                    counter   = self.dropped_unregistered,
                    "on_vote: dropping vote — no registered pubkey for voter \
                     (possible config drift / pubkey-rotation lag — operator \
                     should audit registry vs active validator set)"
                );
                return Ok(Vec::new());
            }
            Some(registered) if registered.0 != voter_pubkey.0 => {
                self.dropped_pubkey_mismatch = self.dropped_pubkey_mismatch.saturating_add(1);
                tracing::warn!(
                    validator = ?vote.voter,
                    counter   = self.dropped_pubkey_mismatch,
                    "on_vote: dropping vote — supplied pubkey does not \
                     match registry (possible voter/pubkey-mismatch \
                     poisoning attempt OR genuine pubkey rotation)"
                );
                return Ok(Vec::new());
            }
            _ => {} // registered & matches → proceed
        }

        // SEC-2026-05-09 Pass-10 (architect-review follow-up #2) —
        // VERIFY THE BLS SIGNATURE BEFORE TOUCHING `seen_votes`.
        //
        // The accumulator validates signatures internally, but we
        // detect equivocation BEFORE the accumulator runs, so without
        // this check an attacker could spam unsigned/forged votes for
        // an honest validator's address — they would poison
        // `seen_votes` first, then the honest validator's real vote
        // would falsely raise `RemoteEquivocation` and be dropped
        // (vote suppression / DoS / false slashing-signal spam).
        let msg = zbx_crypto::keccak::keccak256(&vote.data.signing_bytes());
        if !zbx_crypto::bls::verify_single(&vote.signature, &voter_pubkey, &msg) {
            // Drop — invalid signature is not a slashable event and
            // must NOT pollute the equivocation detector. Counted +
            // logged so a sustained spike alerts operators (not just
            // silently absorbed).
            self.dropped_invalid_sig = self.dropped_invalid_sig.saturating_add(1);
            tracing::warn!(
                validator = ?vote.voter,
                counter   = self.dropped_invalid_sig,
                "on_vote: dropping vote — BLS signature did not verify"
            );
            return Ok(Vec::new());
        }

        // SEC-2026-05-09 Pass-10 (architect-review follow-up) — REMOTE
        // equivocation detection. Index by (round, phase, validator);
        // raise `ConsensusError::RemoteEquivocation` if the same
        // validator already signed a different block hash for the same
        // round/phase. The accumulator below is keyed only by
        // (round, phase) so a divergent vote would otherwise be
        // silently ignored by the `vote.data != self.data` arm in
        // `VoteAccumulator::add_vote`.
        let ek = (vote.data.block_number, vote.data.phase, vote.voter);
        if let Some((prev_hash, prev_vote, _prev_pk)) = self.seen_votes.get(&ek) {
            if *prev_hash != vote.data.block_hash {
                let prev = prev_vote.clone();
                warn!(
                    validator = ?vote.voter,
                    round = vote.data.block_number,
                    phase = vote.data.phase,
                    hash_a = ?prev_hash,
                    hash_b = ?vote.data.block_hash,
                    "HotStuff: REMOTE equivocation detected"
                );
                return Err(ConsensusError::RemoteEquivocation {
                    validator: prev.voter,
                    round:     prev.data.block_number,
                    phase:     prev.data.phase,
                    hash_a:    prev.data.block_hash,
                    hash_b:    vote.data.block_hash,
                });
            }
            // Same hash redelivered — fall through (idempotent
            // accumulator handling will reject the duplicate by addr).
        } else {
            self.seen_votes.insert(
                ek,
                (vote.data.block_hash, vote.clone(), voter_pubkey.clone()),
            );
        }

        let key = (vote.data.block_number, vote.data.phase);
        let quorum = self.validator_set.quorum;
        let acc = self.vote_accumulators.entry(key).or_insert_with(|| {
            VoteAccumulator::new(vote.data.clone(), quorum)
        });
        if let Some(qc) = acc.add_vote(vote, voter_pubkey)? {
            return self.on_qc(qc);
        }
        Ok(Vec::new())
    }

    /// Handle a newly formed QC, potentially advancing phases or committing.
    fn on_qc(
        &mut self,
        qc: QuorumCertificate,
    ) -> Result<Vec<ConsensusEvent>, ConsensusError> {
        let mut events = Vec::new();
        match qc.phase() {
            0 => {
                // Prepare QC → advance to PreCommit
                debug!(round = qc.block_number(), "Prepare QC formed");
                self.safety_rules.update_locked_qc(qc.clone());
                self.highest_qc = Some(qc.clone());
                // Trigger PreCommit phase vote
                let vote_data = VoteData {
                    block_hash: *qc.block_hash(),
                    block_number: qc.block_number(),
                    phase: Phase::PreCommit as u8,
                    epoch: self.pacemaker.current_epoch(),
                };
                if let Ok(v) = self.safety_rules.vote(vote_data, &qc) {
                    events.push(ConsensusEvent::VoteCast(v));
                }
            }
            1 => {
                // PreCommit QC → advance to Commit
                debug!(round = qc.block_number(), "PreCommit QC formed");
                let vote_data = VoteData {
                    block_hash: *qc.block_hash(),
                    block_number: qc.block_number(),
                    phase: Phase::Commit as u8,
                    epoch: self.pacemaker.current_epoch(),
                };
                if let Ok(v) = self.safety_rules.vote(vote_data, &qc) {
                    events.push(ConsensusEvent::VoteCast(v));
                }
            }
            2 => {
                // Commit QC → DECIDE: block is final
                let block_hash = *qc.block_hash();
                if let Some(block) = self.block_store.get(&block_hash).cloned() {
                    if block.number() > self.committed_height {
                        self.committed_height = block.number();
                        info!(height = block.number(), "BLOCK COMMITTED");
                        self.pacemaker.advance_round(block.number() + 1, qc.vote_data.epoch);
                        events.push(ConsensusEvent::Committed { block, qc });
                    }
                }
            }
            _ => {
                // N-05 fix (2026-05-05): The previous `_ => {}` catch-all
                // silently swallowed any QC with an unrecognised phase byte,
                // making malformed or replayed QCs invisible in audit logs and
                // preventing operators from detecting protocol violations.
                //
                // Any phase value outside {0=Prepare, 1=PreCommit, 2=Commit}
                // is illegal by the HotStuff-BFT spec.  Return an explicit
                // error so the caller can log, metric, and (if persistent)
                // penalise the peer that sent it.
                warn!(
                    phase = qc.vote_data.phase,
                    round = qc.block_number(),
                    "on_qc: received QC with unknown phase — rejecting"
                );
                return Err(ConsensusError::InvalidMessage(format!(
                    "unknown QC phase {} for round {}",
                    qc.vote_data.phase,
                    qc.block_number()
                )));
            }
        }
        Ok(events)
    }

    /// Called by the pacemaker timer — produce a timeout vote.
    pub fn on_timeout(&mut self) -> ConsensusEvent {
        let timed_out = self.pacemaker.on_timeout();
        warn!(round = timed_out, "consensus timeout");
        ConsensusEvent::Timeout(timed_out)
    }
}