//! Sequential block executor.

use crate::{
    bloom::{aggregate_block_bloom, compute_receipt_bloom, compute_receipts_root, compute_tx_root},
    error::ExecutionError,
    state_diff::StateDiff,
};
use crate::host_zvm::{ProductionZvmHost, TransientScratchpad, ZvmBlockEnv};
use zbx_types::{
    account::{AccountState, VmKind},
    address::Address,
    block::{Block, BlockHeader},
    receipt::{Log, TransactionReceipt, TxStatus},
    transaction::SignedTransaction,
    H256,
    BLOCK_GAS_LIMIT,
};
use zbx_crypto::keccak::keccak256;
use zbx_evm::{EVMContext, EVMInterpreter, ExitStatus, MockHost};
use zbx_evm::interpreter::EvmLog;
use zbx_zvm::{
    context::{ExecutionStatus as ZvmExecutionStatus, ZvmContext},
    interpreter::ZvmInterpreter,
};
use tracing::{debug, info};

/// Discriminator byte marking an init-code payload as targeting the
/// ZVM interpreter. CREATE / CREATE2 strips it before storing the
/// runtime code; the resulting account is persisted as `VmKind::Zvm`.
pub const ZVM_DEPLOY_DISCRIMINATOR: u8 = 0x5A;

/// Result of executing a full block.
///
/// Sprint S33 (2026-05-02) added `transactions_root`, `receipts_root`, and
/// `logs_bloom` so the producer no longer hardcodes them to all-zero in the
/// header. See `crates/zbx-execution/src/bloom.rs` for the underlying
/// computations and the audit close-out under findings N-01, N-02, S7-PROD1.
#[derive(Debug)]
pub struct ExecutionResult {
    pub receipts: Vec<TransactionReceipt>,
    pub state_diff: StateDiff,
    pub gas_used: u64,
    pub new_state_root: [u8; 32],
    /// Binary-Merkle root over the block's signed transactions.
    /// `[0u8; 32]` for an empty block. Closes S7-PROD1.
    pub transactions_root: [u8; 32],
    /// Binary-Merkle root over the block's receipts (length-prefixed
    /// canonical encoding). `[0u8; 32]` for an empty receipts list. Closes
    /// N-01.
    pub receipts_root: [u8; 32],
    /// Block-level Bloom filter — bitwise OR of every per-receipt
    /// `logs_bloom`. Closes N-02 (paired with the per-receipt bloom now
    /// populated inside the receipts loop).
    pub logs_bloom: [u8; 256],
    /// Pending staking-side persistence ops accumulated during block
    /// execution. The producer flushes via `ZbxDb::apply_staking_delta`
    /// only after the reorg pre-commit check passes — so a dropped
    /// candidate block leaves no staking-side state on disk.
    pub staking_delta: zbx_staking::StakingDelta,
}

/// Reads state from a snapshot and accumulates diffs.
pub struct StateView {
    base: std::collections::HashMap<Address, AccountState>,
    /// Pre-resolved bytecodes keyed by their keccak256 hash. The producer
    /// seeds this from `ZbxDb::get_code` for every contract account it knows
    /// will be touched in the block — without a code-resolver here, contract
    /// CALLs would silently no-op (which the architect review flagged).
    codes: std::collections::HashMap<H256, Vec<u8>>,
    diffs: StateDiff,
    /// Task #8 (EIP-6780): tombstoned addresses. The executor end-of-tx
    /// drain inserts here when a SELFDESTRUCT targets an address that
    /// was CREATEd in the same tx. `state_root` and `state_root_with_db`
    /// filter the visible-set against this. Mirrors `StateDB::to_delete`.
    to_delete: std::collections::HashSet<Address>,
}

impl StateView {
    pub fn new() -> Self {
        StateView {
            base: std::collections::HashMap::new(),
            codes: std::collections::HashMap::new(),
            diffs: StateDiff::new(),
            to_delete: std::collections::HashSet::new(),
        }
    }

    /// Task #8 (EIP-6780): mark `addr` for full deletion at end-of-block
    /// state-root computation. Removes any pending dirty entry in
    /// `diffs.accounts` and any pending storage deltas (the account
    /// will be dropped from the trie entirely so its storage root is
    /// moot). Idempotent.
    ///
    /// Caller MUST ensure this is only invoked when the address was
    /// created in the current tx (EIP-6780 gate); the executor's
    /// end-of-tx drain does this gating.
    pub fn selfdestruct(&mut self, addr: Address) {
        self.diffs.accounts.remove(&addr);
        self.diffs.storage.remove(&addr);
        self.diffs.deleted.push(addr);
        self.to_delete.insert(addr);
    }

    /// Whether `addr` has been tombstoned this block via
    /// [`Self::selfdestruct`]. Test helper.
    pub fn is_deleted(&self, addr: &Address) -> bool {
        self.to_delete.contains(addr)
    }

    pub fn seed_account(&mut self, addr: Address, state: AccountState) {
        self.base.insert(addr, state);
    }

    /// Pre-load a contract's runtime bytecode keyed by its `code_hash`.
    pub fn seed_code(&mut self, code_hash: H256, code: Vec<u8>) {
        self.codes.insert(code_hash, code);
    }

    /// Look up the runtime bytecode for an account. Returns an empty slice
    /// for EOAs or for unseeded contract accounts (which the executor will
    /// then treat as STOP).
    pub fn get_code(&self, addr: &Address) -> Vec<u8> {
        let acct = self.get_account(addr);
        if !acct.is_contract() {
            return Vec::new();
        }
        self.codes.get(&acct.code_hash).cloned().unwrap_or_default()
    }

    pub fn get_account(&self, addr: &Address) -> AccountState {
        // Task #8 (EIP-6780): tombstoned addresses return Default
        // (fresh slot) so any post-deletion read in the same block
        // sees an empty account, matching `StateDB::get_account`.
        if self.to_delete.contains(addr) {
            return AccountState::default();
        }
        self.diffs.accounts.get(addr).cloned()
            .unwrap_or_else(|| self.base.get(addr).cloned().unwrap_or_default())
    }

    /// Modify account state.
    ///
    /// SEC-2026-05-09 (Pass-6 C4 — architect-flagged coverage gap):
    /// the production block-execution commit path persists
    /// `state_diff.accounts` straight to `ZbxDb::put_account`, so the
    /// invariant guards must live HERE — not only on
    /// `zbx-state::StateDB::set_account`.  Without this layer a buggy
    /// executor branch could write an inconsistent diff that gets
    /// committed verbatim, silently desyncing on-disk state from the
    /// MPT root.
    ///
    /// Same two invariants as `StateDB::set_account`:
    ///   1. Nonce monotonicity — `state.nonce >= prior.nonce`.
    ///   2. EIP-684 code immutability — once `prior.is_contract()`, the
    ///      only allowed `code_hash` change is to `EMPTY_CODE_HASH`
    ///      (selfdestruct path).  StateView does not yet implement
    ///      self-destruct (no `to_delete` field — see `state_root`
    ///      doc), so the recreate-after-selfdestruct exemption does
    ///      not apply here today.
    ///
    /// Behaviour on violation: panic in debug, log+clamp in release.
    pub fn set_account(&mut self, addr: Address, state: AccountState) {
        // Task #8 (EIP-6780): if the address was tombstoned this
        // block, treat the slot as fresh — un-tombstone and use
        // Default as `prior` so the standard "fresh account / EOA
        // upgrade to contract" path applies. This mirrors
        // `StateDB::set_account` after `StateDB::selfdestruct`.
        // Architect-review follow-up: also drop the matching
        // `diffs.deleted` entry so downstream consumers of
        // `state_diff.deleted` don't see a stale delete-intent for
        // an address that's been resurrected.
        if self.to_delete.remove(&addr) {
            self.diffs.deleted.retain(|a| a != &addr);
            self.diffs.accounts.insert(addr, state);
            return;
        }
        let prior = self.diffs.accounts.get(&addr).cloned()
            .unwrap_or_else(|| self.base.get(&addr).cloned().unwrap_or_default());

        // (1) Nonce monotonicity.
        if state.nonce < prior.nonce {
            #[cfg(debug_assertions)]
            panic!(
                "SEC-2026-05-09 Pass-6 C4 (StateView): nonce regression on {:?} \
                 (prior={}, attempted={})",
                addr, prior.nonce, state.nonce
            );
            #[cfg(not(debug_assertions))]
            {
                tracing::error!(
                    target: "executor",
                    ?addr, prior_nonce = prior.nonce, attempted_nonce = state.nonce,
                    "SEC-2026-05-09 Pass-6 C4: refusing nonce regression in StateView — keeping prior nonce"
                );
                let mut fixed = state;
                fixed.nonce = prior.nonce;
                self.diffs.accounts.insert(addr, fixed);
                return;
            }
        }

        // (2) EIP-684 code immutability.
        let prior_is_contract = prior.code_hash != zbx_types::account::EMPTY_CODE_HASH;
        if !prior_is_contract {
            // Fresh account or EOA upgrading to contract — allowed.
        } else if state.code_hash == prior.code_hash {
            // No-op.
        } else if state.code_hash == zbx_types::account::EMPTY_CODE_HASH {
            tracing::warn!(
                target: "executor",
                ?addr, prior_code = ?prior.code_hash,
                "SEC-2026-05-09 Pass-6 C4: code hash cleared in StateView — verify this is a selfdestruct path"
            );
        } else {
            #[cfg(debug_assertions)]
            panic!(
                "SEC-2026-05-09 Pass-6 C4 (StateView): contract code_hash mutation on {:?} \
                 ({:?} → {:?}) — violates EIP-684",
                addr, prior.code_hash, state.code_hash
            );
            #[cfg(not(debug_assertions))]
            {
                tracing::error!(
                    target: "executor",
                    ?addr,
                    prior_code = ?prior.code_hash,
                    attempted_code = ?state.code_hash,
                    "SEC-2026-05-09 Pass-6 C4: refusing contract code_hash mutation in StateView — keeping prior code"
                );
                let mut fixed = state;
                fixed.code_hash = prior.code_hash;
                self.diffs.accounts.insert(addr, fixed);
                return;
            }
        }

        self.diffs.accounts.insert(addr, state);
    }

    pub fn get_storage(&self, addr: &Address, slot: &[u8; 32]) -> [u8; 32] {
        let key = H256(*slot);
        self.diffs.storage.get(addr).and_then(|s| s.get(&key)).copied()
            .unwrap_or(H256([0u8; 32])).0
    }

    pub fn set_storage(&mut self, addr: Address, slot: [u8; 32], value: [u8; 32]) {
        self.diffs.storage.entry(addr).or_default().insert(H256(slot), H256(value));
    }

    pub fn emit_log(&mut self, log: Log) {
        self.diffs.logs.push(log);
    }

    pub fn into_diff(self) -> StateDiff { self.diffs }

    /// Compute the world-state Merkle-Patricia Trie root over the visible
    /// account set (S33-state-root W3a — was a flat keccak placeholder).
    ///
    /// Delegates to `zbx_state::mpt::compute_state_root_filtered` so this
    /// path produces identical roots to `zbx_state::StateDB::state_root()`
    /// for identical inputs. The W3a invariant: the executor and the
    /// state DB never disagree on the canonical state root.
    ///
    /// # Storage view
    ///
    /// `StateView::diffs.storage` uses `[u8; 32]` keys/values whereas the
    /// shared MPT helper expects `H256`. We rebuild the storage map with
    /// `H256` wrappers per call. This is O(slots-touched) and runs only
    /// once per block, matching the existing `state_root()` call cadence.
    ///
    /// # SELFDESTRUCT
    ///
    /// `StateView` does not yet implement self-destruct (no `to_delete`
    /// field), so we pass an empty set. When self-destruct lands on the
    /// executor side it'll feed the same set the StateDB uses.
    pub fn state_root(&self) -> [u8; 32] {
        use std::collections::{HashMap as Map, HashSet as Set};
        use zbx_types::H256;

        // Convert the [u8; 32]-keyed storage cache to H256-keyed for the
        // shared helper. Single allocation per call.
        let storage_h256: Map<Address, Map<H256, H256>> = self
            .diffs
            .storage
            .iter()
            .map(|(addr, slots)| {
                let h256_slots: Map<H256, H256> = slots
                    .iter()
                    .map(|(slot, value)| (*slot, *value))
                    .collect();
                (*addr, h256_slots)
            })
            .collect();

        // Task #8 (EIP-6780): exclude self-destructed addresses from
        // the visible-set so they vanish from the state root, exactly
        // matching `StateDB::state_root`'s `to_delete` filter.
        let _ = Set::<Address>::new();
        let root = zbx_state::mpt::compute_state_root_filtered(
            &self.base,
            &self.diffs.accounts,
            &storage_h256,
            &self.to_delete,
        );
        root.0
    }

    /// Persistent variant of [`Self::state_root`] (S33-state-root W3b
    /// production wire-up, architect-required closure of C-09).
    ///
    /// Computes the canonical YP §4.1 state root using the supplied
    /// persistent `TrieDB` (typically a `ZbxDbTrieAdapter`). Per-account
    /// storage tries are reopened via `MutableTrie::from_root(account.storage_root, db)`
    /// so the W2/W3a "honest limitation" — divergence on partial-overwrite
    /// blocks where pre-existing slots were not in cache — is closed.
    ///
    /// # Caller contract
    ///
    /// On success, the new trie nodes have been **buffered** in the
    /// adapter's pending list but not yet fsynced. The caller MUST call
    /// `db.commit()` (or pass the same adapter to a layer that does)
    /// before persisting the block header — otherwise the header will
    /// commit to a state root whose justifying trie nodes are not durable.
    ///
    /// # Errors
    ///
    /// Surfaces `TrieError::MissingNode` when `account.storage_root`
    /// references a node that is not yet on disk (cold-start corruption
    /// or missing migration), or any I/O failure from the underlying
    /// `TrieDB`. Block production MUST abort on this error rather than
    /// commit a header to an undefined root.
    pub fn state_root_with_db<DB>(&self, db: DB) -> Result<[u8; 32], zbx_trie::TrieError>
    where
        DB: zbx_trie::TrieDB + Clone,
    {
        use std::collections::{HashMap as Map, HashSet as Set};
        let storage_h256: Map<Address, Map<H256, H256>> = self
            .diffs
            .storage
            .iter()
            .map(|(addr, slots)| {
                let h256_slots: Map<H256, H256> = slots
                    .iter()
                    .map(|(slot, value)| (*slot, *value))
                    .collect();
                (*addr, h256_slots)
            })
            .collect();

        let _ = Set::<Address>::new();
        // Visible-set = (base ∪ dirty) \ to_delete. Task #8 (EIP-6780):
        // self-destructed addresses are filtered out so the persistent
        // root excludes them, matching `StateDB::state_root_with_db`.
        let mut visible: Map<Address, AccountState> = Map::new();
        for (addr, state) in &self.base {
            if !self.to_delete.contains(addr) {
                visible.insert(*addr, state.clone());
            }
        }
        for (addr, state) in &self.diffs.accounts {
            if !self.to_delete.contains(addr) {
                visible.insert(*addr, state.clone());
            }
        }
        let root = zbx_state::mpt::compute_state_root_with_db(
            &visible,
            &storage_h256,
            db,
        )?;
        Ok(root.0)
    }
}

/// Executes transactions in a block sequentially.
pub struct BlockExecutor;

impl BlockExecutor {
    /// Persistent-trie variant of [`Self::execute`] — the production
    /// block-execution path (S33-state-root W3b, architect-required
    /// closure of C-09).
    ///
    /// Identical to `execute` except the final state root is computed
    /// via `view.state_root_with_db(db)`, which writes trie nodes to the
    /// supplied adapter. The caller MUST call `db.commit()` after this
    /// returns and BEFORE persisting the produced block header — see
    /// `node/src/block_producer.rs::produce_one` for the canonical
    /// integration.
    ///
    /// `execute` (in-memory variant) remains for tests + bootstrap
    /// scenarios where no persistent backing is required.
    pub fn execute_with_db<DB>(
        block: &Block,
        view: StateView,
        db: DB,
    ) -> Result<ExecutionResult, ExecutionError>
    where
        DB: zbx_trie::TrieDB + Clone,
    {
        Self::execute_inner(block, view, Some(db), None)
    }

    pub fn execute(block: &Block, view: StateView) -> Result<ExecutionResult, ExecutionError> {
        // None-DB path falls through to the legacy in-memory state_root.
        Self::execute_inner::<zbx_trie::trie::MemoryTrieDB>(block, view, None, None)
    }

    /// Production block-execution path with staking routing wired in.
    /// Identical to `execute_with_db` plus a per-tx branch on
    /// `is_staking_destination`: txs to `STAKING_PRECOMPILE_ADDR` are
    /// dispatched via `execute_staking_tx` (mutating the supplied
    /// `&mut ValidatorSet` + `&ZbxDb`) instead of the EVM interpreter.
    /// Apply a slashing burn through the executor: debit `amount_wei`
    /// from the offender's balance on `view`, transitioning the staking
    /// pipeline's accounting into actual on-state-view balance changes.
    /// Returns the actual amount burned (saturating at the offender's
    /// current balance).
    pub fn apply_slash_burn(view: &mut StateView, offender: Address, amount_wei: u128) -> u128 {
        let mut acct = view.get_account(&offender);
        let bal = acct.balance_u128();
        let actual = amount_wei.min(bal);
        acct.set_balance_u128(bal - actual);
        view.set_account(offender, acct);
        actual
    }

    pub fn execute_with_staking<DB>(
        block: &Block,
        view: StateView,
        db: DB,
        vs: &mut zbx_staking::ValidatorSet,
        staking_db: &zbx_storage::ZbxDb,
        pipeline: Option<&zbx_staking::SlashingPipeline>,
    ) -> Result<ExecutionResult, ExecutionError>
    where
        DB: zbx_trie::TrieDB + Clone,
    {
        Self::execute_inner(block, view, Some(db), Some((vs, staking_db, pipeline)))
    }

    fn execute_inner<DB>(
        block: &Block,
        mut view: StateView,
        db: Option<DB>,
        mut staking_ctx: Option<(&mut zbx_staking::ValidatorSet, &zbx_storage::ZbxDb, Option<&zbx_staking::SlashingPipeline>)>,
    ) -> Result<ExecutionResult, ExecutionError>
    where
        DB: zbx_trie::TrieDB + Clone,
    {
        let base_fee = block.header.base_fee_per_gas;
        let coinbase = block.header.coinbase;
        let mut cumulative_gas = 0u64;
        let mut receipts = Vec::new();
        let mut staking_delta = zbx_staking::StakingDelta::new();

        // Validate block gas limit
        if block.header.gas_limit > BLOCK_GAS_LIMIT {
            return Err(ExecutionError::Validation(
                format!("gas_limit {} > protocol max {}", block.header.gas_limit, BLOCK_GAS_LIMIT)
            ));
        }

        for (idx, tx) in block.body.transactions.iter().enumerate() {
            // Branch on staking destination before the EVM path so txs to
            // STAKING_PRECOMPILE_ADDR mutate the validator set instead of
            // running empty EVM bytecode. With `staking_ctx == None`
            // (legacy `execute`/`execute_with_db` callers) the tx falls
            // through to the EVM path for back-compat.
            let result = if zbx_staking::is_staking_destination(tx.tx.to.as_ref())
                && staking_ctx.is_some()
            {
                let (vs, sdb, pipeline) = staking_ctx.as_mut().expect("checked is_some");
                let sr = execute_staking_tx(tx, &block.header, &mut view, *vs, sdb, &mut staking_delta, *pipeline)?;
                TxResult {
                    gas_used: sr.gas_used,
                    success: sr.success,
                    effective_price: sr.effective_price,
                    contract_address: None,
                    logs: Vec::new(),
                }
            } else {
                Self::execute_tx(tx, &block.header, &mut view)?
            };
            cumulative_gas += result.gas_used;

            // Pay validator
            let tip = result.effective_price.saturating_sub(base_fee);
            let validator_fee = (tip as u128) * (result.gas_used as u128);
            let mut validator_acct = view.get_account(&coinbase);
            let vbal = validator_acct.balance_u128();
            validator_acct.set_balance_u128(vbal.saturating_add(validator_fee));
            view.set_account(coinbase, validator_acct);

            // S33 (2026-05-02) — N-02 fix. Per-receipt logs_bloom is now
            // computed from `result.logs` instead of being hardcoded to
            // `[0u8; 256]`. NB: the EVM-interpreter call site at line 332
            // currently returns `Vec::<Log>::new()` unconditionally — until
            // the interpreter actually pipes LOG0..LOG4 emissions through
            // `result.logs` (tracked separately as a follow-up after S33),
            // the bloom will remain all-zero in normal operation. The
            // computation pipeline is still wired correctly so the moment
            // logs DO start flowing, the bloom and receipts_root respond
            // automatically.
            let receipt_bloom = compute_receipt_bloom(&result.logs);
            receipts.push(TransactionReceipt {
                status: if result.success { TxStatus::Success } else { TxStatus::Failure },
                cumulative_gas_used: cumulative_gas,
                logs_bloom: receipt_bloom,
                logs: result.logs,
                transaction_hash: tx.hash,
                transaction_index: idx as u32,
                block_hash: block.hash(),
                block_number: block.number(),
                from: tx.from,
                to: tx.tx.to,
                contract_address: result.contract_address,
                gas_used: result.gas_used,
                effective_gas_price: result.effective_price,
            });
        }

        // Block reward
        let reward = zbx_types::block_reward_at(block.number());
        if reward > 0 {
            let mut acct = view.get_account(&coinbase);
            let bal = acct.balance_u128();
            acct.set_balance_u128(bal.saturating_add(reward));
            view.set_account(coinbase, acct);
        }

        // S33-state-root W3c production wire-up (architect round-2 closure
        // of C-09): when the caller supplied a persistent `TrieDB`
        // (typically a `ZbxDbTrieAdapter` from the block producer),
        // dispatch the canonical YP §4.1 root to the persistent helper
        // so per-account storage tries are reopened from disk via
        // `MutableTrie::from_root(account.storage_root, db)`. Trie I/O
        // failures map to `ExecutionError::State` which aborts the block
        // — a header committing to a state_root whose justifying nodes
        // are not durable would brick the chain on next restart.
        //
        // The `None` branch keeps the legacy in-memory path for tests
        // and bootstrap callers that don't have a persistent backing.
        let new_state_root = match db {
            Some(d) => view
                .state_root_with_db(d)
                .map_err(|e| ExecutionError::State(format!("trie state_root: {e}")))?,
            None => view.state_root(),
        };

        // S33 (2026-05-02) — N-01 + S7-PROD1 fixes. Compute the three header
        // commitment fields the producer used to hardcode to all-zero:
        //   - `transactions_root` over the block's signed-tx hashes
        //   - `receipts_root` over the just-built receipt list
        //   - `logs_bloom` as the bitwise-OR of every per-receipt bloom
        // All three fall back to all-zero on empty input (canonical
        // sentinel that downstream filters short-circuit on).
        let transactions_root = compute_tx_root(&block.body.transactions);
        let receipts_root = compute_receipts_root(&receipts);
        let logs_bloom = aggregate_block_bloom(&receipts);

        let state_diff = view.into_diff();

        info!(
            height = block.number(),
            txs = block.body.transactions.len(),
            gas = cumulative_gas,
            "block executed"
        );

        Ok(ExecutionResult {
            receipts,
            state_diff,
            gas_used: cumulative_gas,
            new_state_root,
            transactions_root,
            receipts_root,
            logs_bloom,
            staking_delta,
        })
    }

    fn execute_tx(
        tx: &SignedTransaction,
        header: &BlockHeader,
        view: &mut StateView,
    ) -> Result<TxResult, ExecutionError> {
        let base_fee = header.base_fee_per_gas;
        let effective_price = tx.effective_gas_price(base_fee);
        let gas_limit = tx.tx.gas_limit;

        // 1. Pre-flight nonce + balance checks against the sender account.
        let mut sender = view.get_account(&tx.from);
        if tx.tx.nonce != sender.nonce {
            return Err(ExecutionError::InvalidNonce {
                expected: sender.nonce,
                got: tx.tx.nonce,
            });
        }
        let max_cost = (tx.tx.max_fee_per_gas as u128) * (gas_limit as u128);
        let mut value_bytes = [0u8; 32];
        tx.tx.value.to_big_endian(&mut value_bytes);
        let value_u128 = u128::from_be_bytes(
            value_bytes[16..].try_into().unwrap_or([0u8; 16]),
        );
        let total_cost = max_cost.saturating_add(value_u128);
        let balance = sender.balance_u128();
        if total_cost > balance {
            return Err(ExecutionError::InsufficientBalance { balance, cost: total_cost });
        }
        let intrinsic = tx.tx.intrinsic_gas();
        if intrinsic > gas_limit {
            return Err(ExecutionError::IntrinsicGasTooLow {
                required: intrinsic,
                provided: gas_limit,
            });
        }

        // 2. Deduct max gas cost + value, bump nonce. Refund unused gas later.
        sender.set_balance_u128(balance.saturating_sub(max_cost + value_u128));
        sender
            .increment_nonce()
            .map_err(|e| ExecutionError::State(e.to_string()))?;
        view.set_account(tx.from, sender);

        // 3. Resolve the call target: contract creation vs call.
        //    `contract_address` is set on creation so receipts surface it.
        //    `code_to_run` is the bytecode the EVM will execute (init code on
        //    creation, the deployed runtime code on call).
        let mut contract_address: Option<Address> = None;
        let code_to_run: Vec<u8>;
        let callee: Address;
        let mut is_pure_value_transfer = false;
        // Drives the EVM-vs-ZVM dispatch branch below.
        let mut callee_vm = VmKind::Evm;
        match tx.tx.to {
            None => {
                // CREATE: address = keccak256(rlp([sender, nonce-1]))[12..]
                //         (we use a deterministic surrogate until full RLP is wired).
                let mut buf = Vec::with_capacity(28);
                buf.extend_from_slice(tx.from.as_bytes());
                buf.extend_from_slice(&tx.tx.nonce.to_be_bytes());
                let h = keccak256(&buf);
                let mut a = [0u8; 20];
                a.copy_from_slice(&h[12..]);
                let new_addr = Address(a);
                contract_address = Some(new_addr);
                callee = new_addr;
                // Discriminator-byte deploy-time VM selection: a leading
                // 0x5A is stripped and the new account marked Zvm.
                let init = tx.tx.data.as_slice();
                if init.first().copied() == Some(ZVM_DEPLOY_DISCRIMINATOR) {
                    callee_vm = VmKind::Zvm;
                    code_to_run = init[1..].to_vec();
                } else {
                    code_to_run = init.to_vec();
                }
                // Persist the runtime bytecode + code_hash for the new
                // account NOW so a subsequent CALL within the same block
                // (or any later block) can resolve it. We treat the
                // discriminator-stripped tx data AS the runtime bytecode
                // — a deliberate simplification of the EVM init→runtime
                // split that matches this codebase's deploy semantics.
                // Persisting before execution is safe: on revert step 6
                // does not roll back code (mirroring the existing
                // `set_account` for the CREATE address which also
                // persists pre-revert).
                if !code_to_run.is_empty() {
                    let code_hash = keccak256(&code_to_run);
                    view.seed_code(code_hash, code_to_run.clone());
                    let mut new_acct = view.get_account(&new_addr);
                    new_acct.code_hash = code_hash;
                    if callee_vm == VmKind::Zvm {
                        new_acct.vm = VmKind::Zvm;
                    }
                    view.set_account(new_addr, new_acct);
                }
            }
            Some(to) => {
                callee = to;
                let to_acct = view.get_account(&to);
                if to_acct.is_contract() {
                    callee_vm = to_acct.vm;
                    // Resolve runtime bytecode through the state view; the
                    // producer must `seed_code(code_hash, bytes)` for every
                    // contract account that may be called this block. If the
                    // hash is missing the EVM will STOP — same as calling an
                    // empty contract — which is correct for upgradable proxies
                    // pre-init but NOT what we want silently for live ones.
                    code_to_run = view.get_code(&to);
                } else {
                    // Pure value transfer to an EOA: no EVM execution at all.
                    code_to_run = Vec::new();
                    is_pure_value_transfer = true;
                }
            }
        }

        // 4. Run the EVM. For a pure value transfer we charge intrinsic gas
        //    only — no interpreter dispatch — to match Ethereum semantics.
        //
        //    Audit-2026-05-01 S7-EVM2: precompile addresses 0x01..=0x09 have
        //    no deployed code, so step 3 above sets `is_pure_value_transfer
        //    = true` and `code_to_run = Vec::new()` for them. Without this
        //    explicit precompile branch, a top-level transaction to e.g.
        //    ecrecover (0x01) would fall through the `is_pure_value_transfer
        //    || code_to_run.is_empty()` short-circuit below and be treated
        //    as a value transfer — the precompile would never run, so the
        //    caller's intended computation silently produced no output. We
        //    dispatch precompiles BEFORE the short-circuit.
        //
        //    Value-flow note (architect-corrected 2026-05-01): if `tx.value`
        //    is non-zero, step 6 below credits the precompile ADDRESS with
        //    that value on success — exactly the Ethereum-mainnet behaviour
        //    where wei sent to a precompile address is effectively stranded
        //    at that address (the precompile itself has no withdrawal logic,
        //    so the funds become permanently locked). On failure, step 6's
        //    `else` branch refunds the value to the sender. We deliberately
        //    do NOT special-case the value debit for precompile targets:
        //    the existing settlement path is already correct.
        //
        //    Failure semantics mirror `zbx-vm/src/interpreter.rs` precompile
        //    branch: any `Err(_)` consumes ALL forwarded gas (standard EVM
        //    convention for precompile reverts).
        let (exit_status, evm_gas_used, logs) = if tx.tx.to.is_some()
            && zbx_evm::precompiles::is_precompile(&callee)
        {
            let forwarded = gas_limit.saturating_sub(intrinsic);
            let calldata: &[u8] = tx.tx.data.as_slice();
            match zbx_evm::precompiles::call_precompile(&callee, calldata, forwarded) {
                Ok((_output, used)) => {
                    debug!(
                        target: "executor",
                        precompile = ?callee,
                        gas_used = used,
                        "precompile call succeeded (output discarded at top level — \
                         Ethereum semantics, callers should use a wrapper contract \
                         to capture return data)"
                    );
                    (ExitStatus::Succeeded, used, Vec::<Log>::new())
                }
                Err(e) => {
                    debug!(
                        target: "executor",
                        precompile = ?callee,
                        error = %e,
                        forwarded_gas_burnt = forwarded,
                        "precompile call failed; consuming all forwarded gas \
                         per EVM convention"
                    );
                    (ExitStatus::Failed(e), forwarded, Vec::<Log>::new())
                }
            }
        } else if is_pure_value_transfer || code_to_run.is_empty()
        {
            (ExitStatus::Succeeded, 0u64, Vec::<Log>::new())
        } else if callee_vm == VmKind::Zvm {
            // ZVM dispatch path. Per-tx EIP-1153 scratchpad is local
            // and explicitly cleared at end-of-tx below.
            let mut transient: TransientScratchpad = std::collections::HashMap::new();
            let env = ZvmBlockEnv::from_header(header);

            // Snapshot StateView log tail so only this tx's ZVM LOG*
            // emissions are drained back into the receipt.
            let pre_log_len = view.diffs.logs.len();

            let mut evm_value = [0u8; 32];
            tx.tx.value.to_big_endian(&mut evm_value);
            let value_lo: [u8; 16] = evm_value[16..].try_into().unwrap_or([0u8; 16]);
            let value_for_zvm = u128::from_be_bytes(value_lo);

            let ctx = ZvmContext {
                bytecode:        code_to_run,
                calldata:        if tx.tx.to.is_some() { tx.tx.data.clone() } else { Vec::new() },
                caller:          tx.from.0,
                contract:        callee.0,
                value:           value_for_zvm,
                gas_limit:       gas_limit.saturating_sub(intrinsic),
                block_number:    header.number,
                block_timestamp: header.timestamp,
                base_fee:        base_fee as u128,
                blob_base_fee:   1,
                chain_id:        tx.tx.chain_id,
                is_static:       false,
                aa_sender:       None,
                zbx_price_usd:   0,
                origin:          tx.from.0,
            };

            // Task #8 (EIP-6780): drain the host's pending-destruct
            // queue after the run. Full account deletion is applied
            // ONLY for entries whose `contract` was created in this
            // tx (`was_created_this_tx`) AND only when execution
            // succeeded — a reverted tx leaves no trace, matching
            // Cancun semantics.
            let (result, pending_destructs, created_this_tx) = {
                let mut host = ProductionZvmHost::new(view, &env, &mut transient);
                // Task #8 (EIP-6780): top-level CREATE txs bypass the
                // interpreter's `do_create` (which marks created
                // addresses for nested CREATE/CREATE2). Mark the new
                // contract address here so an init-code SELFDESTRUCT
                // is recognised as same-tx and triggers full deletion.
                if let Some(new_addr) = contract_address {
                    use zbx_zvm::host::ZvmHost;
                    host.mark_created_this_tx(&new_addr.0);
                }
                let mut interp = ZvmInterpreter::new(&ctx, &mut host);
                let r = interp.run();
                let pd = host.take_pending_destructs();
                let ct = std::mem::take(&mut host.created_this_tx);
                (r, pd, ct)
            };

            // End-of-tx EIP-1153 wipe (Cancun semantics).
            zbx_state::host_zvm::clear_transient(&mut transient);

            // Apply EIP-6780 deletions BEFORE we map the status, so
            // the success-only gate is a single conditional on the
            // raw ZvmExecutionStatus.
            if matches!(result.status, ZvmExecutionStatus::Success) {
                for (contract, _beneficiary) in &pending_destructs {
                    if created_this_tx.contains(contract) {
                        view.selfdestruct(Address(*contract));
                        debug!(
                            target: "executor",
                            ?contract,
                            "EIP-6780 SELFDESTRUCT applied (created in same tx — full delete)"
                        );
                    } else {
                        debug!(
                            target: "executor",
                            ?contract,
                            "EIP-6780 SELFDESTRUCT — pre-existing contract, balance swept only \
                             (deletion suppressed per Cancun)"
                        );
                    }
                }
            }

            let (status, used) = match result.status {
                ZvmExecutionStatus::Success => (ExitStatus::Succeeded, result.gas_used),
                ZvmExecutionStatus::Revert  => (ExitStatus::Reverted, result.gas_used),
                ZvmExecutionStatus::OutOfGas => (
                    ExitStatus::Failed(zbx_evm::EvmError::OutOfGas),
                    result.gas_used,
                ),
                ZvmExecutionStatus::InvalidOpcode(op) => (
                    ExitStatus::Failed(zbx_evm::EvmError::InvalidOpcode(op)),
                    result.gas_used,
                ),
                ZvmExecutionStatus::StackOverflow => (
                    ExitStatus::Failed(zbx_evm::EvmError::StackOverflow),
                    result.gas_used,
                ),
                ZvmExecutionStatus::ZvmError(_) => (
                    ExitStatus::Failed(zbx_evm::EvmError::OutOfGas),
                    result.gas_used,
                ),
            };

            // Drain only this frame's logs from StateView into the
            // receipt path. `result.logs` is the ZVM-internal copy of
            // the same events (already forwarded via host.emit_log).
            let _ = result.logs;
            let drained: Vec<Log> = view.diffs.logs.drain(pre_log_len..).collect();
            (status, used, drained)
        } else {
            let mut evm_value = [0u8; 32];
            tx.tx.value.to_big_endian(&mut evm_value);
            let ctx = EVMContext {
                caller: tx.from,
                callee,
                value: evm_value,
                calldata: if tx.tx.to.is_some() {
                    tx.tx.data.clone()
                } else {
                    Vec::new()
                },
                gas_limit: gas_limit.saturating_sub(intrinsic),
                is_static: false,
                block_number: header.number,
                timestamp: header.timestamp,
                coinbase: header.coinbase,
                base_fee,
                gas_price: tx.effective_gas_price(base_fee),
                tx_origin: tx.from,
                chain_id: tx.tx.chain_id,
                randao_mix: header.mix_hash.into(),
            };
            let mut host = MockHost::new();
            let mut interp = EVMInterpreter::new(ctx, code_to_run, &mut host);
            let (status, used) = interp.run();
            // EXEC-03 fix (2026-05-16): the previous code discarded all EVM logs
            // by returning Vec::<Log>::new(). The interpreter accumulates LOG*
            // emissions (including from sub-calls, after the EVM-01 fix) in its
            // internal `logs` field, exposed via `take_logs()`. Convert each
            // `EvmLog` to the canonical `Log` receipt type; fields not available
            // inside execute_tx (log_index, transaction_hash, transaction_index)
            // use zero placeholders — the receipt builder at the call site holds
            // the transaction hash and index and may update these on re-serialisation.
            // block_number is filled from `header.number` which IS available here.
            let evm_logs: Vec<Log> = interp.take_logs().into_iter().map(|el: EvmLog| {
                Log {
                    address:           el.address,
                    topics:            el.topics.into_iter().map(|t| H256(t)).collect(),
                    data:              el.data,
                    block_number:      header.number,
                    log_index:         0,
                    transaction_hash:  H256::zero(),
                    transaction_index: 0,
                }
            }).collect();
            (status, used, evm_logs)
        };

        let gas_used = intrinsic
            .saturating_add(evm_gas_used)
            .min(gas_limit);
        let success = matches!(exit_status, ExitStatus::Succeeded);

        // 5. Settle the EIP-1559 fee. We deducted `max_fee_per_gas * gas_limit`
        //    upfront; the actual fee owed is `gas_used * effective_price`. The
        //    refund must return BOTH the unused-gas portion AND the
        //    (max_fee - effective_price) overcharge on every used gas unit —
        //    otherwise senders overpay during base-fee dips. Architect review
        //    flagged the previous `unused_gas * effective_price` form.
        let fee_owed = (gas_used as u128) * (effective_price as u128);
        let gas_refund = max_cost.saturating_sub(fee_owed);
        let mut sender2 = view.get_account(&tx.from);
        let bal2 = sender2.balance_u128();
        sender2.set_balance_u128(bal2.saturating_add(gas_refund));
        view.set_account(tx.from, sender2);

        // 6. Apply value transfer ONLY if execution succeeded — Ethereum
        //    semantics require a revert to roll back ETH movement.
        if success {
            if let Some(to) = tx.tx.to {
                let mut to_acct = view.get_account(&to);
                let to_bal = to_acct.balance_u128();
                to_acct.set_balance_u128(to_bal.saturating_add(value_u128));
                view.set_account(to, to_acct);
            } else if let Some(new_addr) = contract_address {
                // CREATE: fund the new contract (if value>0) and persist
                // the deploy-time VM flag so subsequent CALLs route to
                // the correct interpreter.
                //
                // Task #8 (EIP-6780): if the just-CREATEd contract
                // SELFDESTRUCT'd in its own init code (same-tx
                // CREATE+destruct), `view.is_deleted(new_addr)` is
                // already true and any `set_account` here would
                // un-tombstone it. Skip finalization in that case;
                // the value debited from the sender is effectively
                // burnt at the now-deleted address — matching Cancun
                // semantics where init-code SELFDESTRUCT to a
                // beneficiary already swept the contract's balance
                // (including this `value_u128`, which is transferred
                // sender→new_addr in the pre-execution credit step
                // and then sub-frame-swept by SELFDESTRUCT).
                if view.is_deleted(&new_addr) {
                    debug!(
                        target: "executor",
                        ?new_addr,
                        "EIP-6780: skipping CREATE finalization — \
                         init-code SELFDESTRUCT already tombstoned this addr"
                    );
                } else {
                    let mut new_acct = view.get_account(&new_addr);
                    if value_u128 > 0 {
                        let nb = new_acct.balance_u128();
                        new_acct.set_balance_u128(nb.saturating_add(value_u128));
                    }
                    if callee_vm == VmKind::Zvm {
                        new_acct.vm = VmKind::Zvm;
                    }
                    view.set_account(new_addr, new_acct);
                }
            }
        } else {
            // On revert: refund the value that was deducted upfront.
            let mut s3 = view.get_account(&tx.from);
            let b3 = s3.balance_u128();
            s3.set_balance_u128(b3.saturating_add(value_u128));
            view.set_account(tx.from, s3);
            debug!(
                hash_prefix = ?&tx.hash[..8],
                ?exit_status,
                "tx reverted"
            );
        }

        Ok(TxResult {
            gas_used,
            effective_price,
            success,
            logs,
            contract_address,
        })
    }
}

struct TxResult {
    gas_used: u64,
    effective_price: u64,
    success: bool,
    logs: Vec<Log>,
    contract_address: Option<Address>,
}

// Staking-tx dispatch from the executor: wires StateView to
// zbx_staking::BalanceAccess and exposes `execute_staking_tx` for
// the block-execution path to call when `is_staking_destination(to)`.

impl zbx_staking::BalanceAccess for StateView {
    fn get_balance(&self, addr: &Address) -> u128 {
        self.get_account(addr).balance_u128()
    }
    fn set_balance(&mut self, addr: &Address, wei: u128) {
        let mut a = self.get_account(addr);
        a.set_balance_u128(wei);
        self.set_account(*addr, a);
    }
}

/// Outcome of executing a single staking-tx (one variant of `StakingTx`).
/// Maps onto the receipt fields the producer needs: gas_used + success.
#[derive(Debug)]
pub struct StakingTxResult {
    pub gas_used: u64,
    pub success: bool,
    pub effective_price: u64,
    pub error: Option<String>,
}

/// Execute a `SignedTransaction` whose destination is the staking
/// precompile (`STAKING_PRECOMPILE_ADDR`). Mirrors `execute_tx` for the
/// pre-flight (nonce + balance + intrinsic gas + max-cost debit + nonce
/// bump) and post-flight (gas refund + value settlement) phases, but
/// dispatches the body through `zbx_staking::dispatch_staking_tx`
/// instead of the EVM interpreter.
///
/// Caller MUST first verify `is_staking_destination(tx.tx.to.as_ref())`
/// — this function does NOT re-check the destination.
pub fn execute_staking_tx(
    tx: &SignedTransaction,
    header: &BlockHeader,
    view: &mut StateView,
    vs: &mut zbx_staking::ValidatorSet,
    db: &zbx_storage::ZbxDb,
    delta: &mut zbx_staking::StakingDelta,
    pipeline: Option<&zbx_staking::SlashingPipeline>,
) -> Result<StakingTxResult, ExecutionError> {
    let base_fee = header.base_fee_per_gas;
    let effective_price = tx.effective_gas_price(base_fee);
    let gas_limit = tx.tx.gas_limit;

    // 1. Pre-flight nonce + balance.
    let mut sender = view.get_account(&tx.from);
    if tx.tx.nonce != sender.nonce {
        return Err(ExecutionError::InvalidNonce {
            expected: sender.nonce,
            got: tx.tx.nonce,
        });
    }
    let max_cost = (tx.tx.max_fee_per_gas as u128) * (gas_limit as u128);
    let mut value_bytes = [0u8; 32];
    tx.tx.value.to_big_endian(&mut value_bytes);
    let value_u128 = u128::from_be_bytes(
        value_bytes[16..].try_into().unwrap_or([0u8; 16]),
    );
    let total_cost = max_cost.saturating_add(value_u128);
    let balance = sender.balance_u128();
    if total_cost > balance {
        return Err(ExecutionError::InsufficientBalance { balance, cost: total_cost });
    }
    let intrinsic = tx.tx.intrinsic_gas();
    if intrinsic > gas_limit {
        return Err(ExecutionError::IntrinsicGasTooLow {
            required: intrinsic,
            provided: gas_limit,
        });
    }

    // 2. Deduct max gas + value, bump nonce.
    sender.set_balance_u128(balance.saturating_sub(max_cost + value_u128));
    sender
        .increment_nonce()
        .map_err(|e| ExecutionError::State(e.to_string()))?;
    view.set_account(tx.from, sender);

    // 3. Decode + dispatch the StakingTx call.
    let (success, dispatch_gas, error) =
        match zbx_staking::decode_staking_call(tx.tx.data.as_slice()) {
            Err(e) => (false, 0u64, Some(e.to_string())),
            Ok(call) => {
                // FileAppeal needs the SlashingPipeline (registry +
                // EvidenceStore), which `dispatch_staking_tx` does NOT
                // accept. Route it to `dispatch_file_appeal_tx` here.
                // When no pipeline is wired (legacy callers / tests
                // without slashing context), the tx reverts cleanly.
                if let zbx_types::staking_tx::StakingTx::FileAppeal { evidence_id } = &call {
                    match pipeline {
                        Some(p) => match zbx_staking::dispatch_file_appeal_tx(
                            *evidence_id, tx.from, value_u128, header.number, p,
                        ) {
                            Ok(g) => (true, g, None),
                            Err(e) => (false, 0u64, Some(e.to_string())),
                        },
                        None => (false, 0u64, Some(
                            "FileAppeal tx received but executor has no SlashingPipeline wired".into())),
                    }
                } else {
                    match zbx_staking::dispatch_staking_tx(
                        &call, tx.from, value_u128, header.number, vs, db, delta, view,
                    ) {
                        Ok(g) => (true, g, None),
                        Err(e) => (false, 0u64, Some(e.to_string())),
                    }
                }
            },
        };

    let gas_used = intrinsic
        .saturating_add(dispatch_gas)
        .min(gas_limit);

    // 4. Settle the EIP-1559 fee — same algebra as execute_tx step 5.
    let fee_owed = (gas_used as u128) * (effective_price as u128);
    let gas_refund = max_cost.saturating_sub(fee_owed);
    let mut sender2 = view.get_account(&tx.from);
    let bal2 = sender2.balance_u128();
    sender2.set_balance_u128(bal2.saturating_add(gas_refund));
    view.set_account(tx.from, sender2);

    // 5. Value settlement.
    //
    // On SUCCESS: deposit variants (Register/Delegate) carried `value > 0`
    // — credit it to STAKING_PRECOMPILE_ADDR (the global escrow). Non-deposit
    // variants asserted `value == 0` upstream so this is a no-op for them.
    //
    // On FAILURE: refund the value to the sender (mirrors execute_tx step
    // 6's revert branch — Ethereum semantics require ETH movement to roll
    // back on revert).
    if success {
        if value_u128 > 0 {
            let to = zbx_types::staking_tx::STAKING_PRECOMPILE_ADDR;
            let mut t = view.get_account(&to);
            let tb = t.balance_u128();
            t.set_balance_u128(tb.saturating_add(value_u128));
            view.set_account(to, t);
        }
    } else {
        let mut s3 = view.get_account(&tx.from);
        let b3 = s3.balance_u128();
        s3.set_balance_u128(b3.saturating_add(value_u128));
        view.set_account(tx.from, s3);
        debug!(hash_prefix = ?&tx.hash[..8], ?error, "staking tx reverted");
    }

    Ok(StakingTxResult { gas_used, success, effective_price, error })
}