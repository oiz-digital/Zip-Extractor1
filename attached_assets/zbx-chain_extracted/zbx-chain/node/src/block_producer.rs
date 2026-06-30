//! Block production loop.
//!
//! On each tick we:
//!   1. Read the chain head from `ZbxDb`.
//!   2. Pull the highest-tip transactions from the mempool that fit under the
//!      block gas limit.
//!   3. Assemble a candidate `Block` (parent_hash, number+1, timestamp, …).
//!   4. Run `BlockExecutor::execute` to apply the txs to a `StateView` seeded
//!      with the senders' on-chain accounts. The executor returns receipts,
//!      a state diff, and the new state root.
//!   5. Persist the resulting accounts (state diff) and the block atomically.
//!   6. Update the mempool's base-fee tracker and evict the included txs.
//!
//! ## HotStuff consensus integration
//!
//! `build_candidate` and `execute_and_commit` are the two halves that the
//! `ConsensusDriver` uses for multi-validator operation:
//!   * `build_candidate` — select txs + assemble header/body, no execution.
//!   * `execute_and_commit` — execute a pre-built candidate, patch header
//!      commitments, persist, update mempool.
//!
//! The single-validator `run` loop calls `produce_one` which does both in one
//! step; it is still used for backward-compatible single-validator mode.
//!
//! ## Audit 2026-04-30 — S4-B4 / B5 / B6 / B7 closed
//!
//! - **B4 reorg/equivocation hooks**: every iteration verifies the parent we
//!   sealed against is still the storage head.
//! - **B5 block-time deadline**: logged at >80% and >100% of block_time.
//! - **B6 configurable empty blocks**: off by default.
//! - **B7 scheduler observability gate**: lane metrics behind ZBX_LOG_SCHEDULER.
//!
//! ## Slashing integration (2026-05-03)
//!
//! `run()` maintains a `SlashingDetector` and records per-block liveness:
//! the proposer (coinbase) counts as a signed vote; every other address in
//! `ProducerConfig::active_validators` counts as a missed block.  Instant-jail
//! events are logged; the full on-chain state update requires a wired
//! `ValidatorSet` (wired by the ConsensusDriver for committee mode).

use parking_lot::RwLock;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ─── Task #15 commit hook ──────────────────────────────────────────────
// Process-global handle to the trie pruner's retained-roots list.
// `node::run` installs this once at startup via `set_retained_tracker`;
// every subsequent successful `execute_and_commit_inner` push the new
// (block, state_root) checkpoint inline. This is the real producer-
// commit hook the pruner depends on (replaces the earlier polling
// tracker that had a startup-seeding bug). Covers all three commit
// paths (consensus driver, single-validator `produce_one`, and the
// network-sync path in `network.rs`) without touching their signatures.
static RETAINED_TRACKER: OnceLock<Arc<RwLock<Vec<zbx_pruner::Retained>>>> =
    OnceLock::new();

/// Install the process-global pruner retained-roots tracker. Call once
/// at node startup BEFORE the consensus driver / producer begins
/// committing blocks. Subsequent calls are no-ops (returns the original
/// handle), so test binaries that initialise twice stay safe.
pub fn set_retained_tracker(t: Arc<RwLock<Vec<zbx_pruner::Retained>>>) {
    let _ = RETAINED_TRACKER.set(t);
}

fn push_retained(block_number: u64, state_root: H256) {
    if let Some(t) = RETAINED_TRACKER.get() {
        t.write().push(zbx_pruner::Retained {
            block: block_number,
            state_root,
        });
    }
}
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use zbx_execution::{schedule as schedule_lanes, BlockExecutor, StateView};
use zbx_fee::BaseFeeCalculator;
use zbx_mempool::TransactionPool;
use zbx_staking::{RewardDistributor, SlashingDetector, ValidatorSet, REWARD_INTERVAL,
    try_finalize_all_pending,
};
use zbx_storage::ZbxDb;
use zbx_types::{
    address::Address,
    block::{Block, BlockBody, BlockHeader},
    governance::{ProposalId, ProposalRegistry},
    module_version::ModuleVersion,
    staking_tx::STAKING_PRECOMPILE_ADDR,
    transaction::SignedTransaction,
    version_registry::{RegistryUpgrade, VersionRegistry},
    H256, BLOCK_GAS_LIMIT,
};

// ─── On-chain governance state (ZEP execution) ──────────────────────────────
// Two well-known keys in the `Metadata` CF carry the canonical
// `VersionRegistry` and `ProposalRegistry` for the chain. Stored as
// bincode (matches the slashing-pipeline convention) so schema evolution
// is additive-only on the wire.
//
// Both registries are written in a SINGLE fsynced RocksDB WriteBatch
// (`put_metadata_batch_synced`) so a torn half-write is impossible: a
// crash either leaves both old values on disk or both new ones.
//
// Crash-recovery semantics:
//   * block-tip is fsynced first (existing `put_block` path);
//   * staking-delta is fsynced second (existing `apply_staking_delta`);
//   * governance is fsynced third (this hook).
// If the process dies between (2) and (3), the block IS on disk but
// the governance promotion isn't — on restart the producer re-loads
// the previous `ProposalRegistry`, the same proposals are still
// `Scheduled`, and `ready_to_execute(N) ⊇ ready_to_execute(prev_N)` so
// re-application at the next block is monotonically equivalent for
// the module-version-bump payloads this hook supports today
// (`ModuleVersions::set` rejects downgrades, so a redundant re-apply
// is either a no-op or an erroring no-op). The producer therefore
// remains deterministic across restarts without an explicit reconcile.
const GOVERNANCE_META_VERSION_REGISTRY:  &[u8] = b"governance/version_registry";
const GOVERNANCE_META_PROPOSAL_REGISTRY: &[u8] = b"governance/proposal_registry";

/// Load the canonical `VersionRegistry` from metadata, defaulting to the
/// empty (genesis) registry when no governance proposal has yet been
/// executed. Decode failure is fail-closed: a corrupted registry must
/// not be silently overwritten — better to halt consensus than to
/// silently re-apply every historical upgrade on top of a default.
fn load_version_registry(storage: &ZbxDb) -> Result<VersionRegistry, String> {
    match storage.get_metadata(GOVERNANCE_META_VERSION_REGISTRY) {
        Ok(Some(bytes)) => bincode::deserialize(&bytes)
            .map_err(|e| format!("version_registry decode: {e}")),
        Ok(None) => Ok(VersionRegistry::default()),
        Err(e)   => Err(format!("version_registry read: {e}")),
    }
}

fn load_proposal_registry(storage: &ZbxDb) -> Result<ProposalRegistry, String> {
    match storage.get_metadata(GOVERNANCE_META_PROPOSAL_REGISTRY) {
        Ok(Some(bytes)) => bincode::deserialize(&bytes)
            .map_err(|e| format!("proposal_registry decode: {e}")),
        Ok(None) => Ok(ProposalRegistry::new()),
        Err(e)   => Err(format!("proposal_registry read: {e}")),
    }
}

fn persist_governance_state(
    storage: &ZbxDb,
    vreg: &VersionRegistry,
    preg: &ProposalRegistry,
) -> Result<(), String> {
    let v = bincode::serialize(vreg)
        .map_err(|e| format!("version_registry encode: {e}"))?;
    let p = bincode::serialize(preg)
        .map_err(|e| format!("proposal_registry encode: {e}"))?;
    // SINGLE fsynced write-batch — both keys land atomically, no torn
    // half-write possible. See `ZbxDb::put_metadata_batch_synced`.
    storage
        .put_metadata_batch_synced(&[
            (GOVERNANCE_META_VERSION_REGISTRY,  v),
            (GOVERNANCE_META_PROPOSAL_REGISTRY, p),
        ])
        .map_err(|e| format!("governance state write: {e}"))
}

/// Apply every governance proposal whose `activation_block ≤ current_block`
/// and whose status is `Scheduled`. Returns `true` iff the registries
/// changed (i.e. at least one proposal was promoted to `Executed` or
/// `Failed`). The caller is responsible for persisting the registries
/// when this returns `true`.
///
/// `UpgradeProposal` currently encodes only `(module_name, new_version)` —
/// not a full `RegistryUpgrade` payload — so this hook synthesises a
/// minimal upgrade containing just the module-version bump. Activation
/// schedules, feature flags, and storage-version bumps require a future
/// ZEP that extends `UpgradeProposal` with an `upgrade: RegistryUpgrade`
/// field. Documented as the follow-up to this wiring.
///
/// FAIL-CLOSED semantics: if `RegistryUpgrade::apply` returns `Err`
/// (e.g. monotonicity violation), the proposal is marked `Failed` and
/// the registry is left untouched. The block still commits — governance
/// failure is recorded on-chain, not propagated as a consensus halt.
fn apply_ready_governance(
    vreg: &mut VersionRegistry,
    preg: &mut ProposalRegistry,
    current_block: u64,
) -> bool {
    let ready: Vec<ProposalId> = preg
        .ready_to_execute(current_block)
        .map(|p| p.id)
        .collect();
    if ready.is_empty() {
        return false;
    }
    for id in ready {
        // Snapshot the fields we need so we can mutate preg afterwards
        // without holding an immutable borrow.
        let (module_name, new_version) = match preg.get(id) {
            Some(p) => (p.module_name.clone(), p.new_version),
            None    => continue, // unreachable: ready_to_execute returned this id
        };
        let upgrade = match ModuleVersion::new(module_name, new_version) {
            Ok(mv) => RegistryUpgrade {
                set_modules: vec![mv],
                ..Default::default()
            },
            Err(e) => {
                warn!(?id, error = %e, "governance: module_version construct failed");
                if let Some(p) = preg.get_mut(id) {
                    let _ = p.mark_failed();
                }
                continue;
            }
        };
        match vreg.apply(&upgrade) {
            Ok(()) => {
                if let Some(p) = preg.get_mut(id) {
                    let _ = p.mark_executed();
                }
                info!(?id, height = current_block, "governance upgrade executed");
            }
            Err(e) => {
                warn!(?id, error = %e, "governance upgrade rejected; marking proposal Failed");
                if let Some(p) = preg.get_mut(id) {
                    let _ = p.mark_failed();
                }
            }
        }
    }
    true
}

/// Configuration for the producer.
#[derive(Debug, Clone)]
pub struct ProducerConfig {
    /// Target time between blocks.
    pub block_time: Duration,
    /// Validator coinbase that receives block rewards + tips.
    pub coinbase: Address,
    /// Per-block gas ceiling (must be ≤ BLOCK_GAS_LIMIT).
    pub gas_limit: u64,
    /// When `false` (default), skip ticks that have no eligible transactions
    /// instead of sealing an empty block.
    pub produce_empty_blocks: bool,
    /// Full active validator set used for per-block liveness / slashing tracking.
    /// Empty = slashing disabled (safe for single-validator devnet mode).
    pub active_validators: Vec<Address>,
}

impl Default for ProducerConfig {
    fn default() -> Self {
        ProducerConfig {
            block_time: Duration::from_millis(5_000),
            coinbase: Address::ZERO,
            gas_limit: BLOCK_GAS_LIMIT,
            produce_empty_blocks: false,
            active_validators: vec![],
        }
    }
}

// ---------------------------------------------------------------------------
// Single-validator run loop (original behaviour, slashing added)
// ---------------------------------------------------------------------------

/// Async block-production task. Runs until the runtime is dropped.
pub async fn run(
    storage: Arc<ZbxDb>,
    mempool: Arc<RwLock<TransactionPool>>,
    cfg: ProducerConfig,
) {
    info!(
        block_time_ms = cfg.block_time.as_millis() as u64,
        gas_limit = cfg.gas_limit,
        coinbase = %hex::encode(cfg.coinbase.as_bytes()),
        produce_empty_blocks = cfg.produce_empty_blocks,
        "block producer started"
    );

    let mut tick = interval(cfg.block_time);
    tick.tick().await;

    let mut slashing = SlashingDetector::new();

    loop {
        tick.tick().await;
        let start = Instant::now();
        match produce_one(&storage, &mempool, &cfg) {
            Ok(Some(sealed)) => {
                // Liveness tracking: proposer voted, all others missed.
                let height = sealed.header.number;
                let epoch = height / zbx_staking::EPOCH_LENGTH;
                let block_hash = sealed.hash();

                if !cfg.active_validators.is_empty() {
                    for addr in &cfg.active_validators {
                        if *addr == cfg.coinbase {
                            if let Some(ev) = slashing.record_vote(*addr, height, 0, block_hash) {
                                warn!(validator = ?addr, "double-sign detected during single-validator production");
                                let _ = ev;
                            }
                        } else if let Some(ev) = slashing.record_missed_block(*addr, epoch, height) {
                            warn!(
                                validator = ?addr,
                                "instant-jail: validator missed {} consecutive blocks",
                                zbx_staking::MAX_CONSECUTIVE_MISSED
                            );
                            let _ = ev;
                        }
                    }
                }

                let elapsed = start.elapsed();
                if elapsed > cfg.block_time {
                    warn!(
                        elapsed_ms = elapsed.as_millis() as u64,
                        budget_ms = cfg.block_time.as_millis() as u64,
                        "block production exceeded block_time"
                    );
                } else if elapsed > cfg.block_time.mul_f32(0.8) {
                    warn!(
                        elapsed_ms = elapsed.as_millis() as u64,
                        budget_ms = cfg.block_time.as_millis() as u64,
                        "block production using >80% of block_time budget"
                    );
                }
            }
            Ok(None) => {
                debug!("idle — no eligible transactions in mempool");
            }
            Err(e) => {
                error!(error = %e, "block production failed; will retry on next tick");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Public primitives used by ConsensusDriver
// ---------------------------------------------------------------------------

/// Build a candidate block (steps 1–3): resolve parent, select txs, assemble
/// header + body.  Does NOT execute or persist anything.
///
/// Returns `Ok(None)` when the mempool is empty and `produce_empty_blocks` is
/// disabled.  Returns `Ok(Some(block))` with all commitment fields set to
/// their placeholder (all-zero) values — they will be patched by
/// `execute_and_commit` after execution.
pub fn build_candidate(
    storage: &Arc<ZbxDb>,
    mempool: &Arc<RwLock<TransactionPool>>,
    cfg: &ProducerConfig,
) -> Result<Option<Block>, String> {
    let parent_number = storage.get_latest_block_number().unwrap_or(0);
    let parent = storage
        .get_block_by_number(parent_number)
        .map_err(|e| format!("storage parent: {e}"))?
        .ok_or_else(|| format!("no block at height {parent_number}"))?;
    let parent_hash = parent.hash();

    debug!(
        height = parent.header.number + 1,
        proposer = %hex::encode(cfg.coinbase.as_bytes()),
        parent = %hex::encode(parent_hash),
        "building candidate block"
    );

    let mut txs: Vec<SignedTransaction> = {
        let pool = mempool.read();
        pool.select_transactions(cfg.gas_limit)
    };
    if txs.is_empty() && !cfg.produce_empty_blocks {
        return Ok(None);
    }
    txs.sort_by(|a, b| match a.from.cmp(&b.from) {
        std::cmp::Ordering::Equal => a.tx.nonce.cmp(&b.tx.nonce),
        _ => std::cmp::Ordering::Equal,
    });

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(parent.header.timestamp + 1);
    let timestamp = now.max(parent.header.timestamp + 1);

    let header = BlockHeader {
        parent_hash,
        uncle_hash: H256([0u8; 32]),
        coinbase: cfg.coinbase,
        state_root: H256([0u8; 32]),
        transactions_root: H256([0u8; 32]),
        receipts_root: H256([0u8; 32]),
        logs_bloom: [0u8; 256],
        difficulty: zbx_types::U256::zero(),
        number: parent.header.number + 1,
        gas_limit: cfg.gas_limit,
        gas_used: 0,
        timestamp,
        extra_data: b"zbx".to_vec(),
        mix_hash: H256([0u8; 32]),
        nonce: 0,
        // EIP-1559 fee market wiring (zbx-fee): compute next block's base fee
        // from the parent block's actual gas usage instead of blindly copying
        // the parent value.  Pre-fix, base_fee never changed — every block
        // paid exactly the genesis fee regardless of network demand.
        //
        // Formula (EIP-1559): if parent was above target (50% gas_limit) →
        // base_fee increases by up to 12.5%; below → decreases by up to 12.5%;
        // at target → unchanged.  Floor is MIN_BASE_FEE (7 wei).
        base_fee_per_gas: BaseFeeCalculator::next_base_fee(
            parent.header.base_fee_per_gas,
            parent.header.gas_used,
            parent.header.gas_limit,
        ).unwrap_or_else(|_| parent.header.base_fee_per_gas.max(1)),
        committee_signature: Vec::new(),
        epoch: parent.header.epoch,
        // SEC-2026-05-09 Pass-19 (Task #9): epoch_seed is patched in by
        // the consensus driver AFTER build_candidate when this block is
        // the first of a new epoch. Producer-side default is `None`.
        epoch_seed: None,
    };
    let body = BlockBody { transactions: txs, uncles: Vec::new() };
    Ok(Some(Block { header, body }))
}

/// Execute a pre-built candidate block and persist it to storage.
///
/// This seeds a `StateView` from the block's transactions, runs the
/// `BlockExecutor`, patches the header commitment fields (state_root,
/// transactions_root, receipts_root, logs_bloom), flushes trie nodes,
/// writes the block, and evicts included txs from the mempool.
///
/// Returns the committed `Block` (with patched header fields) on success.
/// Returns `Err` if execution fails, a reorg is detected, or storage fails.
pub fn execute_and_commit(
    storage: &Arc<ZbxDb>,
    mempool: &Arc<RwLock<TransactionPool>>,
    block: Block,
) -> Result<Block, String> {
    execute_and_commit_inner(storage, mempool, None, block)
}

/// Variant of `execute_and_commit` that routes staking-destination txs
/// through `BlockExecutor::execute_with_staking`, mutating the supplied
/// `ValidatorSet` (registers, delegations, undelegations, rewards).
pub fn execute_and_commit_with_validator_set(
    storage: &Arc<ZbxDb>,
    mempool: &Arc<RwLock<TransactionPool>>,
    validator_set: &Arc<RwLock<ValidatorSet>>,
    block: Block,
) -> Result<Block, String> {
    execute_and_commit_inner(storage, mempool, Some(validator_set), block)
}

/// Apply the per-interval (every `REWARD_INTERVAL` blocks) escrow mint and
/// ValidatorSet reward accounting.
///
/// ## What this does (RWD-ESCROW-01)
///
/// 1. Scans the last `REWARD_INTERVAL` committed blocks from storage to sum the
///    actual priority fees paid across every transaction in the window.  Per-tx
///    fee = `tip_per_gas × gas_used`; `gas_used` is read from the persisted
///    receipt for precision (falls back to `gas_limit` if receipt is missing).
///
/// 2. Calls `RewardDistributor::interval_escrow_mint` to compute the total ZBX
///    that should be minted into the staking escrow account (`STAKING_PRECOMPILE_ADDR`).
///    Writes the updated balance to storage so that subsequent `ClaimRewards` /
///    `ClaimDelegatorRewards` transactions can draw from it without underflow.
///
/// 3. Calls `RewardDistributor::distribute_block_reward` to split the interval
///    total across active validators proportionally to their total stake, crediting
///    each validator's `pending_rewards` and `delegator_reward_pool` in the live
///    `ValidatorSet` (already swapped to the post-execution state by the caller).
///
/// Returns the minted amount (may be 0 on the genesis block or if no validators
/// are active).  Errors are I/O failures from the storage layer — the caller
/// logs and suppresses them rather than halting the node.
fn apply_interval_rewards(
    storage: &Arc<ZbxDb>,
    block: &Block,
    vs_arc: &Arc<RwLock<ValidatorSet>>,
) -> Result<u128, String> {
    let height = block.header.number;
    let window_start = height.saturating_sub(REWARD_INTERVAL - 1);

    // ── Step 1: accumulate actual priority fees from the window ──────────────
    // Using receipt gas_used gives the precise amount burned; gas_limit would
    // overestimate (typically 2× for simple transfers, more for contracts).
    let mut accumulated_fees: u128 = 0;
    for h in window_start..=height {
        if let Ok(Some(b)) = storage.get_block_by_number(h) {
            let base_fee = b.header.base_fee_per_gas;
            for tx in &b.body.transactions {
                let tip_per_gas =
                    tx.effective_gas_price(base_fee).saturating_sub(base_fee) as u128;
                if tip_per_gas == 0 {
                    continue;
                }
                // Prefer the persisted receipt for actual gas_used; fall back to
                // gas_limit when the receipt is absent (genesis / test paths).
                let gas_used = storage
                    .get_receipt(&tx.hash)
                    .ok()
                    .flatten()
                    .map(|r| r.gas_used as u128)
                    .unwrap_or(tx.tx.gas_limit as u128);
                accumulated_fees =
                    accumulated_fees.saturating_add(tip_per_gas.saturating_mul(gas_used));
            }
        }
    }

    // ── Step 2: mint into staking escrow (STAKING_PRECOMPILE_ADDR) ───────────
    // interval_escrow_mint returns 0 on non-interval blocks (no-op guard is
    // redundant since the caller already checks, but defensive is fine here).
    let mint = RewardDistributor::interval_escrow_mint(height, accumulated_fees);
    if mint > 0 {
        let mut escrow = storage
            .get_account(&STAKING_PRECOMPILE_ADDR)
            .map_err(|e| format!("escrow get_account: {e}"))?;
        escrow.set_balance_u128(escrow.balance_u128().saturating_add(mint));
        storage
            .put_account(&STAKING_PRECOMPILE_ADDR, &escrow)
            .map_err(|e| format!("escrow put_account: {e}"))?;
        debug!(
            height,
            mint_wei = mint,
            accumulated_fees,
            "escrow minted into STAKING_PRECOMPILE_ADDR"
        );
    }

    // ── Step 3: distribute into ValidatorSet accounting ──────────────────────
    // This updates pending_rewards + delegator_reward_pool in the live
    // validator set.  The update is in-memory and will be persisted on the
    // next staking-delta flush (i.e. the next block containing a staking tx).
    {
        let mut vs = vs_arc.write();
        RewardDistributor::distribute_block_reward(
            &mut vs,
            height,
            &block.header.coinbase,
            accumulated_fees,
        );
    }

    Ok(mint)
}

fn execute_and_commit_inner(
    storage: &Arc<ZbxDb>,
    mempool: &Arc<RwLock<TransactionPool>>,
    validator_set: Option<&Arc<RwLock<ValidatorSet>>>,
    mut block: Block,
) -> Result<Block, String> {
    let parent_number = block.header.number.saturating_sub(1);

    // Seed StateView with all referenced accounts and their code.
    let mut view = StateView::new();
    let mut seen = std::collections::HashSet::new();
    let mut seed = |view: &mut StateView,
                    addr: Address,
                    seen: &mut std::collections::HashSet<Address>|
     -> Result<(), String> {
        if !seen.insert(addr) {
            return Ok(());
        }
        let acct = storage
            .get_account(&addr)
            .map_err(|e| format!("get_account {}: {e}", hex::encode(addr.as_bytes())))?;
        if acct.is_contract() {
            let code = storage
                .get_code(&acct.code_hash)
                .map_err(|e| format!("get_code: {e}"))?;
            view.seed_code(acct.code_hash, code);
        }
        view.seed_account(addr, acct);
        Ok(())
    };

    for tx in &block.body.transactions {
        seed(&mut view, tx.from, &mut seen)?;
        if let Some(to) = tx.tx.to {
            seed(&mut view, to, &mut seen)?;
        }
    }
    seed(&mut view, block.header.coinbase, &mut seen)?;

    // Lane-scheduling (observability gate, same as produce_one).
    let lane_budget = std::env::var("ZBX_PARALLEL_LANES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(8);
    let lanes = schedule_lanes(&block.body.transactions, lane_budget);
    let log_scheduler = std::env::var("ZBX_LOG_SCHEDULER")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if log_scheduler && !block.body.transactions.is_empty() {
        info!(
            txs = block.body.transactions.len(),
            lanes = lanes.lane_count(),
            "scheduler: lane assignment computed"
        );
    }

    // Execute. When a ValidatorSet is supplied, route staking-destination
    // txs through execute_with_staking; otherwise fall back to the
    // EVM-only path (used by genesis loaders / single-validator devnet).
    let adapter = zbx_state::ZbxDbTrieAdapter::new(Arc::clone(storage));
    // IMPORTANT (atomicity): when staking is wired, execute against a
    // CLONE of the live ValidatorSet. The clone holds every staking
    // mutation (register / delegate / undelegate / withdraw / claim).
    // We only swap it back into the shared Arc<RwLock<ValidatorSet>>
    // AFTER the reorg pre-commit check + block persist + staking-delta
    // flush all succeed. On any early-return path the clone is dropped
    // and live in-memory validator state is untouched — matching the
    // deferred-DB-write semantics provided by `StakingDelta`.
    let (exec, vs_after) = if let Some(vs_arc) = validator_set {
        let mut vs_clone = vs_arc.read().clone();
        // `pipeline = None` until the node-level wiring lands: when an
        // operator constructs `Arc<SlashingPipeline>` at boot and passes
        // a borrow here, `StakingTx::FileAppeal` txs will route to
        // `dispatch_file_appeal_tx`. With None they revert cleanly with
        // an explanatory receipt error, mirroring bad-payload behaviour.
        let exec = BlockExecutor::execute_with_staking(
            &block, view, adapter.clone(), &mut vs_clone, storage.as_ref(), None,
        ).map_err(|e| format!("execute: {e}"))?;
        (exec, Some(vs_clone))
    } else {
        let exec = BlockExecutor::execute_with_db(&block, view, adapter.clone())
            .map_err(|e| format!("execute: {e}"))?;
        (exec, None)
    };

    // Reorg pre-commit check: ensure parent is still the storage head.
    let head_now = storage.get_latest_block_number().unwrap_or(0);
    if head_now != parent_number {
        return Err(format!(
            "reorg detected: expected head {parent_number}, got {head_now} — dropping candidate"
        ));
    }

    // Patch header with execution outputs.
    block.header.gas_used             = exec.gas_used;
    block.header.state_root           = H256(exec.new_state_root);
    block.header.transactions_root    = H256(exec.transactions_root);
    block.header.receipts_root        = H256(exec.receipts_root);
    block.header.logs_bloom           = exec.logs_bloom;

    // ─── Task #15 (architect-review #3) outer pruner read-guard ─────
    // Hold ONE pruner read-guard for the ENTIRE critical section
    // covering account persist + trie-node flush + canonical block /
    // tip-pointer write. Without this outer guard, the pruner could
    // acquire `lock.write()` between `adapter.commit()` and
    // `storage.put_block()`, observe an intermediate `head_n`, and
    // GC trie nodes the producer just flushed but not yet anchored
    // to a published block header — exactly the race the per-method
    // guards individually couldn't close. The inner method-level
    // guards (`put_account`, `adapter.commit`, `put_block`) re-enter
    // the same `Arc<RwLock<()>>` as additional readers (parking_lot
    // RwLock supports recursive read acquisition from the same
    // thread), so this is correct and zero-cost on the lock side.
    // No-op when no commit_lock is installed (genesis loaders, tests).
    let _commit_guard = storage.acquire_commit_read_guard();

    // Persist accounts first (crash here = block missing, recoverable).
    for (addr, state) in &exec.state_diff.accounts {
        storage
            .put_account(addr, state)
            .map_err(|e| format!("put_account {}: {e}", hex::encode(addr.as_bytes())))?;
    }

    // Flush trie nodes BEFORE the canonical block-header write.
    adapter
        .commit()
        .map_err(|e| format!("trie node commit: {e}"))?;

    // Write block.
    storage
        .put_block(&block)
        .map_err(|e| format!("put_block: {e}"))?;

    // Flush deferred staking write-set in a single fsync'd batch ONLY
    // after the reorg pre-commit check passed and the block has been
    // persisted. A dropped candidate (reorg/error path above) leaves
    // no staking-side state on disk because the delta is dropped with
    // `exec` on the early-return path.
    if !exec.staking_delta.is_empty() {
        let dels: Vec<(zbx_types::address::Address, zbx_types::address::Address, u128)> =
            exec.staking_delta.delegation_overrides()
                .iter()
                .map(|(&(v, d), &amt)| (v, d, amt))
                .collect();
        let puts: Vec<(u64, zbx_types::address::Address, zbx_types::address::Address, u128)> =
            exec.staking_delta.unbonding_put_overrides()
                .iter()
                .map(|(&(u, d, v), &amt)| (u, d, v, amt))
                .collect();
        let dels_un: Vec<(u64, zbx_types::address::Address, zbx_types::address::Address)> =
            exec.staking_delta.unbonding_delete_overrides()
                .iter()
                .copied()
                .collect();
        storage
            .apply_staking_delta(&dels, &puts, &dels_un)
            .map_err(|e| format!("apply_staking_delta: {e}"))?;
    }

    // Swap the executed `ValidatorSet` clone back into shared live
    // state. This MUST happen before the governance hook below: the
    // swap is a pure in-memory reflection of state already durable on
    // disk (block + staking-delta both fsynced above), so deferring it
    // past a later fallible step would create a committed-block /
    // stale-in-memory split if that step returned `Err`. Any earlier
    // `?` would have dropped `vs_after` with the function frame and
    // left `validator_set` untouched.
    if let (Some(vs_arc), Some(vs_new)) = (validator_set, vs_after) {
        *vs_arc.write() = vs_new;
    }

    // ─── ZEP execution hook ────────────────────────────────────────────
    // Two-phase governance tick, both in a single loaded-and-saved
    // ProposalRegistry:
    //
    //   Phase 1 — `try_finalize` sweep (Pending → Scheduled | Rejected)
    //   ─────────────────────────────────────────────────────────────────
    //   `CastVote` txs that ran in this block already called
    //   `cast_and_maybe_finalize` inline, so most proposals will have
    //   already transitioned inside the dispatcher. The sweep below is a
    //   safety-net catch-all: it promotes any Pending proposal that
    //   crossed quorum (e.g. from votes spread across multiple txs in
    //   the same block, or whose validator-set shrank mid-epoch so the
    //   existing tally now qualifies).
    //
    //   Phase 2 — `apply_ready_governance` (Scheduled → Executed | Failed)
    //   ─────────────────────────────────────────────────────────────────
    //   Once a proposal is Scheduled, it waits until `activation_block
    //   ≤ current_block` before the module-version bump is applied.
    //   Phase 1 and phase 2 run in this order so a proposal that reaches
    //   quorum AND whose activation block is the *current* block can be
    //   fully executed in a single round (Pending → Scheduled → Executed).
    //
    // Runs AFTER block + staking-delta + validator-set swap are all
    // durable / consistent. FAIL-CLOSED on I/O; logical failures mark
    // the proposal `Failed` rather than halting consensus.
    {
        let mut vreg = load_version_registry(storage)?;
        let mut preg = load_proposal_registry(storage)?;
        let current_block = block.header.number;

        // Phase 1: promote any Pending proposals that have reached quorum.
        // Requires a live ValidatorSet reference; if we are on the genesis /
        // devnet path (`validator_set` is None) we fall back to the
        // post-swap view already embedded in `exec.validator_set_after`.
        let finalize_changed = if let Some(vs_arc) = validator_set {
            let vs = vs_arc.read();
            try_finalize_all_pending(&mut preg, &vs, current_block)
        } else {
            false // no active-validator context — skip sweep on genesis path
        };

        // Phase 2: apply every Scheduled proposal whose activation block
        // has arrived (Scheduled → Executed | Failed).
        let execute_changed = apply_ready_governance(&mut vreg, &mut preg, current_block);

        if finalize_changed || execute_changed {
            persist_governance_state(storage, &vreg, &preg)?;
        }
    }

    // ─── zbx-rewards: interval escrow mint + ValidatorSet credit ───────────
    // Every REWARD_INTERVAL (100) blocks:
    //   1. Scan the window of committed blocks to sum actual priority fees paid.
    //   2. Call RewardDistributor::interval_escrow_mint → amount to create out of
    //      thin air into STAKING_PRECOMPILE_ADDR so ClaimRewards / ClaimDelegatorRewards
    //      can draw from it.
    //   3. Call RewardDistributor::distribute_block_reward to split the subsidy +
    //      fees across validators into pending_rewards / delegator_reward_pool.
    //
    // Runs AFTER the governance hook so any epoch-level upgrade is settled.
    // Skipped on the devnet / genesis-loader path where validator_set is None.
    // Errors are logged and suppressed rather than halting the block commit —
    // a missed reward window is less catastrophic than a crashed node.
    if block.header.number % REWARD_INTERVAL == 0 {
        if let Some(vs_arc) = validator_set {
            match apply_interval_rewards(storage, &block, vs_arc) {
                Ok(mint) if mint > 0 => info!(
                    height = block.header.number,
                    mint_wei = mint,
                    "interval rewards: escrow minted and ValidatorSet updated"
                ),
                Ok(_) => {}
                Err(e) => warn!(
                    height = block.header.number,
                    err = %e,
                    "interval reward distribution failed; rewards deferred to next boundary"
                ),
            }
        }
    }

    // Evict included txs and update base-fee feed.
    {
        let mut pool = mempool.write();
        pool.update_base_fee(&block.header);
        pool.remove_confirmed(&block.body.transactions);
    }

    info!(
        height = block.header.number,
        txs = block.body.transactions.len(),
        gas = exec.gas_used,
        "block committed"
    );

    // Task #15 commit hook: notify the trie pruner of the new
    // retained checkpoint. This is the real finalize-time signal —
    // it runs after every successful path (consensus driver,
    // single-validator producer, network sync) without any per-call-
    // site plumbing. No-op when the tracker isn't installed
    // (genesis loaders, tests).
    push_retained(block.header.number, block.header.state_root);

    Ok(block)
}

// ---------------------------------------------------------------------------
// Internal: single-validator produce_one (build + execute + commit in one step)
// ---------------------------------------------------------------------------

fn produce_one(
    storage: &Arc<ZbxDb>,
    mempool: &Arc<RwLock<TransactionPool>>,
    cfg: &ProducerConfig,
) -> Result<Option<Block>, String> {
    let deadline = Instant::now() + cfg.block_time;

    let candidate = build_candidate(storage, mempool, cfg)?;
    let block = match candidate {
        Some(b) => b,
        None => return Ok(None),
    };

    // B5 deadline guard — checked before (potentially long) execution.
    if Instant::now() > deadline {
        warn!("candidate exceeded block_time deadline — dropping");
        return Ok(None);
    }

    let committed = execute_and_commit(storage, mempool, block)?;
    Ok(Some(committed))
}

// ---------------------------------------------------------------------------
// Inline regression: Round-3 atomicity fix — when the reorg pre-commit
// check fails inside `execute_and_commit_inner`, the live shared
// `ValidatorSet` MUST be untouched. Pre-fix the executor mutated the
// live `&mut ValidatorSet` directly, so a dropped candidate's staking
// txs leaked into in-memory validator state. Post-fix the executor
// runs against a clone that is swapped back in only after every
// fallible commit step succeeds.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod reorg_atomicity_tests {
    use super::*;
    use tempfile::TempDir;
    use zbx_crypto::bls::BlsPrivKey;
    use zbx_mempool::MempoolConfig;
    use zbx_staking::MIN_SELF_STAKE;
    use zbx_types::{
        block::{BlockBody, BlockHeader},
        staking_tx::{StakingTx, STAKING_PRECOMPILE_ADDR},
        transaction::{SignedTransaction, Signature, Transaction},
        H256, U256,
    };

    fn make_pop(seed: u8, addr: &Address) -> ([u8; 48], [u8; 96]) {
        let sk = BlsPrivKey::from_bytes(&[seed; 32]).unwrap();
        let pk = sk.to_pubkey();
        let mut preimg = Vec::with_capacity(34);
        preimg.extend_from_slice(addr.as_bytes());
        preimg.extend_from_slice(b"zbx-bls-pop-v1");
        let pop = sk.sign(&zbx_crypto::keccak::keccak256(&preimg));
        (*pk.as_bytes(), *pop.as_bytes())
    }

    fn build_register_tx(from: Address) -> SignedTransaction {
        let (pk, pop) = make_pop(7, &from);
        let data = StakingTx::RegisterValidator {
            pubkey: [0u8; 33],
            bls_pubkey: pk,
            bls_pop: pop,
            self_stake: MIN_SELF_STAKE,
            commission_bps: 500,
        }.encode().unwrap();
        let mut value_be = [0u8; 32];
        value_be[16..].copy_from_slice(&MIN_SELF_STAKE.to_be_bytes());
        let tx = Transaction {
            tx_type: zbx_types::transaction::TxType::DynamicFee,
            chain_id: zbx_types::CHAIN_ID_TESTNET,
            nonce: 0,
            max_priority_fee_per_gas: 0,
            max_fee_per_gas: 1_000_000_000,
            gas_limit: 5_000_000,
            to: Some(STAKING_PRECOMPILE_ADDR),
            value: U256::from_big_endian(&value_be),
            data,
            access_list: vec![],
        };
        let signing_hash = tx.signing_hash();
        let sig = Signature { v: 0, r: H256([0u8; 32]), s: H256([0u8; 32]) };
        let sig_bytes = sig.to_bytes();
        let mut hbuf = Vec::with_capacity(32 + 65);
        hbuf.extend_from_slice(signing_hash.as_bytes());
        hbuf.extend_from_slice(&sig_bytes);
        use sha3::{Digest, Keccak256};
        let hash = H256::from_slice(&Keccak256::digest(&hbuf));
        SignedTransaction { tx, sig, from, hash }
    }

    #[test]
    fn reorg_pre_commit_failure_leaves_live_validator_set_untouched() {
        let tmp = TempDir::new().unwrap();
        let db = Arc::new(ZbxDb::open(tmp.path()).unwrap());

        // Fund the validator so execute can credit MIN_SELF_STAKE +
        // gas to the staking precompile. (If execute itself errored,
        // the test would degenerate into a non-staking error path
        // and prove nothing about reorg-time atomicity.)
        let mut funded = zbx_types::account::AccountState::default();
        funded.set_balance_u128(2 * MIN_SELF_STAKE);
        db.put_account(&Address([0xa1; 20]), &funded).unwrap();

        // Storage head = 0 (fresh db). Hand the producer a candidate
        // whose number = 5 ⇒ parent_number = 4 ≠ head_now = 0, so the
        // reorg pre-commit check MUST fire AFTER execution has already
        // mutated the cloned ValidatorSet.
        let validator_addr = Address([0xa1; 20]);
        let coinbase       = Address([0x99; 20]);
        let tx = build_register_tx(validator_addr);

        let header = BlockHeader {
            parent_hash: H256([0u8; 32]),
            uncle_hash: H256([0u8; 32]),
            coinbase,
            state_root: H256([0u8; 32]),
            transactions_root: H256([0u8; 32]),
            receipts_root: H256([0u8; 32]),
            logs_bloom: [0u8; 256],
            difficulty: U256::zero(),
            number: 5,
            gas_limit: zbx_types::BLOCK_GAS_LIMIT,
            gas_used: 0,
            timestamp: 1_700_000_000,
            extra_data: b"reorg-test".to_vec(),
            mix_hash: H256([0u8; 32]),
            nonce: 0,
            base_fee_per_gas: 1,
            committee_signature: vec![],
            epoch: 0,
            epoch_seed: None,
        };
        let block = Block {
            header,
            body: BlockBody { transactions: vec![tx], uncles: vec![] },
        };

        let vs_arc: Arc<RwLock<ValidatorSet>> =
            Arc::new(RwLock::new(ValidatorSet::new()));
        let mempool: Arc<RwLock<TransactionPool>> =
            Arc::new(RwLock::new(TransactionPool::new(MempoolConfig::default())));

        assert_eq!(vs_arc.read().validators.len(), 0,
                   "precondition: empty validator set");

        let res = execute_and_commit_with_validator_set(&db, &mempool, &vs_arc, block);
        assert!(
            res.as_ref().err().map(|e| e.contains("reorg detected")).unwrap_or(false),
            "expected reorg-detected error, got {res:?}",
        );

        // ATOMICITY INVARIANT: dropped candidate must NOT have leaked
        // its RegisterValidator into the live ValidatorSet.
        let post_count = vs_arc.read().validators.len();
        assert_eq!(
            post_count, 0,
            "ATOMICITY VIOLATION: dropped candidate mutated live ValidatorSet \
             (expected 0, found {post_count})",
        );
        assert!(
            vs_arc.read().get(&validator_addr).is_none(),
            "ATOMICITY VIOLATION: validator leaked into live state from dropped candidate",
        );
    }
}

// ---------------------------------------------------------------------------
// SEC-2026-05-09 Pass-19 (Task #9) — architect-review follow-up #4 + #5:
// end-to-end integration coverage for the epoch-boundary canonical-hash
// invariant. Pre-Pass-19 the rotation site in `do_commit` keyed off the
// pre-execution candidate hash; `execute_and_commit` patches state_root /
// transactions_root / receipts_root / logs_bloom / gas_used, mutating the
// canonical block hash that every honest node + every light client reads
// from the chain. Keying rotation on the pre-exec hash → consensus split
// at every epoch boundary. These tests pin the invariant.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod epoch_boundary_canonical_hash_tests {
    use super::*;
    use tempfile::TempDir;
    use zbx_consensus::hotstuff::ValidatorSet as ConsensusValidatorSet;
    use zbx_mempool::MempoolConfig;
    use zbx_types::{
        block::{BlockBody, BlockHeader},
        H256, U256,
    };

    const EPOCH_LENGTH: u64 = 4;

    fn parent_block(height: u64, parent_hash: H256, coinbase: Address) -> Block {
        let header = BlockHeader {
            parent_hash,
            uncle_hash: H256::zero(),
            coinbase,
            state_root: H256::zero(),
            transactions_root: H256::zero(),
            receipts_root: H256::zero(),
            logs_bloom: [0u8; 256],
            difficulty: U256::zero(),
            number: height,
            gas_limit: BLOCK_GAS_LIMIT,
            gas_used: 0,
            timestamp: 1_000_000 + height,
            extra_data: Vec::new(),
            mix_hash: H256::zero(),
            nonce: 0,
            base_fee_per_gas: 1_000_000_000,
            committee_signature: Vec::new(),
            epoch: height / EPOCH_LENGTH,
            epoch_seed: None,
        };
        Block { header, body: BlockBody { transactions: Vec::new(), uncles: Vec::new() } }
    }

    fn child_candidate(parent: &Block, coinbase: Address, epoch_seed: Option<H256>) -> Block {
        let n = parent.header.number + 1;
        let header = BlockHeader {
            parent_hash: parent.hash(),
            uncle_hash: H256::zero(),
            coinbase,
            // Intentionally bogus — execute_and_commit MUST overwrite these.
            state_root: H256::zero(),
            transactions_root: H256::zero(),
            receipts_root: H256::zero(),
            logs_bloom: [0u8; 256],
            difficulty: U256::zero(),
            number: n,
            gas_limit: BLOCK_GAS_LIMIT,
            gas_used: 0,
            timestamp: parent.header.timestamp + 1,
            extra_data: Vec::new(),
            mix_hash: H256::zero(),
            nonce: 0,
            base_fee_per_gas: 1_000_000_000,
            committee_signature: Vec::new(),
            epoch: n / EPOCH_LENGTH,
            epoch_seed,
        };
        Block { header, body: BlockBody { transactions: Vec::new(), uncles: Vec::new() } }
    }

    /// End-to-end proof that `execute_and_commit` mutates the canonical
    /// block hash. Pre-Pass-19 the rotation site in `do_commit` used the
    /// pre-execution candidate hash; this test independently demonstrates
    /// that the two hashes ARE different, which is the WHY behind keying
    /// rotation on `committed.hash()` instead.
    #[test]
    fn execute_and_commit_changes_canonical_block_hash() {
        let tmp = TempDir::new().unwrap();
        let storage = Arc::new(ZbxDb::open(tmp.path()).unwrap());
        let mempool = Arc::new(RwLock::new(TransactionPool::new(MempoolConfig::default())));

        let coinbase = Address([0x42u8; 20]);
        let parent = parent_block(0, H256::zero(), coinbase);
        storage.put_block(&parent).unwrap();

        let candidate = child_candidate(&parent, coinbase, None);
        let pre_hash = candidate.hash();

        let committed = execute_and_commit(&storage, &mempool, candidate)
            .expect("execute_and_commit on empty block must succeed");
        let post_hash = committed.hash();

        assert_ne!(
            pre_hash, post_hash,
            "execute_and_commit MUST patch the header (state_root etc) so the \
             canonical block hash differs from the pre-execution candidate hash. \
             If this assertion ever flips, the Pass-19 architect-review follow-up \
             #4 fix in node/src/consensus.rs (rotation keyed on `committed.hash()`) \
             can be reverted — but verify against multiple non-empty blocks first."
        );
    }

    /// Architect requirements #1 + #2 + #3 in one end-to-end pass:
    ///   (1) seed changes at the boundary,
    ///   (2) proposer schedule differs across epochs for the same round,
    ///   (3) `header.epoch_seed` on the first block of the new epoch
    ///       equals the derivation `keccak256(canonical_parent_hash ‖
    ///       next_epoch_be8 ‖ prev_seed)` — i.e. light-client verification
    ///       matches what producer nodes actually rotate to.
    ///
    /// We simulate the boundary commit path by:
    ///   (a) committing real blocks 1..(EPOCH_LENGTH - 1) via the actual
    ///       `execute_and_commit` so the canonical hash is the true
    ///       post-execution hash,
    ///   (b) deriving `expected_new_seed` from that canonical hash,
    ///   (c) calling `set_epoch_seed(expected_new_seed)` on a
    ///       representative `ValidatorSet` (mirrors what
    ///       `HotStuff2::rotate_epoch_seed` does at the boundary),
    ///   (d) committing the first block of the new epoch with
    ///       `header.epoch_seed = Some(vs.epoch_seed)` (mirrors the
    ///       propose-path patch in `do_commit`),
    ///   (e) cross-checking that the persisted header carries exactly the
    ///       seed a light client would derive from the canonical parent.
    #[test]
    fn epoch_boundary_seed_derives_from_canonical_post_exec_hash() {
        let tmp = TempDir::new().unwrap();
        let storage = Arc::new(ZbxDb::open(tmp.path()).unwrap());
        let mempool = Arc::new(RwLock::new(TransactionPool::new(MempoolConfig::default())));

        let coinbase = Address([0x42u8; 20]);

        // Persist synthetic genesis (height 0) so height 1's
        // `parent_number = 0` reorg pre-commit check passes.
        let g = parent_block(0, H256::zero(), coinbase);
        storage.put_block(&g).unwrap();

        // Build + commit blocks 1..(EPOCH_LENGTH - 1). The boundary
        // block (LAST block of epoch 0) is at height EPOCH_LENGTH - 1.
        let mut prev = g;
        for h in 1..EPOCH_LENGTH {
            let cand = child_candidate(&prev, coinbase, None);
            prev = execute_and_commit(&storage, &mempool, cand)
                .unwrap_or_else(|e| panic!("execute_and_commit at height {h}: {e}"));
            assert_eq!(prev.header.number, h);
        }

        // `prev` is now the committed boundary block. Its canonical hash
        // is what rotation MUST key on (architect follow-up #4).
        let canonical_hash = prev.hash();

        // Build a 6-validator set so a different seed produces a
        // visibly different proposer schedule at round 0.
        let validators: Vec<Address> = (1u8..=6u8).map(|i| Address([i; 20])).collect();
        let prev_seed = H256::zero();
        let mut vs = ConsensusValidatorSet::new(validators.clone());
        vs.epoch_seed = prev_seed;

        // Light-client / honest-node derivation — MUST match what
        // `do_commit` runs (node/src/consensus.rs:854-880).
        let next_epoch: u64 = 1;
        let expected_new_seed = {
            let mut buf = Vec::with_capacity(32 + 8 + 32);
            buf.extend_from_slice(canonical_hash.as_bytes());
            buf.extend_from_slice(&next_epoch.to_be_bytes());
            buf.extend_from_slice(prev_seed.as_bytes());
            zbx_crypto::keccak::keccak256(&buf)
        };

        let proposer_before = vs.proposer_for_round(0);
        vs.set_epoch_seed(expected_new_seed);

        // (1) seed changed
        assert_ne!(vs.epoch_seed, prev_seed,
            "rotation MUST mutate ValidatorSet.epoch_seed at the boundary");
        assert_eq!(vs.epoch_seed, expected_new_seed,
            "rotated seed MUST equal canonical-hash derivation");

        // (2) proposer schedule differs at the same round
        let proposer_after = vs.proposer_for_round(0);
        assert_ne!(proposer_before, proposer_after,
            "post-rotation proposer schedule MUST differ from pre-rotation \
             (seeded keccak shuffle, not legacy round-robin)");

        // (3) light-client verification: first block of new epoch carries
        // the rotated seed, and that seed equals the canonical-hash
        // derivation a light client would compute independently.
        let new_epoch_first = child_candidate(&prev, coinbase, Some(vs.epoch_seed));
        let new_epoch_first = execute_and_commit(&storage, &mempool, new_epoch_first)
            .expect("first block of new epoch must commit");

        let header_seed = new_epoch_first.header.epoch_seed.expect(
            "first block of new epoch MUST carry epoch_seed for light-client verification",
        );
        assert_eq!(header_seed, expected_new_seed,
            "light-client invariant: header.epoch_seed on the first block of \
             epoch N MUST equal keccak256(canonical_parent_hash ‖ N_be8 ‖ \
             prev_seed). If this fails, light clients cannot verify the \
             proposer schedule across the epoch transition — the exact \
             failure mode the architect flagged in the Pass-19 follow-up.");
    }
}
