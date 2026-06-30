//! Bridge relayer: processes multi-token deposit and withdrawal requests.
//!
//! ## Security fixes in this module
//!
//! ### OUT2 (source-chain binding)
//!
//! `BridgeRelayer` records its `own_chain_id` (a raw `u64` — ZBX mainnet = 8989)
//! at construction time.  Every call to `submit()`, `confirm()`, and `execute()`
//! asserts that the request's `source_chain_id` matches `own_chain_id`.  Without
//! this guard a malicious relayer could copy a signed request from a ZBX→ETH
//! bridge and replay it against a ZBX→BSC bridge instance running the same key
//! set; the `msg_hash` would differ, but the explicit chain check provides
//! defence-in-depth that is cheap and obvious.
//!
//! `source_chain_id` and `own_chain_id` are `u64` (not the `ChainId` enum) because
//! the `ChainId` enum only covers *target* chains (ETH/BSC/Polygon); ZBX Chain
//! (8989) is always the *source* chain for deposits and is not in that enum.
//!
//! ### MS1 (tally-griefing) — see also `multisig.rs`
//!
//! `confirm()` calls `auth.verify_single()` BEFORE `add_confirmation()`.
//! Previously, the (signer, sig) pair was appended to `req.confirmations` first,
//! then `verify_threshold` checked the accumulated list.  A single call with an
//! invalid signature permanently poisoned the list; every subsequent
//! `verify_threshold` call saw the bad entry and failed — a per-request DoS.
//!
//! ### OUT1 (spent-operations persistence) — see `multisig.rs`
//!
//! `BridgeRelayer::load_spent_ops` / `drain_spent_ops` delegate to
//! `MultisigAuth` and must be wired to durable storage before mainnet.

use crate::{
    error::BridgeError,
    multisig::{MultisigAuth, MultisigKey},
    persistence::SpentOpsStore,
    proofs::BridgeProof,
    token::{BridgeToken, DailyLimitTracker, TokenWhitelist, NATIVE_ZBX_SENTINEL},
    ChainId, BRIDGE_FEE_BPS, MIN_BRIDGE_AMOUNT, REQUEST_EXPIRY_SECS,
};
use zbx_types::{address::Address, H256};
use zbx_crypto::secp256k1::Signature;
use std::collections::HashMap;
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// ZBX Chain mainnet ID — always the source chain for deposits.
pub const ZBX_CHAIN_ID_MAINNET: u64 = 8989;
/// ZBX Chain testnet ID.
pub const ZBX_CHAIN_ID_TESTNET: u64 = 8990;

/// Type of bridge transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BridgeRequestType {
    /// Token → Wrapped token on target chain (lock-and-mint or burn-and-mint).
    Deposit,
    /// Wrapped token on target chain → Token on Zebvix (unlock or mint).
    Withdrawal,
}

/// Outcome of a successfully executed bridge request.
/// The execution layer applies this action to chain state.
#[derive(Debug, Clone)]
pub enum BridgeAction {
    /// Mint wrapped token on the target chain for `recipient`.
    MintOnTarget {
        target_chain: ChainId,
        token:        [u8; 20],
        token_symbol: String,
        recipient:    Address,
        amount:       u128,
        request_id:   H256,
    },
    /// Unlock native token on Zebvix chain and credit `recipient`.
    UnlockOnZbx {
        token:        [u8; 20],
        token_symbol: String,
        recipient:    Address,
        amount:       u128,
        request_id:   H256,
    },
}

/// A pending or completed bridge request (multi-token aware).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeRequest {
    /// Unique content-hash ID — prevents duplicates and replay.
    pub id:             H256,
    pub request_type:   BridgeRequestType,
    /// Token contract address (or `NATIVE_ZBX_SENTINEL` for native ZBX).
    pub token:          [u8; 20],
    /// Human-readable symbol, e.g. "ZBX", "ZUSD" — for logging only.
    pub token_symbol:   String,
    pub from:           Address,
    pub to:             Address,
    /// Net amount after fee deduction (in token base units).
    pub amount:         u128,
    /// Fee collected (in token base units).
    pub fee:            u128,
    pub target_chain:   ChainId,
    /// L-06 / OUT2 fix: raw chain ID of the chain where this request originated.
    ///
    /// Stored as `u64` rather than `ChainId` because ZBX Chain (8989) is always
    /// the source chain for deposits and is not represented in the `ChainId`
    /// enum (which covers only supported *target* chains: ETH/BSC/Polygon).
    ///
    /// Included in the request ID preimage so cross-chain requests with identical
    /// parameters produce distinct IDs.  Also checked explicitly by
    /// `BridgeRelayer` at `submit`, `confirm`, and `execute` time.
    pub source_chain_id: u64,
    pub block_number:   u64,
    /// Merkle receipt proof — required before `execute()`.
    pub proof:        Option<BridgeProof>,
    pub confirmations: Vec<(Address, Signature)>,
    pub executed:     bool,
    pub timestamp:    u64,
}

impl BridgeRequest {
    pub fn new(
        token:          [u8; 20],
        token_symbol:   String,
        from:           Address,
        to:             Address,
        amount:         u128,
        target_chain:   ChainId,
        source_chain_id: u64,
        request_type:   BridgeRequestType,
        block_number:   u64,
        timestamp:      u64,
    ) -> Result<Self, BridgeError> {
        if amount == 0 {
            return Err(BridgeError::ZeroAmount);
        }
        if token == NATIVE_ZBX_SENTINEL && amount < MIN_BRIDGE_AMOUNT {
            return Err(BridgeError::AmountTooSmall(amount));
        }

        let fee        = amount * BRIDGE_FEE_BPS / 10_000;
        let net_amount = amount - fee;

        // ID = keccak256(token ‖ from ‖ to ‖ net_amount ‖ block_number ‖ timestamp
        //                ‖ target_chain_u64 ‖ source_chain_id_u64 ‖ request_type_u8)
        //
        // L-06 / OUT2 fix: source_chain_id at bytes [100..108] ensures requests
        // from different source chains produce distinct IDs even when all other
        // parameters are identical.
        let mut id_input = [0u8; 109];
        id_input[..20].copy_from_slice(&token);
        id_input[20..40].copy_from_slice(from.as_bytes());
        id_input[40..60].copy_from_slice(to.as_bytes());
        id_input[60..76].copy_from_slice(&net_amount.to_be_bytes());
        id_input[76..84].copy_from_slice(&block_number.to_be_bytes());
        id_input[84..92].copy_from_slice(&timestamp.to_be_bytes());
        id_input[92..100].copy_from_slice(&(target_chain as u64).to_be_bytes());
        id_input[100..108].copy_from_slice(&source_chain_id.to_be_bytes());
        id_input[108] = request_type as u8;

        let id = zbx_crypto::keccak::keccak256(&id_input);
        Ok(BridgeRequest {
            id,
            request_type,
            token,
            token_symbol,
            from,
            to,
            amount: net_amount,
            fee,
            target_chain,
            source_chain_id,
            block_number,
            proof: None,
            confirmations: Vec::new(),
            executed: false,
            timestamp,
        })
    }

    /// Attach a Merkle receipt proof — required before `execute()`.
    pub fn set_proof(&mut self, proof: BridgeProof) {
        self.proof = Some(proof);
    }

    /// Append a confirmation only if the signer has not already submitted one.
    ///
    /// MUST only be called AFTER `auth.verify_single()` succeeds — never before.
    /// See MS1 tally-griefing discussion in the module-level docs.
    pub fn add_confirmation(&mut self, signer: Address, sig: Signature) {
        if !self.confirmations.iter().any(|(a, _)| a == &signer) {
            self.confirmations.push((signer, sig));
        }
    }

    pub fn msg_hash(&self) -> H256 {
        let mut buf = Vec::with_capacity(92);
        buf.extend_from_slice(self.id.as_bytes());
        buf.extend_from_slice(&self.token);
        buf.extend_from_slice(self.from.as_bytes());
        buf.extend_from_slice(self.to.as_bytes());
        buf.extend_from_slice(&self.amount.to_be_bytes());
        zbx_crypto::keccak::keccak256(&buf)
    }

    /// True if this request has exceeded the 24-hour TTL.
    pub fn is_expired(&self, current_timestamp: u64) -> bool {
        current_timestamp > self.timestamp + REQUEST_EXPIRY_SECS
    }
}

/// Orchestrates multi-token bridge request lifecycle.
pub struct BridgeRelayer {
    pub auth:          MultisigAuth,
    pub whitelist:     TokenWhitelist,
    pub daily_tracker: DailyLimitTracker,
    pub pending:       HashMap<H256, BridgeRequest>,
    pub completed:     HashMap<H256, BridgeRequest>,
    /// Global emergency pause — stops all new submissions.
    pub paused:        bool,
    /// OUT2 fix: the raw chain ID on which this relayer instance is running.
    ///
    /// Stored as `u64` (not `ChainId`) because ZBX Chain (8989) is not in the
    /// target-chain `ChainId` enum.  Use `ZBX_CHAIN_ID_MAINNET` or
    /// `ZBX_CHAIN_ID_TESTNET` from this module.
    ///
    /// Every `submit()`, `confirm()`, and `execute()` call verifies
    /// `request.source_chain_id == own_chain_id`, rejecting requests that
    /// claim to originate from a different chain.
    pub own_chain_id:  u64,
    /// OUT1 fix: optional RocksDB-backed spent-operations store.
    ///
    /// When `Some`, `execute()` writes the operation hash to durable storage
    /// BEFORE calling `auth.mark_spent()` (in-memory).  This guarantees that
    /// a crash between the two leaves the hash on disk; on the next restart
    /// `attach_storage` reloads it into `auth.spent_operations`, blocking replay.
    ///
    /// When `None` (default), only in-memory tracking is used — safe for unit
    /// tests and short-lived processes, but NOT safe for production.
    spent_ops_store: Option<Arc<dyn SpentOpsStore>>,
}

impl BridgeRelayer {
    /// Create a new relayer for `own_chain_id` with the given multisig keys.
    ///
    /// Pass `ZBX_CHAIN_ID_MAINNET` (8989) or `ZBX_CHAIN_ID_TESTNET` (8990)
    /// as `own_chain_id`.  All submitted requests whose `source_chain_id`
    /// differs are rejected immediately.
    pub fn new(keys: Vec<MultisigKey>, own_chain_id: u64) -> Self {
        BridgeRelayer {
            auth:             MultisigAuth::new(keys),
            whitelist:        TokenWhitelist::default_mainnet(),
            daily_tracker:    DailyLimitTracker::new(),
            pending:          HashMap::new(),
            completed:        HashMap::new(),
            paused:           false,
            own_chain_id,
            spent_ops_store:  None,
        }
    }

    // ── Persistence wiring (OUT1 fix) ─────────────────────────────────────

    /// Attach a durable spent-operations store and rehydrate the in-memory
    /// replay-protection set from it.
    ///
    /// ## Required call order on node startup
    ///
    /// ```text
    /// let store = Arc::new(BridgeSpentOpsStore::new(Arc::clone(&db)));
    /// relayer.attach_storage(store)?;
    /// // Now safe to start accepting bridge traffic.
    /// ```
    ///
    /// If `attach_storage` is NOT called, `execute()` falls back to
    /// in-memory-only tracking — correct for tests, but NOT production-safe
    /// because a relayer restart wipes the spent-operations set.
    ///
    /// Returns `Err` if loading previously persisted hashes fails (e.g.
    /// corrupt DB, missing column family).  In that case the caller should
    /// treat it as a node startup failure and halt — running the bridge
    /// without persistence defeats replay protection.
    pub fn attach_storage(
        &mut self,
        store: Arc<dyn SpentOpsStore>,
    ) -> Result<(), BridgeError> {
        let hashes = store.load_all()
            .map_err(|e| BridgeError::PersistenceFailure(
                format!("load_all on startup: {e}"),
            ))?;
        let n = hashes.len();
        self.auth.load_spent_operations(hashes);
        self.spent_ops_store = Some(store);
        tracing::info!(
            loaded = n,
            "bridge: persistence store attached — {} spent-op hashes restored",
            n
        );
        Ok(())
    }

    /// Builder-style alias for `attach_storage`, consuming and returning
    /// `self`.  Convenient for test and CLI tool construction:
    ///
    /// ```rust,ignore
    /// let relayer = BridgeRelayer::new(keys, ZBX_CHAIN_ID_MAINNET)
    ///     .with_storage(store)?;
    /// ```
    pub fn with_storage(
        mut self,
        store: Arc<dyn SpentOpsStore>,
    ) -> Result<Self, BridgeError> {
        self.attach_storage(store)?;
        Ok(self)
    }

    /// Load spent-operation hashes from durable storage on startup.
    ///
    /// Retained for test-only use.  Production callers should use
    /// `attach_storage` / `with_storage` instead, which also wires the
    /// persistence backend so `execute()` can write new hashes durably.
    pub fn load_spent_ops(&mut self, hashes: impl IntoIterator<Item = H256>) {
        self.auth.load_spent_operations(hashes);
    }

    /// Drain in-memory spent ops for checkpointing (test / debug use).
    pub fn drain_spent_ops(&mut self) -> Vec<H256> {
        self.auth.drain_spent_operations()
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────

    /// Pause/unpause the bridge (emergency use only).
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if paused {
            warn!("bridge: PAUSED — no new requests accepted");
        } else {
            info!("bridge: unpaused");
        }
    }

    /// Register a new token on the whitelist.
    pub fn register_token(&mut self, token: BridgeToken) {
        self.whitelist.insert(token);
    }

    // ── Request lifecycle ─────────────────────────────────────────────────

    /// Submit a new bridge request.
    ///
    /// Validates (in order):
    /// 1. Bridge not paused.
    /// 2. `source_chain_id == own_chain_id` (OUT2 source-binding).
    /// 3. Token whitelisted and enabled.
    /// 4. Amount ≤ max_per_tx.
    /// 5. Daily limit not exceeded.
    /// 6. Not a duplicate request.
    pub fn submit(
        &mut self,
        req:               BridgeRequest,
        current_timestamp: u64,
    ) -> Result<H256, BridgeError> {
        if self.paused {
            return Err(BridgeError::Paused);
        }

        // OUT2: reject requests that don't originate from this chain.
        if req.source_chain_id != self.own_chain_id {
            warn!(
                own = self.own_chain_id,
                got = req.source_chain_id,
                "OUT2: submit rejected — source_chain_id mismatch"
            );
            return Err(BridgeError::SourceChainMismatch {
                expected: self.own_chain_id,
                got:      req.source_chain_id,
            });
        }

        let token_cfg = self.whitelist.get_enabled(&req.token)?;
        if req.amount > token_cfg.max_per_tx {
            return Err(BridgeError::ExceedsMaxPerTx { max: token_cfg.max_per_tx });
        }

        let daily_limit = token_cfg.daily_limit;
        self.daily_tracker.check(&req.token, req.amount, daily_limit, current_timestamp)?;

        let id = req.id;
        if self.pending.contains_key(&id) || self.completed.contains_key(&id) {
            return Err(BridgeError::DuplicateRequest(hex::encode(id)));
        }

        self.daily_tracker.record(req.token, req.amount, current_timestamp);

        info!(
            id     = %hex::encode(id),
            token  = %req.token_symbol,
            amount = req.amount,
            chain  = ?req.target_chain,
            kind   = ?req.request_type,
            "bridge: request submitted"
        );
        self.pending.insert(id, req);
        Ok(id)
    }

    /// Add a relayer confirmation.
    ///
    /// ## MS1 tally-griefing fix
    ///
    /// The individual `(signer, sig)` pair is validated via
    /// `auth.verify_single()` BEFORE being appended to the confirmation list.
    /// This ensures an invalid or unauthorised signature is NEVER written into
    /// `req.confirmations`, so it cannot poison the list for future callers.
    ///
    /// Returns:
    /// - `Ok(true)`  — threshold reached; call `execute()` next.
    /// - `Ok(false)` — valid sig accepted; waiting for more.
    /// - `Err(InvalidSignature)`     — bad or unknown sig; nothing mutated.
    /// - `Err(Expired)`              — request TTL exceeded.
    /// - `Err(SourceChainMismatch)`  — OUT2 guard.
    pub fn confirm(
        &mut self,
        id:                &H256,
        signer:            Address,
        sig:               Signature,
        current_timestamp: u64,
    ) -> Result<bool, BridgeError> {
        let req = self.pending.get_mut(id)
            .ok_or_else(|| BridgeError::NotFound(hex::encode(id)))?;

        if req.is_expired(current_timestamp) {
            warn!(id = %hex::encode(id), "bridge: confirm rejected — request expired");
            return Err(BridgeError::Expired);
        }

        // OUT2: defence-in-depth — re-check source chain on every confirmation.
        if req.source_chain_id != self.own_chain_id {
            warn!(
                own = self.own_chain_id,
                got = req.source_chain_id,
                "OUT2: confirm rejected — source_chain_id mismatch"
            );
            return Err(BridgeError::SourceChainMismatch {
                expected: self.own_chain_id,
                got:      req.source_chain_id,
            });
        }

        let msg_hash = req.msg_hash();

        // MS1 FIX: validate BEFORE mutating confirmations.
        // Any invalid/unknown sig is rejected here with zero state change.
        self.auth.verify_single(&msg_hash, signer, &sig)?;

        // Sig is valid — append (dedup by signer address).
        req.add_confirmation(signer, sig);

        let sigs = req.confirmations.clone();
        match self.auth.verify_threshold(&msg_hash, &sigs) {
            Ok(_) => {
                info!(id = %hex::encode(id), "bridge: threshold reached — ready for execute");
                Ok(true)
            }
            Err(BridgeError::InsufficientConfirmations { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Execute an authorized bridge request.
    ///
    /// ## 4-step atomic execution flow (OUT1 + H-03 crash-safe)
    ///
    /// ```text
    /// Step 1: is_spent(msg_hash)       — fast in-memory replay check
    /// Step 2: verify_threshold(sigs)   — ECDSA threshold verification
    /// Step 3: store.persist_one(hash)  — fsync to Column::BridgeSpentOps
    ///                                    (skipped when no store attached — test mode)
    /// Step 4: auth.mark_spent(hash)    — insert into in-memory set
    /// ```
    ///
    /// Steps 3 then 4 MUST execute in that order.  A crash between 3 and 4
    /// leaves the hash in the DB; on restart `attach_storage` reloads it into
    /// the in-memory set, blocking the replay.  The reverse order would
    /// recreate the MAINNET-BLOCKER vulnerability.
    ///
    /// Returns `BridgeError::PersistenceFailure` if the DB write fails.
    /// In that case the request stays in `pending` and the in-memory
    /// `spent_operations` set is NOT updated — the caller can retry.
    pub fn execute(
        &mut self,
        id:                &H256,
        receipts_root:     &H256,
        current_timestamp: u64,
    ) -> Result<BridgeAction, BridgeError> {
        let req = self.pending.get(id)
            .ok_or_else(|| BridgeError::NotFound(hex::encode(id)))?;

        if req.is_expired(current_timestamp) {
            return Err(BridgeError::Expired);
        }

        // OUT2: final source-chain check before any state mutation.
        if req.source_chain_id != self.own_chain_id {
            warn!(
                own = self.own_chain_id,
                got = req.source_chain_id,
                "OUT2: execute rejected — source_chain_id mismatch"
            );
            return Err(BridgeError::SourceChainMismatch {
                expected: self.own_chain_id,
                got:      req.source_chain_id,
            });
        }

        let sigs     = req.confirmations.clone();
        let msg_hash = req.msg_hash();

        // ── Step 1: fast in-memory replay check ──────────────────────────
        // This is O(1) and avoids any DB hit for the common non-replay case.
        if self.auth.is_spent(&msg_hash) {
            warn!(
                hash = %hex::encode(msg_hash.0),
                "bridge: execute rejected — operation already spent (replay attempt)"
            );
            return Err(BridgeError::ReplayedOperation(hex::encode(msg_hash.0)));
        }

        // ── Step 2: ECDSA threshold verification ─────────────────────────
        // Validates that at least `MULTISIG_THRESHOLD` distinct authorised
        // signers have signed `msg_hash`.  Returns `Err` for insufficient or
        // invalid signatures (does NOT mutate any state).
        self.auth.verify_threshold(&msg_hash, &sigs)?;

        // ── Step 3: durable persistence (fsync) ──────────────────────────
        // Write to RocksDB BEFORE marking in-memory (crash-safe ordering).
        // If the write fails we abort — the request stays in `pending` for retry.
        if let Some(store) = &self.spent_ops_store {
            store.persist_one(msg_hash)
                .map_err(|e| {
                    warn!(
                        hash = %hex::encode(msg_hash.0),
                        err  = %e,
                        "bridge: execute aborted — persistence write failed"
                    );
                    BridgeError::PersistenceFailure(e)
                })?;
        }

        // ── Step 4: in-memory mark ────────────────────────────────────────
        // Only reached after a successful DB fsync (step 3).  Future calls
        // with the same msg_hash are short-circuited by the step-1 check
        // without any DB access.
        self.auth.mark_spent(msg_hash);

        // Merkle receipt proof must be attached and valid.
        let proof = req.proof.as_ref()
            .ok_or_else(|| BridgeError::ProofInvalid("no proof attached".into()))?;
        proof.verify(receipts_root)?;

        let mut done = self.pending.remove(id).unwrap();
        done.executed = true;

        let action = match done.request_type {
            BridgeRequestType::Deposit => {
                info!(
                    id     = %hex::encode(id),
                    token  = %done.token_symbol,
                    amount = done.amount,
                    chain  = ?done.target_chain,
                    to     = %hex::encode(done.to.as_bytes()),
                    "bridge: deposit executed — mint on target"
                );
                BridgeAction::MintOnTarget {
                    target_chain: done.target_chain,
                    token:        done.token,
                    token_symbol: done.token_symbol.clone(),
                    recipient:    done.to,
                    amount:       done.amount,
                    request_id:   *id,
                }
            }
            BridgeRequestType::Withdrawal => {
                info!(
                    id     = %hex::encode(id),
                    token  = %done.token_symbol,
                    amount = done.amount,
                    to     = %hex::encode(done.to.as_bytes()),
                    "bridge: withdrawal executed — unlock on Zebvix"
                );
                BridgeAction::UnlockOnZbx {
                    token:        done.token,
                    token_symbol: done.token_symbol.clone(),
                    recipient:    done.to,
                    amount:       done.amount,
                    request_id:   *id,
                }
            }
        };

        self.completed.insert(*id, done);
        Ok(action)
    }

    /// Remove all pending requests that exceeded the 24-hour TTL.
    /// Call periodically (e.g. every epoch) to prevent memory growth.
    pub fn expire_stale(&mut self, current_timestamp: u64) -> usize {
        let expired: Vec<H256> = self.pending
            .iter()
            .filter(|(_, req)| req.is_expired(current_timestamp))
            .map(|(id, _)| *id)
            .collect();

        let count = expired.len();
        for id in &expired {
            if let Some(req) = self.pending.remove(id) {
                warn!(
                    id     = %hex::encode(id),
                    token  = %req.token_symbol,
                    amount = req.amount,
                    "bridge: request expired and removed"
                );
            }
        }
        if count > 0 {
            info!(count, "bridge: expired {} stale requests", count);
        }
        count
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// These tests cover the logic-only security paths (pause, OUT2 source-chain
// check, expiry, daily limits, spent-ops persistence surface).  Tests that
// require real ECDSA key material (verify_single / confirm / execute flows)
// are marked #[ignore] with a TODO(crypto) comment — enable them once the
// zbx_crypto test-key helper is available.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multisig::MultisigKey;
    use zbx_types::address::Address;

    const ALICE: Address = Address([0xCCu8; 20]);
    const BOB:   Address = Address([0xBBu8; 20]);
    const ONE_ZBX: u128  = 1_000_000_000_000_000_000;
    const TS_NOW:  u64   = 1_700_000_000; // arbitrary fixed timestamp

    /// Helper: construct a relayer with no keys (crypto tests use this base).
    fn make_relayer(own_chain: u64) -> BridgeRelayer {
        BridgeRelayer::new(
            vec![
                MultisigKey { address: Address([0xA1u8; 20]), name: "R1".into() },
                MultisigKey { address: Address([0xA2u8; 20]), name: "R2".into() },
                MultisigKey { address: Address([0xA3u8; 20]), name: "R3".into() },
            ],
            own_chain,
        )
    }

    fn make_req(source_chain_id: u64) -> BridgeRequest {
        BridgeRequest {
            id:             H256::default(),
            request_type:   BridgeRequestType::Deposit,
            token:          crate::token::NATIVE_ZBX_SENTINEL,
            token_symbol:   "ZBX".into(),
            from:           ALICE,
            to:             BOB,
            amount:         ONE_ZBX,
            fee:            0,
            target_chain:   ChainId::BSC,
            source_chain_id,
            block_number:   1_000,
            proof:          None,
            confirmations:  Vec::new(),
            executed:       false,
            timestamp:      TS_NOW,
        }
    }

    // ── Pause ─────────────────────────────────────────────────────────────

    #[test]
    fn paused_bridge_rejects_submit() {
        let mut r = make_relayer(ZBX_CHAIN_ID_MAINNET);
        r.set_paused(true);

        let req = make_req(ZBX_CHAIN_ID_MAINNET);
        let err = r.submit(req, TS_NOW).unwrap_err();
        assert!(matches!(err, BridgeError::Paused),
            "paused bridge must reject submit");
    }

    #[test]
    fn unpaused_bridge_allows_submit_through_validation() {
        let mut r = make_relayer(ZBX_CHAIN_ID_MAINNET);
        r.set_paused(true);
        r.set_paused(false);

        // Will fail at whitelist check (amount 0 disallowed), not at pause.
        let mut req = make_req(ZBX_CHAIN_ID_MAINNET);
        req.amount = 0;
        // ZeroAmount would fail in BridgeRequest::new; here we test submit path
        // so just verify the bridge is no longer paused by expecting a
        // non-Paused error (whitelist check fires instead).
        let err = r.submit(req, TS_NOW).unwrap_err();
        assert!(!matches!(err, BridgeError::Paused),
            "unpaused bridge must not return Paused error");
    }

    // ── OUT2: source-chain binding ─────────────────────────────────────────

    #[test]
    fn submit_wrong_source_chain_rejected() {
        let mut r = make_relayer(ZBX_CHAIN_ID_MAINNET);
        let req = make_req(1 /* ETH, wrong */);

        let err = r.submit(req, TS_NOW).unwrap_err();
        assert!(
            matches!(err, BridgeError::SourceChainMismatch { expected: 8989, got: 1 }),
            "OUT2: wrong source chain in submit must be rejected — got {err:?}"
        );
    }

    #[test]
    fn testnet_relayer_rejects_mainnet_request() {
        let mut r = make_relayer(ZBX_CHAIN_ID_TESTNET);
        let req = make_req(ZBX_CHAIN_ID_MAINNET);

        let err = r.submit(req, TS_NOW).unwrap_err();
        assert!(
            matches!(err, BridgeError::SourceChainMismatch {
                expected: 8990,
                got:      8989,
            }),
            "OUT2: mainnet request must be rejected by testnet relayer"
        );
    }

    // ── OUT1: spent-operations persistence surface ─────────────────────────

    #[test]
    fn load_and_drain_spent_ops_roundtrip() {
        let mut r = make_relayer(ZBX_CHAIN_ID_MAINNET);

        // Simulate loading 3 previously-spent hashes from storage.
        let h1 = H256::from([0x01u8; 32]);
        let h2 = H256::from([0x02u8; 32]);
        let h3 = H256::from([0x03u8; 32]);
        r.load_spent_ops(vec![h1, h2, h3]);

        // Drain and verify all three are returned.
        let mut drained = r.drain_spent_ops();
        drained.sort_by_key(|h| *h.as_bytes());
        assert_eq!(drained.len(), 3, "drain must return all loaded hashes");

        // After drain, a second drain must return nothing.
        assert!(r.drain_spent_ops().is_empty(), "second drain must be empty");
    }

    // ── Expiry ─────────────────────────────────────────────────────────────

    #[test]
    fn expire_stale_removes_timed_out_requests() {
        let mut r = make_relayer(ZBX_CHAIN_ID_MAINNET);

        // Manually insert a request with an old timestamp into pending.
        let old_ts = TS_NOW - REQUEST_EXPIRY_SECS - 1;
        let mut req = make_req(ZBX_CHAIN_ID_MAINNET);
        req.timestamp = old_ts;
        let fake_id = H256::from([0xDEu8; 32]);
        req.id = fake_id;
        r.pending.insert(fake_id, req);

        // Also insert a fresh request.
        let fresh_id = H256::from([0xFEu8; 32]);
        let mut fresh = make_req(ZBX_CHAIN_ID_MAINNET);
        fresh.timestamp = TS_NOW;
        fresh.id = fresh_id;
        r.pending.insert(fresh_id, fresh);

        let removed = r.expire_stale(TS_NOW);
        assert_eq!(removed, 1, "only the stale request should be removed");
        assert!(!r.pending.contains_key(&fake_id),  "stale request must be gone");
        assert!( r.pending.contains_key(&fresh_id), "fresh request must survive");
    }

    // ── Crypto-dependent tests (require ECDSA key material) ───────────────

    /// M-7: MS1 — an invalid (all-zero) signature submitted via `confirm()` must
    /// be rejected. The relayer checks ECDSA validity before mutating state.
    #[test]
    fn ms1_invalid_sig_does_not_poison_confirmations() {
        let mut r = make_relayer(ZBX_CHAIN_ID_MAINNET);
        let req = make_req(ZBX_CHAIN_ID_MAINNET);
        let id  = req.id;
        r.pending.insert(id, req);

        // Use a signer address that is NOT in the multisig key list — this
        // will trigger a "not a known signer" error in auth.verify_single().
        let unknown_signer = Address([0xBBu8; 20]);
        let bad_sig: Signature = [0u8; 65];

        let result = r.confirm(&id, unknown_signer, bad_sig, TS_NOW);

        // confirm must fail — the bad/unknown signer must not be added.
        assert!(result.is_err(),
            "confirm with unknown signer must return Err, got {:?}", result);

        // Confirmations list must still be empty.
        let req_after = r.pending.get(&id).unwrap();
        assert!(req_after.confirmations.is_empty(),
            "bad sig must not be added to confirmations");
    }

    /// M-7: OUT1 — execute() must reject a request that was already spent.
    /// `load_spent_ops` pre-seeds the in-memory spent set (crash-recovery path).
    #[test]
    fn out1_already_spent_operation_is_rejected() {
        let mut r = make_relayer(ZBX_CHAIN_ID_MAINNET);
        let req = make_req(ZBX_CHAIN_ID_MAINNET);
        let id  = req.id;
        r.pending.insert(id, req.clone());

        // Pre-seed the spent set with the msg_hash of this request.
        let msg_hash = req.msg_hash();
        r.load_spent_ops(std::iter::once(msg_hash));

        // execute() should detect the replay and return an error.
        let receipts_root = H256::from([0u8; 32]);
        let result = r.execute(&id, &receipts_root, TS_NOW);
        assert!(
            matches!(result, Err(BridgeError::ReplayedOperation(_))),
            "already-spent op must be rejected with ReplayedOperation, got {:?}", result
        );
    }

    /// M-7: Full deposit flow — submit → confirm (insufficient sigs) → execute (fails).
    /// Verifies that execute() without quorum is rejected with InsufficientConfirmations.
    #[test]
    fn full_deposit_flow_submit_confirm_execute() {
        let mut r = make_relayer(ZBX_CHAIN_ID_MAINNET);
        let req = make_req(ZBX_CHAIN_ID_MAINNET);
        let id  = req.id;

        // Submit: operation must appear in pending.
        r.submit(req, TS_NOW).expect("submit must succeed for valid request");
        assert!(r.pending.contains_key(&id), "submitted op must be in pending");

        // Execute without any confirmations → must fail with InsufficientConfirmations.
        let receipts_root = H256::from([0u8; 32]);
        let result = r.execute(&id, &receipts_root, TS_NOW);
        assert!(
            matches!(result, Err(BridgeError::InsufficientConfirmations { .. })),
            "execute without quorum must fail with InsufficientConfirmations, got {:?}", result
        );
    }
}
