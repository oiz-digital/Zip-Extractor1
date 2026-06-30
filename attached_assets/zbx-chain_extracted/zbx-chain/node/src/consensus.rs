//! Multi-validator HotStuff-BFT consensus driver.
//!
//! Wires `zbx-consensus::HotStuffConsensus` into the node lifecycle,
//! replacing the single-validator tick with a full three-phase BFT round.
//!
//! ## Architecture
//!
//! ```text
//! ConsensusDriver
//!   ├─ HotStuffConsensus  — state machine (vote accumulation, phase transitions)
//!   ├─ SafetyRules        — per-(round, phase) equivocation guard (BLS signing)
//!   ├─ SlashingDetector   — per-block liveness tracking → instant-jail on miss
//!   ├─ pending_blocks     — uncommitted candidate blocks keyed by block_hash
//!   └─ vote_tx            — broadcast::Sender for in-process vote propagation
//!                           (wire P2P gossip here for multi-node deployments)
//! ```
//!
//! ## Single-validator operation (quorum = 1)
//!
//! When the validator set has exactly one member, `ValidatorSet::quorum = 1`.
//! The driver casts a Prepare vote; since quorum=1, that immediately forms a
//! Prepare QC which triggers a PreCommit vote (also quorum-satisfying
//! immediately), and so on through to the Commit QC and the `Committed` event.
//! A block is committed in a single tick without any network round-trip.
//! (The S35 C-12 fix in `safety_rules.rs` is required for this — it changes
//! the staleness guard from `<=` to `<` so a validator can vote in all three
//! phases for the same block_number.)
//!
//! ## Multi-validator operation
//!
//! When `validator_set.len() > 1`:
//! * The proposer for round r is `validators[r % len]`.
//! * Votes are broadcast through `vote_tx` (a tokio `broadcast` channel).
//!   In a multi-node deployment, inject remote votes via `inject_vote`.
//! * `wait_for_commit` returns after `ROUND_TIMEOUT_MS` if quorum is not
//!   reached, and the pacemaker advances to the next round.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use serde_json::{json, Value};
use tokio::sync::{broadcast, watch};
use tracing::{debug, error, info, warn};

use zbx_consensus::error::ConsensusError;
use zbx_consensus::hotstuff::{ConsensusEvent, Phase, ValidatorSet as HsValidatorSet};
use zbx_consensus::{HotStuffConsensus, QuorumCertificate, SafetyRules, Vote, VoteData};
use zbx_metrics::ConsensusMetrics;
use zbx_crypto::bls::{BlsPrivKey, BlsPubKey, BlsSignature};
use zbx_mempool::TransactionPool;
use zbx_staking::validator::{Validator, ValidatorSet, ValidatorStatus};
use zbx_staking::{SlashingDetector, SlashingPipeline};
use zbx_storage::ZbxDb;
use zbx_types::{address::Address, block::Block, H256};

use crate::network::NetworkServer;

use crate::block_producer::{build_candidate, execute_and_commit, ProducerConfig};

/// Broadcast channel capacity for in-process vote propagation.
const VOTE_CHAN_CAP: usize = 256;

/// Maximum time to wait for quorum votes in one round before timing out.
const ROUND_TIMEOUT_MS: u64 = 4_000;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// BLS key material for a consensus participant.
pub struct ValidatorKey {
    pub address: Address,
    pub bls_priv: BlsPrivKey,
}

/// Full configuration for the multi-validator consensus driver.
pub struct ConsensusConfig {
    /// This node's identity and BLS signing key.
    pub my_key: ValidatorKey,
    /// Ordered active validator set (address, BLS public key).
    /// Proposer for round r = validators[r % len].
    pub validators: Vec<(Address, BlsPubKey)>,
    /// Block production parameters.
    pub producer_cfg: ProducerConfig,
    /// Epoch length in blocks (for slashing epoch calculation).
    pub epoch_length: u64,
    /// SEC-2026-05-09 Pass-19 (Task #9) — initial epoch seed for the
    /// proposer-shuffle. Derived from `GenesisBuilder::genesis_epoch_seed()`
    /// (= `keccak256(genesis_block_hash || chain_id_be8)`) and supplied
    /// at startup so epoch-0 already uses the keccak-keyed shuffle
    /// rather than the predictable `round % n` legacy fallback.
    /// `None` is accepted for tests / single-validator dev modes —
    /// they keep the legacy round-robin behaviour, matching the
    /// `legacy_fallback_only_on_zero_seed` regression test.
    pub genesis_epoch_seed: Option<H256>,
}

/// Message broadcast between local consensus participants / P2P.
#[derive(Clone, Debug)]
pub struct InboundVote {
    pub vote: Vote,
    pub pubkey: BlsPubKey,
}

/// The async multi-validator HotStuff consensus driver.
pub struct ConsensusDriver {
    cfg: ConsensusConfig,
    consensus: HotStuffConsensus,
    storage: Arc<ZbxDb>,
    mempool: Arc<RwLock<TransactionPool>>,
    slashing: SlashingDetector,
    /// Candidate blocks built by this proposer, awaiting commit.
    pending_blocks: HashMap<H256, Block>,
    /// Broadcast channel for vote propagation.
    vote_tx: broadcast::Sender<InboundVote>,
    /// Shared WebSocket broadcast channel — populated on each committed block
    /// so that `eth_subscribe "newHeads"` subscribers receive live updates.
    new_head_tx: Option<Arc<broadcast::Sender<Value>>>,
    /// RPC-layer ValidatorSet — written at epoch boundaries so that
    /// `zbx_getValidatorSet` / `zbx_getStakingInfo` return live data.
    rpc_validator_set: Option<Arc<RwLock<ValidatorSet>>>,
    /// P2P network server — used to broadcast committed blocks and self-cast
    /// votes to connected TCP peers (multi-validator propagation).
    network: Option<Arc<NetworkServer>>,
    /// Optional consensus-metrics handle — bumps `equivocations_total` etc.
    /// (Pass-10 wiring.) `None` for tests / single-binary modes that don't
    /// run the metrics server.
    metrics: Option<ConsensusMetrics>,
    /// SEC-2026-05-09 Pass-11 — end-to-end slashing pipeline.
    /// `None` for tests / single-validator dev modes; in production
    /// the node assembles this with `EvidenceStore` + the shared
    /// `SlashingRegistryV2` + the shared `ValidatorSet`. When set,
    /// `wait_for_commit` ingests verified `EquivocationEvidence`
    /// straight to the pipeline and `do_commit` calls
    /// `tick_finalize` once per block.
    slashing_pipeline: Option<SlashingPipeline>,
}

impl ConsensusDriver {
    /// Create a new driver and return it alongside a vote channel receiver.
    ///
    /// Clone the returned receiver to give to P2P handlers so they can inject
    /// remote validator votes via `inject_vote`.
    pub fn new(
        cfg: ConsensusConfig,
        storage: Arc<ZbxDb>,
        mempool: Arc<RwLock<TransactionPool>>,
        new_head_tx: Option<Arc<broadcast::Sender<Value>>>,
        rpc_validator_set: Option<Arc<RwLock<ValidatorSet>>>,
    ) -> (Self, broadcast::Receiver<InboundVote>) {
        let validator_addrs: Vec<Address> = cfg.validators.iter().map(|(a, _)| *a).collect();
        let hs_valset = HsValidatorSet::new(validator_addrs);

        let safety = SafetyRules::new(cfg.my_key.bls_priv.clone(), cfg.my_key.address);
        let mut consensus = HotStuffConsensus::new(cfg.my_key.address, safety, hs_valset);

        // SEC-2026-05-09 Pass-19 (Task #9, architect-review follow-up #2):
        // BOOTSTRAP THE EPOCH-0 SHUFFLE SEED. Without this, every node
        // starts with `epoch_seed == H256::zero()` → legacy round-
        // robin proposer selection → the entire first epoch's leader
        // schedule is publicly predictable from genesis. The seed is
        // derived deterministically (`keccak256(genesis_block_hash ||
        // chain_id_be8)`) so every honest node bootstraps to the same
        // value without any out-of-band ceremony.
        if let Some(seed) = cfg.genesis_epoch_seed {
            if seed != H256::zero() {
                consensus.rotate_epoch_seed(seed);
                tracing::info!(?seed, "consensus driver: bootstrapped epoch-0 shuffle seed from genesis");
            }
        } else {
            tracing::warn!(
                "consensus driver: no genesis_epoch_seed supplied — \
                 epoch-0 proposer schedule will use legacy round-robin \
                 (acceptable for tests / single-validator dev only)"
            );
        }
        // SEC-2026-05-09 Pass-10 (architect-review follow-up #3) —
        // populate the canonical address→pubkey registry. Without this
        // every inbound vote would be dropped (no auth basis), and the
        // remote-equivocation detector would also be disabled. Source
        // of truth is `cfg.validators` which the node assembles from
        // genesis / staking-registry state at startup.
        for (addr, pk) in cfg.validators.iter() {
            consensus.register_validator_pubkey(*addr, pk.clone());
        }
        // SEC-2026-05-09 Pass-10 (architect-review follow-up #4) —
        // STARTUP INVARIANT. Every validator-set member must have a
        // registered pubkey. If not, fail-fast at node init rather
        // than silently brown out the chain via dropped honest votes.
        let missing = consensus.check_pubkey_registry_invariant();
        if !missing.is_empty() {
            panic!(
                "consensus invariant violation: {} validator(s) in the active \
                 set have no registered BLS pubkey: {:?}. Refusing to start — \
                 fix the genesis / staking-registry config.",
                missing.len(), missing
            );
        }

        let (vote_tx, vote_rx) = broadcast::channel(VOTE_CHAN_CAP);

        let driver = ConsensusDriver {
            cfg,
            consensus,
            storage,
            mempool,
            slashing: SlashingDetector::new(),
            pending_blocks: HashMap::new(),
            vote_tx,
            new_head_tx,
            rpc_validator_set,
            network: None,
            metrics: None,
            slashing_pipeline: None,
        };
        (driver, vote_rx)
    }

    /// Wire the P2P network server into this driver so committed blocks and
    /// self-cast votes are broadcast to connected peers.
    pub fn set_network(&mut self, net: Arc<NetworkServer>) {
        self.network = Some(net);
    }

    /// Wire the consensus metrics handle (Pass-10). Optional — driver
    /// degrades gracefully if metrics aren't running.
    pub fn set_metrics(&mut self, m: ConsensusMetrics) {
        self.metrics = Some(m);
    }

    /// SEC-2026-05-09 Pass-11 — wire the end-to-end slashing pipeline.
    ///
    /// Optional. When set:
    /// * Verified remote-equivocation evidence in `wait_for_commit`
    ///   is persisted + submitted to `SlashingRegistryV2` (rather than
    ///   only logged as "SLASHABLE" + lost on restart).
    /// * `do_commit` runs `tick_finalize(height)` once per block to
    ///   debit stake from validators whose appeal window has closed.
    pub fn set_slashing_pipeline(&mut self, pipeline: SlashingPipeline) {
        self.slashing_pipeline = Some(pipeline);
    }

    /// Clone the internal vote broadcast sender so the P2P layer can inject
    /// remote votes into the consensus state machine.
    pub fn vote_sender(&self) -> broadcast::Sender<InboundVote> {
        self.vote_tx.clone()
    }

    /// Inject a vote received from a remote validator (called by P2P handler).
    pub fn inject_vote(&mut self, vote: Vote, pubkey: BlsPubKey) {
        let _ = self.vote_tx.send(InboundVote { vote, pubkey });
    }

    /// Run the consensus loop until shutdown is signalled.
    pub async fn run(mut self, mut shutdown_rx: watch::Receiver<bool>) {
        let block_time = self.cfg.producer_cfg.block_time;
        let mut tick = tokio::time::interval(block_time);
        tick.tick().await; // skip the initial immediate tick

        let mut vote_rx = self.vote_tx.subscribe();

        info!(
            validators = self.cfg.validators.len(),
            quorum = self.consensus.validator_set.quorum,
            my_address = %hex::encode(self.cfg.my_key.address.as_bytes()),
            "HotStuff consensus driver started"
        );

        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if let Err(e) = self.run_round(&mut vote_rx).await {
                        error!(error = %e, "HotStuff round error");
                    }
                }
                _ = shutdown_rx.changed() => {
                    info!("consensus driver: shutdown signal received");
                    return;
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Core round logic
    // -----------------------------------------------------------------------

    async fn run_round(
        &mut self,
        vote_rx: &mut broadcast::Receiver<InboundVote>,
    ) -> Result<(), String> {
        let round = self.consensus.current_round();
        let proposer = self.consensus.validator_set.proposer_for_round(round);
        let is_proposer = proposer == self.cfg.my_key.address;

        if !is_proposer {
            debug!(round, "HotStuff: not the proposer this round — waiting for proposal");
            return Ok(());
        }

        // We are the proposer — build a candidate block.
        let mut block = match build_candidate(&self.storage, &self.mempool, &self.cfg.producer_cfg)
            .map_err(|e| format!("build_candidate: {e}"))?
        {
            Some(b) => b,
            None => {
                debug!(round, "HotStuff: no eligible txs — skipping round");
                return Ok(());
            }
        };

        // SEC-2026-05-09 Pass-19 (Task #9): on the first block of every
        // new epoch (height % epoch_length == 0 && height > 0), record
        // the freshly-rotated epoch seed in the header so light clients
        // can independently verify the proposer schedule across the
        // epoch transition. The seed itself was rotated at the END of
        // the previous block's `do_commit` (see the matching trigger at
        // `(height+1) % epoch_length == 0`), so by the time we propose
        // the new epoch's first block, `validator_set.epoch_seed`
        // already holds the new value. `H256::zero()` (legacy round-
        // robin) is intentionally NOT recorded — the field stays `None`
        // until a real seed is in place, preserving the existing
        // genesis-era behaviour and avoiding a misleading "I have a
        // seed" signal to light clients.
        let h = block.header.number;
        if h > 0 && h % self.cfg.epoch_length == 0 {
            let seed = self.consensus.validator_set.epoch_seed;
            if seed != H256::zero() {
                block.header.epoch_seed = Some(seed);
            }
        }

        info!(
            round,
            height = block.header.number,
            "HotStuff: proposer building candidate block"
        );

        let block_hash = block.hash();
        self.pending_blocks.insert(block_hash, block.clone());

        let parent_qc = self.parent_qc_for(&block);

        // Feed into HotStuff state machine — casts our Prepare vote.
        //
        // SEC-2026-05-09 Pass-10: previously every error was stringified
        // via `format!("{e:?}")`, which silently dropped the high-signal
        // `Equivocation` variant. The Pass-5 H3 guard fires when our
        // local node is *about to* double-vote (a defence-in-depth check
        // against a corrupt safety_rules state) — that event is exactly
        // the on-chain slashing trigger we need to surface to ops. We
        // bump a Prometheus counter here and route the error through.
        //
        // NOTE: this `on_proposal` arm guards against OUR OWN
        // equivocation. The REMOTE-validator equivocation detector is
        // wired into the `process_events` / vote-ingestion path below
        // (`ConsensusError::RemoteEquivocation` → metric bump + log,
        // SEC-2026-05-09 Pass-10 architect-review follow-up). The
        // end-to-end slashing pipeline (evidence → registry →
        // stake-burn) remains an open HARD blocker — see
        // `docs/SUBSYSTEM-MATURITY-AUDIT-2026-05-09.md`.
        let initial_events = match self.consensus.on_proposal(block, parent_qc) {
            Ok(evs) => evs,
            Err(ConsensusError::Equivocation { round: r, seen, attempted }) => {
                if let Some(m) = &self.metrics {
                    m.equivocations_total.inc();
                }
                error!(
                    round = r,
                    seen = ?seen,
                    attempted = ?attempted,
                    "HotStuff: SELF-equivocation guard fired — refusing to double-vote. \
                     This indicates a corrupt safety_rules state — investigate immediately."
                );
                return Err(format!("self-equivocation guard fired at round {r}"));
            }
            Err(e) => return Err(format!("on_proposal: {e:?}")),
        };

        // Process the event queue iteratively (no async recursion).
        let committed = self.process_events(initial_events)?;

        // For multi-validator scenarios, if Prepare vote didn't immediately
        // reach quorum, wait for remote votes with a timeout.
        let committed = if committed.is_none() && self.consensus.validator_set.quorum > 1 {
            self.wait_for_commit(vote_rx).await
        } else {
            committed
        };

        if let Some(b) = committed {
            self.do_commit(b)?;
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Event processing — iterative (no async recursion)
    // -----------------------------------------------------------------------

    /// Process a batch of ConsensusEvents using an iterative queue.
    ///
    /// For each `VoteCast` event, broadcasts the vote through `vote_tx` and
    /// feeds it back into `on_vote`, appending the resulting events to the
    /// queue.  Returns `Some(Block)` if a `Committed` event is reached.
    fn process_events(
        &mut self,
        initial: Vec<ConsensusEvent>,
    ) -> Result<Option<Block>, String> {
        let mut queue: VecDeque<ConsensusEvent> = initial.into();
        while let Some(ev) = queue.pop_front() {
            match ev {
                ConsensusEvent::VoteCast(vote) => {
                    let pubkey = self.cfg.my_key.bls_priv.to_pubkey();
                    // Broadcast so remote validators (or our own vote_rx) receive it.
                    let _ = self.vote_tx.send(InboundVote {
                        vote: vote.clone(),
                        pubkey: pubkey.clone(),
                    });
                    // Propagate our vote to connected TCP peers (multi-validator).
                    if let Some(net) = &self.network {
                        net.broadcast_vote(&vote);
                    }
                    // Feed our vote back into the state machine immediately.
                    match self.consensus.on_vote(vote, pubkey) {
                        Ok(more_events) => queue.extend(more_events),
                        Err(ConsensusError::RemoteEquivocation {
                            validator, round, phase, hash_a, hash_b,
                        }) => {
                            // SEC-2026-05-09 Pass-10 — should not occur
                            // on the SELF-vote path, but if it does it
                            // means our own vote contradicts a prior
                            // remote vote we observed for our address
                            // — investigate immediately.
                            if let Some(m) = &self.metrics {
                                m.equivocations_total.inc();
                            }
                            error!(
                                validator = ?validator, round, phase,
                                hash_a = ?hash_a, hash_b = ?hash_b,
                                "REMOTE-equivocation raised on SELF vote — \
                                 corrupt safety state, investigate"
                            );
                        }
                        Err(e) => {
                            // Benign errors (duplicate, stale) are debug-logged.
                            debug!(error = ?e, "on_vote (self): non-fatal error");
                        }
                    }
                }
                ConsensusEvent::Committed { block, qc: _ } => {
                    return Ok(Some(block));
                }
                ConsensusEvent::Timeout(round) => {
                    warn!(round, "HotStuff: timeout event in event queue");
                }
                ConsensusEvent::ProposalRequired { round, .. } => {
                    debug!(round, "HotStuff: ProposalRequired (handled in run_round)");
                }
            }
        }
        Ok(None)
    }

    // -----------------------------------------------------------------------
    // Multi-validator vote collection
    // -----------------------------------------------------------------------

    /// Wait up to `ROUND_TIMEOUT_MS` for inbound votes to drive consensus
    /// to the `Committed` state for the current round.
    async fn wait_for_commit(
        &mut self,
        vote_rx: &mut broadcast::Receiver<InboundVote>,
    ) -> Option<Block> {
        let deadline = tokio::time::Instant::now()
            + Duration::from_millis(ROUND_TIMEOUT_MS);

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                warn!("HotStuff: round timed out waiting for quorum");
                let _ = self.consensus.on_timeout(); // advance pacemaker
                return None;
            }

            let recv_result = tokio::time::timeout(remaining, vote_rx.recv()).await;
            match recv_result {
                Ok(Ok(msg)) => {
                    let vote_for_evidence = msg.vote.clone();
                    let pk_for_evidence   = msg.pubkey.clone();
                    match self.consensus.on_vote(msg.vote, msg.pubkey) {
                        Ok(events) => {
                            match self.process_events(events) {
                                Ok(Some(b)) => return Some(b),
                                Ok(None) => {}
                                Err(e) => {
                                    error!(error = %e, "event processing error in vote-wait");
                                }
                            }
                        }
                        // SEC-2026-05-09 Pass-10 (architect-review
                        // follow-up) — REMOTE equivocation detected:
                        // same validator signed two block hashes at
                        // the same (round, phase). Build verified
                        // `EquivocationEvidence` for the slashing
                        // pipeline (currently logged + counted; the
                        // end-to-end registry → stake-burn integration
                        // is documented as an open HARD blocker in
                        // SUBSYSTEM-MATURITY-AUDIT-2026-05-09.md).
                        Err(ConsensusError::RemoteEquivocation {
                            validator, round, phase, hash_a, hash_b,
                        }) => {
                            if let Some(m) = &self.metrics {
                                m.equivocations_total.inc();
                            }
                            let evidence = self.consensus
                                .build_remote_equivocation_evidence(
                                    validator, round, phase,
                                    &vote_for_evidence, &pk_for_evidence,
                                );
                            match evidence {
                                Some(ev) => {
                                    // SEC-2026-05-09 Pass-11 — wire the
                                    // verified equivocation evidence into
                                    // the end-to-end slashing pipeline.
                                    // Pre-Pass-11 this branch only logged
                                    // "SLASHABLE" and dropped the evidence
                                    // — the chain had a detector but no
                                    // economic security. The pipeline
                                    // persists, submits to the registry,
                                    // and finalizes after `APPEAL_WINDOW`
                                    // blocks (per-block tick in do_commit).
                                    if let Some(pipeline) = &self.slashing_pipeline {
                                        // Reporter = self (the detecting
                                        // node). Whistleblower reward
                                        // accrues to our coinbase address;
                                        // bond is implicit because we
                                        // produced the evidence.
                                        let reporter = self.cfg.my_key.address;
                                        // Best-effort current block + epoch
                                        // for the registry's appeal-deadline
                                        // math. Use the round number
                                        // (== block_number for the offending
                                        // vote) as `current_block` so the
                                        // appeal window is anchored to the
                                        // event, not to wall-clock detection
                                        // time.
                                        let current_block = round;
                                        let current_epoch = round / self.cfg.epoch_length;
                                        // Offender's stake — read from the
                                        // shared validator set if available;
                                        // fall back to MIN_SELF_STAKE.
                                        let offender_stake = self.rpc_validator_set
                                            .as_ref()
                                            .and_then(|vs| {
                                                vs.read().validators.get(&validator)
                                                    .map(|v| v.self_stake + v.delegated_stake)
                                            })
                                            .unwrap_or(zbx_staking::MIN_SELF_STAKE);

                                        match pipeline.ingest_equivocation(
                                            &ev, reporter, current_block,
                                            current_epoch, offender_stake,
                                        ) {
                                            Ok(record_id) => {
                                                error!(
                                                    validator = ?validator,
                                                    round, phase,
                                                    hash_a = ?hash_a, hash_b = ?hash_b,
                                                    record_id = ?record_id,
                                                    "SLASHABLE: remote equivocation \
                                                     ingested into slashing pipeline \
                                                     — stake will burn after appeal \
                                                     window"
                                                );
                                            }
                                            Err(e) => {
                                                // SEC-2026-05-09 Pass-11
                                                // (architect-review follow-up):
                                                // FAIL-CLOSED. The only
                                                // expected non-fatal error
                                                // here is DuplicateEvidence,
                                                // which `ingest_equivocation`
                                                // already absorbs internally
                                                // and returns Ok. Anything
                                                // reaching this arm is a
                                                // BLS-verify failure (caller
                                                // bug) or a RocksDB write
                                                // failure (disk catastrophe).
                                                // Either way we MUST NOT
                                                // silently drop the evidence
                                                // — that is the same bypass
                                                // Pass-10 closed.
                                                error!(
                                                    validator = ?validator, round, phase,
                                                    error = %e,
                                                    "SLASHABLE FATAL: pipeline \
                                                     ingest failed — halting \
                                                     to prevent silent slash \
                                                     drop. Investigate before \
                                                     restart."
                                                );
                                                panic!(
                                                    "slashing pipeline ingest failed for {:?}: {e}",
                                                    validator
                                                );
                                            }
                                        }
                                    } else {
                                        // Pipeline not wired (test / dev
                                        // mode). Behave as pre-Pass-11.
                                        error!(
                                            validator = ?validator, round, phase,
                                            hash_a = ?hash_a, hash_b = ?hash_b,
                                            evidence_verified = ev.verify(),
                                            "SLASHABLE: remote equivocation \
                                             (no slashing pipeline wired — \
                                             evidence dropped)"
                                        );
                                    }
                                }
                                None => {
                                    error!(
                                        validator = ?validator, round, phase,
                                        "REMOTE-equivocation reported but evidence \
                                         could not be assembled / re-verified — \
                                         possible internal bug, investigate"
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            debug!(error = ?e, "HotStuff: on_vote error (may be benign)");
                        }
                    }
                }
                Ok(Err(_)) => break, // channel closed
                Err(_) => {
                    warn!("HotStuff: round timed out");
                    let _ = self.consensus.on_timeout();
                    return None;
                }
            }
        }
        None
    }

    // -----------------------------------------------------------------------
    // Commit path
    // -----------------------------------------------------------------------

    fn do_commit(&mut self, block: Block) -> Result<(), String> {
        let height = block.header.number;
        let epoch = height / self.cfg.epoch_length;
        let coinbase = block.header.coinbase;
        let validators: Vec<Address> = self.consensus.validator_set.validators.clone();

        // SEC-2026-05-09 Pass-19 (Task #9, architect-review follow-up #6):
        // CONSENSUS-INVALIDATING STRUCTURAL VALIDATION of `header.epoch_seed`.
        //
        // The field is intended for light clients to independently verify
        // the proposer schedule across an epoch transition. It must
        // appear ONLY on the first block of a new epoch (height %
        // epoch_length == 0 && height > 0), and when present it MUST
        // equal the locally-rotated seed. Pre-Pass-19 / pre-follow-up-#6
        // these mismatches were logged as warnings and the block was
        // accepted, which means a malicious or buggy proposer could
        // produce canonical blocks with invalid `epoch_seed` — full
        // nodes would accept (because they compute the proposer
        // schedule from `validator_set.epoch_seed`, not the header
        // field), but every light client deriving the schedule from
        // canonical headers would reject the same chain. That is a
        // light-client / full-node consensus split.
        //
        // Bootstrap exception: when the local rotated seed is still
        // `H256::zero()` (genesis_epoch_seed never supplied OR no full
        // epoch has elapsed yet), the propose path INTENTIONALLY skips
        // patching the header — so `(None, true)` is valid only while
        // `validator_set.epoch_seed == H256::zero()`. Once a real seed
        // is in place, every boundary block MUST carry it.
        validate_epoch_seed(
            height,
            block.header.epoch_seed,
            self.consensus.validator_set.epoch_seed,
            self.cfg.epoch_length,
        )?;

        // Liveness tracking: proposer = signed, all others = missed.
        let block_hash = block.hash();
        for addr in &validators {
            if *addr == coinbase {
                if let Some(ev) = self.slashing.record_vote(*addr, height, 0, block_hash) {
                    warn!(validator = ?addr, "double-sign detected at HotStuff commit");
                    let _ = ev;
                }
            } else if let Some(ev) =
                self.slashing.record_missed_block(*addr, epoch, height)
            {
                warn!(
                    validator = ?addr,
                    "instant-jail: validator missed {} consecutive blocks",
                    zbx_staking::MAX_CONSECUTIVE_MISSED
                );
                let _ = ev;
            }
        }

        // Clean up pending cache: remove committed block and any stale earlier ones.
        self.pending_blocks.remove(&block_hash);
        self.pending_blocks.retain(|_, b| b.header.number >= height);

        // SEC-2026-05-09 Pass-10 (architect-review follow-up) — bound
        // the remote-equivocation detector. Once a round is committed,
        // a late equivocating vote at that round cannot affect the
        // chain, so we drop the cached first-vote. Without this hook
        // the `seen_votes` map would grow unbounded over the chain's
        // lifetime.
        self.consensus.prune_seen_votes_below(height);

        // SEC-2026-05-09 Pass-11 — slashing pipeline tick. Finalize
        // any records whose appeal window closed at or before this
        // height: registry → Confirmed, persist, then debit
        // self_stake + jail on the shared ValidatorSet. Idempotent;
        // no-op when nothing is ready.
        if let Some(pipeline) = &self.slashing_pipeline {
            match pipeline.tick_finalize(height) {
                Ok(applied) if !applied.is_empty() => {
                    // Round-4 follow-up: bridge the staking-pipeline's
                    // metadata burn (self_stake debit + jail) into
                    // actual on-chain account state. Without this, a
                    // slashed validator would still appear to hold the
                    // full pre-slash balance to every eth_getBalance,
                    // ERC-20 transfer, and contract call. apply_slash_burns
                    // commits a single fsync'd WriteBatch; failure here
                    // is fail-closed (panic) for the same reason
                    // tick_finalize errors are: silent slash-drop is the
                    // class of bug the whole pipeline exists to prevent.
                    let burns: Vec<(zbx_types::address::Address, u128)> = applied
                        .iter()
                        .map(|a| (a.offender, a.burn_wei))
                        .collect();
                    let actual = self.storage.apply_slash_burns(&burns).unwrap_or_else(|e| {
                        error!(error = %e, block = height,
                               "FATAL: slashing on-state burn FAILED — \
                                halting to prevent silent slash drop");
                        panic!("slashing on-state burn failed at block {height}: {e}");
                    });
                    for (a, burned) in applied.iter().zip(actual.iter()) {
                        warn!(
                            offender = ?a.offender,
                            burn_wei = a.burn_wei,
                            burn_actual_wei = burned,
                            whistleblower = ?a.whistleblower,
                            whistleblower_wei = a.whistleblower_wei,
                            jailed = a.jailed,
                            block = height,
                            "slashing pipeline: finalized + stake burnt + on-state debited"
                        );
                    }
                }
                Ok(_) => {} // nothing ready this block
                Err(e) => {
                    // SEC-2026-05-09 Pass-11 (architect-review
                    // follow-up #2): FAIL-CLOSED. tick_finalize only
                    // returns Err on (a) BLS / structural inconsistency
                    // (impossible — finalize_slash already produced a
                    // result) or (b) RocksDB persistence failure for
                    // the Confirmed record (we persist BEFORE the
                    // burn; a persist failure means the burn was
                    // intentionally skipped to keep the next tick
                    // re-attemptable). Continuing past either case
                    // would silently drop the slash for the rest of
                    // the process lifetime — the same forgive-on-
                    // restart bypass slashing exists to prevent.
                    error!(error = %e, block = height,
                           "FATAL: slashing pipeline tick_finalize FAILED — \
                            halting to prevent silent slash drop");
                    panic!("slashing pipeline tick_finalize failed at block {height}: {e}");
                }
            }
        }

        // Execute + persist.
        let committed = if let Some(vs) = &self.rpc_validator_set {
            crate::block_producer::execute_and_commit_with_validator_set(
                &self.storage, &self.mempool, vs, block,
            )?
        } else {
            execute_and_commit(&self.storage, &self.mempool, block)?
        };

        // ── P2P broadcast: send committed block to all connected TCP peers ──────────────
        if let Some(net) = &self.network {
            net.broadcast_block(&committed);
        }

        // ── WebSocket push: broadcast new block head to all eth_subscribe subscribers ──
        if let Some(tx) = &self.new_head_tx {
            let _ = tx.send(block_to_head_json(&committed));
        }

        // SEC-2026-05-09 Pass-19 (Task #9): EPOCH-BOUNDARY SEED
        // ROTATION. Runs UNCONDITIONALLY on every commit — must NOT be
        // gated on `rpc_validator_set` (architect-review follow-up #1)
        // because the proposer schedule is consensus-critical state
        // and any non-RPC binary path (tests, alt binaries, future
        // refactors) would otherwise silently demote rotation to the
        // legacy round-robin fallback.
        //
        // Trigger: committing the LAST block of an epoch
        // (`(height + 1) % epoch_length == 0`). The next block (the
        // first of the new epoch) will be proposed under the freshly-
        // rotated seed, defeating the multi-epoch leader-prediction
        // DoS that Pass-15 HIGH-R03 only partially closed (rotation
        // hook existed but was never called from a live commit path).
        //
        // `new_seed = keccak256(committed_block_hash || next_epoch_be8 || prev_seed)`
        //
        // - `committed_block_hash`: ties the seed to a quorum-signed
        //   block, so an attacker cannot bias the seed without forking.
        // - `next_epoch_be8`: positionally distinct even if two epochs
        //   ever share a parent hash (genesis edge / deep reorg).
        // - `prev_seed`: chains epochs together so an attacker who
        //   wants to manipulate epoch N's schedule must manipulate
        //   every prior epoch boundary too.
        //
        // Hot-swap (`update_validator_set` below) preserves the
        // `epoch_seed` across slashing-driven shrinks (see
        // `HotStuffConsensus::update_validator_set` doc + the
        // `hot_swap_preserves_epoch_seed` regression test in
        // `crates/zbx-consensus/tests/epoch_seed_rotation.rs`).
        if (height + 1) % self.cfg.epoch_length == 0 {
            // SEC-2026-05-09 Pass-19 (architect-review follow-up #4):
            // Rotation MUST key on the POST-EXECUTION canonical hash
            // (`committed.hash()`), not the pre-execution candidate
            // hash captured at line 708. `execute_and_commit` mutates
            // `state_root` / `transactions_root` / `receipts_root` /
            // `logs_bloom` / `gas_used`, all of which feed the
            // canonical block hash that ends up in storage and on the
            // wire. Any light client that derives the next epoch's
            // proposer schedule from canonical headers would see a
            // different seed than nodes that used the pre-execution
            // hash → consensus split at every epoch boundary.
            let canonical_hash = committed.hash();
            let next_epoch = (height + 1) / self.cfg.epoch_length;
            let prev_seed = self.consensus.validator_set.epoch_seed;
            let mut buf = Vec::with_capacity(32 + 8 + 32);
            buf.extend_from_slice(canonical_hash.as_bytes());
            buf.extend_from_slice(&next_epoch.to_be_bytes());
            buf.extend_from_slice(prev_seed.as_bytes());
            let new_seed = zbx_crypto::keccak::keccak256(&buf);
            self.consensus.rotate_epoch_seed(new_seed);
            info!(
                next_epoch,
                height,
                block = ?canonical_hash,
                "epoch boundary: rotated proposer-shuffle seed (Task #9)"
            );
        }

        // ── ValidatorSet sync: keep RPC layer current ────────────────────────────────
        if let Some(vs_arc) = &self.rpc_validator_set {
            let mut vs = vs_arc.write();
            vs.current_epoch = epoch;

            // Ensure every consensus validator is present in the staking registry.
            // At epoch boundary also refresh active_set and promote to Active status.
            let at_epoch_boundary = height % self.cfg.epoch_length == 0;
            for (addr, pubkey) in &self.cfg.validators {
                vs.validators.entry(*addr).or_insert_with(|| Validator {
                    address: *addr,
                    bls_pubkey: pubkey.clone(),
                    self_stake: zbx_staking::MIN_SELF_STAKE,
                    delegated_stake: 0,
                    commission_bps: 500,
                    status: ValidatorStatus::Active,
                    last_signed_block: height,
                    pending_rewards: 0,
                    delegator_reward_pool: 0,
                    pool_denominator: 0,
                    registered_epoch: epoch,
                });
                if at_epoch_boundary {
                    if let Some(v) = vs.validators.get_mut(addr) {
                        v.last_signed_block = height;
                        // SEC-2026-05-09 Pass-11 (architect-review
                        // follow-up #3): PRESERVE JAIL STATUS across
                        // epoch boundary. The previous unconditional
                        // `status = Active` undid every slashing
                        // outcome at the next epoch — a complete
                        // economic-security bypass. Now: only re-
                        // activate validators that are NOT jailed /
                        // tombstoned. A separate operator-initiated
                        // unjail flow is required to re-enter the
                        // active set after slashing.
                        // SEC-2026-05-09 Pass-11 architect round-2:
                        // ValidatorStatus enum has no `Tombstoned` —
                        // current variants are Active / Jailed /
                        // Inactive / Unbonding / Pending. We preserve
                        // `Jailed` here (the slashing outcome). A
                        // future `Tombstoned` variant for permanent
                        // ban can be added in Pass-12 alongside the
                        // dynamic-active-set HotStuff wiring.
                        if v.status == ValidatorStatus::Jailed {
                            warn!(
                                validator = ?addr,
                                epoch,
                                "epoch boundary: SKIPPING re-activation of \
                                 jailed validator (slashing preserved)"
                            );
                        } else {
                            v.status = ValidatorStatus::Active;
                        }
                    }
                }
            }
            if at_epoch_boundary {
                // VALIDATOR-SYNC FIX: Call elect_active_set() so that:
                //   (a) Validators who staked AFTER genesis enter the active
                //       set once their stake qualifies them.
                //   (b) Validators whose self_stake dropped below MIN_SELF_STAKE
                //       are removed deterministically (STK-ELT-01 tiebreak).
                //   (c) Jailed / Unbonding validators are excluded via
                //       Validator::is_eligible() inside elect_active_set().
                //
                // Previously the code only filtered the static cfg.validators
                // list (genesis-only) by non-Jailed status, which meant:
                //   • No new post-genesis validator ever entered consensus.
                //   • Under-staked validators were never evicted.
                //   • The staking registry and consensus diverged silently.
                let new_active = vs.elect_active_set();

                // VALIDATOR-SYNC FIX: Register BLS pubkeys for any newly
                // elected validators that aren't yet in the HotStuff pubkey
                // registry. Without this, newly-elected validators' votes
                // are rejected by on_vote() (no auth basis) and they can
                // never contribute to quorum even though they are in the
                // active set. Only register on first election — the registry
                // deliberately refuses overwrites (see Pass-10 invariant).
                for addr in &new_active {
                    if let Some(v) = vs.validators.get(addr) {
                        self.consensus.register_validator_pubkey(
                            *addr,
                            v.bls_pubkey.clone(),
                        );
                    }
                }

                drop(vs);

                // HOT-SWAP THE CONSENSUS VALIDATOR SET with the stake-elected
                // result. Quorum (2f+1) is recomputed inside
                // `update_validator_set` on every call, so f shrinks/grows
                // as validators enter/exit the active set.
                self.consensus.update_validator_set(new_active.clone());

                info!(
                    epoch,
                    validators = new_active.len(),
                    quorum = self.consensus.validator_set.quorum,
                    "epoch boundary: RPC + HotStuff validator set refreshed via elect_active_set"
                );
            }
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Build a synthetic parent QuorumCertificate for a given candidate block.
    ///
    /// For the first block after genesis (height 1) or whenever we have no
    /// stored highest QC, we return a "genesis QC" — an all-zero aggregate
    /// signature with an empty signer list.  This satisfies the safety rule
    /// `parent_qc.block_number() >= locked_round` since `locked_round` starts
    /// at 0 and the genesis QC's block_number = parent block height ≥ 0.
    fn parent_qc_for(&self, block: &Block) -> QuorumCertificate {
        // Use the real highest QC if the state machine already has one.
        if let Some(qc) = &self.consensus.highest_qc {
            if qc.block_number() + 1 == block.header.number {
                return qc.clone();
            }
        }

        // Synthetic genesis QC.
        let parent_number = block.header.number.saturating_sub(1);
        let epoch = block.header.epoch;
        QuorumCertificate {
            vote_data: VoteData {
                block_hash: block.header.parent_hash,
                block_number: parent_number,
                phase: Phase::Commit as u8,
                epoch,
            },
            agg_signature: BlsSignature([0u8; 96]),
            signers: vec![],
            signer_pubkeys: vec![],
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Serialize a committed block into the Ethereum `newHeads` subscription JSON
/// format expected by MetaMask, ethers.js, and web3.js.
fn block_to_head_json(block: &Block) -> Value {
    let hash = block.hash();
    json!({
        "number":           format!("0x{:x}", block.header.number),
        "hash":             format!("0x{}", hex::encode(hash)),
        "parentHash":       format!("0x{}", hex::encode(block.header.parent_hash.0)),
        "timestamp":        format!("0x{:x}", block.header.timestamp),
        "gasLimit":         format!("0x{:x}", block.header.gas_limit),
        "gasUsed":          format!("0x{:x}", block.header.gas_used),
        "miner":            format!("0x{}", hex::encode(block.header.coinbase.as_bytes())),
        "stateRoot":        format!("0x{}", hex::encode(block.header.state_root.0)),
        "transactionsRoot": format!("0x{}", hex::encode(block.header.transactions_root.0)),
        "receiptsRoot":     format!("0x{}", hex::encode(block.header.receipts_root.0)),
        "logsBloom":        format!("0x{}", hex::encode(block.header.logs_bloom)),
        "baseFeePerGas":    format!("0x{:x}", block.header.base_fee_per_gas),
        "extraData":        format!("0x{}", hex::encode(&block.header.extra_data)),
        "difficulty":       "0x0",
        "totalDifficulty":  "0x0",
        "nonce":            "0x0000000000000000",
        "sha3Uncles":       "0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347",
        "transactions":     block.body.transactions.len(),
    })
}

// ---------------------------------------------------------------------------
// SEC-2026-05-09 Pass-19 (Task #9, architect-review follow-up #6):
// CONSENSUS-INVALIDATING structural validation of `header.epoch_seed`.
// Extracted as a standalone pure function so the rejection rules can be
// unit-tested directly without spinning up a full ConsensusDriver.
// Called from `do_commit` above. Any change to the rules here MUST also
// preserve the propose-path invariants in `propose_round` (line ~315).
// ---------------------------------------------------------------------------
pub(crate) fn validate_epoch_seed(
    height: u64,
    header_seed: Option<H256>,
    local_seed: H256,
    epoch_length: u64,
) -> Result<(), String> {
    let is_epoch_start = height > 0 && height % epoch_length == 0;
    match (header_seed, is_epoch_start) {
        (Some(hdr_seed), true) => {
            if hdr_seed != local_seed {
                return Err(format!(
                    "consensus-invalid block at height {height}: \
                     header.epoch_seed = {hdr_seed:?} disagrees with \
                     locally-rotated seed = {local_seed:?}. \
                     Light-client verification would split — rejecting."
                ));
            }
            Ok(())
        }
        (Some(hdr_seed), false) => Err(format!(
            "consensus-invalid block at height {height}: \
             header.epoch_seed = {hdr_seed:?} set on a non-boundary \
             block (epoch_length = {epoch_length}). Pass-19 invariant \
             violated — rejecting."
        )),
        (None, true) => {
            if local_seed != H256::zero() {
                return Err(format!(
                    "consensus-invalid block at height {height}: \
                     epoch-start block missing header.epoch_seed while \
                     local rotated seed = {local_seed:?} is non-zero. \
                     Light clients would reject — rejecting at full node."
                ));
            }
            Ok(())
        }
        (None, false) => Ok(()),
    }
}

#[cfg(test)]
mod epoch_seed_validation_tests {
    use super::*;

    const EPOCH: u64 = 4;

    fn nonzero_seed(byte: u8) -> H256 {
        H256([byte; 32])
    }

    /// Negative: boundary block with WRONG seed value MUST be rejected.
    #[test]
    fn rejects_boundary_block_with_mismatched_seed() {
        let local = nonzero_seed(0xAA);
        let wrong = nonzero_seed(0xBB);
        let r = validate_epoch_seed(EPOCH, Some(wrong), local, EPOCH);
        assert!(r.is_err(), "boundary block with wrong seed must be rejected");
        assert!(r.unwrap_err().contains("disagrees with"));
    }

    /// Negative: non-boundary block carrying any `Some` seed MUST be rejected.
    #[test]
    fn rejects_non_boundary_block_with_seed_set() {
        let local = nonzero_seed(0xAA);
        let stray = nonzero_seed(0xCC);
        // Height 5 with epoch_length 4 → not a boundary.
        let r = validate_epoch_seed(5, Some(stray), local, EPOCH);
        assert!(r.is_err(), "non-boundary block with seed must be rejected");
        assert!(r.unwrap_err().contains("non-boundary"));
        // Same with seed equal to local — still rejected (off-boundary is
        // the violation, not the value).
        let r2 = validate_epoch_seed(5, Some(local), local, EPOCH);
        assert!(r2.is_err());
    }

    /// Negative: boundary block missing seed AFTER bootstrap MUST be rejected.
    #[test]
    fn rejects_boundary_block_missing_seed_after_bootstrap() {
        let local = nonzero_seed(0xAA);
        let r = validate_epoch_seed(EPOCH, None, local, EPOCH);
        assert!(r.is_err(), "boundary block missing seed after bootstrap must be rejected");
        assert!(r.unwrap_err().contains("missing header.epoch_seed"));
    }

    /// Positive: boundary block missing seed during BOOTSTRAP epoch
    /// (local_seed still zero) MUST be accepted — the propose path
    /// intentionally skips the header patch in this case.
    #[test]
    fn accepts_bootstrap_epoch_boundary_with_no_seed() {
        let r = validate_epoch_seed(EPOCH, None, H256::zero(), EPOCH);
        assert!(r.is_ok(), "bootstrap-epoch boundary with no seed must be accepted: {r:?}");
    }

    /// Positive: non-boundary block with `None` is the universal happy
    /// path and MUST be accepted.
    #[test]
    fn accepts_non_boundary_block_with_no_seed() {
        let local = nonzero_seed(0xAA);
        let r = validate_epoch_seed(7, None, local, EPOCH);
        assert!(r.is_ok());
    }

    /// Positive: boundary block whose seed exactly matches the local
    /// rotated seed MUST be accepted.
    #[test]
    fn accepts_boundary_block_with_matching_seed() {
        let local = nonzero_seed(0xAA);
        let r = validate_epoch_seed(EPOCH, Some(local), local, EPOCH);
        assert!(r.is_ok());
    }

    /// Edge: height 0 (genesis) is NEVER a boundary, so any `Some` is
    /// rejected and `None` is accepted.
    #[test]
    fn height_zero_is_not_a_boundary() {
        assert!(validate_epoch_seed(0, None, H256::zero(), EPOCH).is_ok());
        assert!(validate_epoch_seed(0, Some(nonzero_seed(0x11)), H256::zero(), EPOCH).is_err());
    }
}
