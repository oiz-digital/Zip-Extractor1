//! 3-of-5 multisig authorization for bridge operations.

use crate::{error::BridgeError, MULTISIG_THRESHOLD, MULTISIG_SIZE};
use zbx_types::{address::Address, H256};
use zbx_crypto::secp256k1::{Signature, recover_signer};
use std::collections::HashSet;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// A multisig participant key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MultisigKey {
    pub address: Address,
    pub name: String,
}

/// Multisig authorization engine.
///
/// ## H-03 fix (ZBX-H-03): operation replay protection
///
/// `MultisigAuth` maintains a `spent_operations` set of operation hashes that
/// have been executed.  `verify_and_consume` verifies threshold signatures AND
/// atomically records the hash, preventing relay or replay of the same batch.
///
/// ## MS1 (tally-griefing) mitigation
///
/// `verify_single` validates a single signer+signature BEFORE it is appended to
/// the confirmation list.  Callers (e.g. `BridgeRelayer::confirm`) MUST call
/// `verify_single` first; only on `Ok` should they persist the signature.
/// This prevents a malicious actor from poisoning the confirmation list with an
/// invalid signature and permanently blocking threshold execution.
///
/// ## MAINNET-BLOCKER (OUT1): `spent_operations` is in-memory only
///
/// `spent_operations` is a `HashSet<H256>` that lives in process memory.
/// A relayer restart resets it to empty — any previously-executed operation
/// hash is forgotten and `verify_and_consume` would accept a replay of that
/// hash on the next call.
///
/// Before mainnet, `spent_operations` MUST be backed by persistent storage
/// (e.g. RocksDB column `Column::BridgeSpentOps`).  The lifecycle must be:
///
/// ```text
/// On startup:      MultisigAuth::load_spent_operations(db.iter_spent_ops())
/// On execute():    verify_and_consume() → db.put_spent_op(hash)  [atomic]
/// On checkpoint:   MultisigAuth::drain_spent_operations() → snapshot
/// ```
///
/// The `load_spent_operations` and `drain_spent_operations` helpers below
/// provide the integration surface for the persistence layer.
pub struct MultisigAuth {
    pub keys: Vec<MultisigKey>,
    pub threshold: usize,
    /// Set of `msg_hash` values already executed.  IN-MEMORY ONLY — see
    /// MAINNET-BLOCKER (OUT1) above.  Must be persisted before mainnet.
    spent_operations: std::collections::HashSet<H256>,
}

impl MultisigAuth {
    pub fn new(keys: Vec<MultisigKey>) -> Self {
        assert!(keys.len() <= MULTISIG_SIZE, "too many signers");
        MultisigAuth {
            keys,
            threshold: MULTISIG_THRESHOLD,
            spent_operations: std::collections::HashSet::new(),
        }
    }

    // ── Persistence integration (OUT1 fix surface) ────────────────────────

    /// Bulk-load spent operation hashes from persistent storage on startup.
    ///
    /// Call this exactly once, after constructing `MultisigAuth` but before
    /// accepting any bridge traffic, so that restarted relayers cannot replay
    /// operations that were already executed in a prior session.
    ///
    /// ```rust,ignore
    /// // Typical startup wiring (persistence layer must implement the iterator):
    /// let hashes = db.iter_spent_ops().collect::<Vec<_>>();
    /// auth.load_spent_operations(hashes);
    /// ```
    pub fn load_spent_operations(&mut self, hashes: impl IntoIterator<Item = H256>) {
        for h in hashes {
            self.spent_operations.insert(h);
        }
        info!(
            count = self.spent_operations.len(),
            "multisig: spent-operations set loaded from storage"
        );
    }

    /// Drain the in-memory spent-operations set for checkpointing.
    ///
    /// After draining, the returned hashes must be written to durable storage
    /// (e.g. `db.put_spent_ops_batch(&hashes)`) before calling `clear()` on the
    /// returned Vec.  Failing to persist before clearing is a safety violation.
    pub fn drain_spent_operations(&mut self) -> Vec<H256> {
        self.spent_operations.drain().collect()
    }

    // ── Single-signature pre-validation (MS1 fix) ─────────────────────────

    /// Validate a single signer+signature pair **before** appending it to the
    /// confirmation list.
    ///
    /// ## MS1 tally-griefing mitigation
    ///
    /// `BridgeRelayer::confirm()` MUST call this before `add_confirmation()`.
    /// If any invalid or unauthorized signature were appended to the list first,
    /// it would permanently poison the confirmations vec: subsequent calls to
    /// `verify_threshold` (which checks ALL sigs) would always fail, making it
    /// impossible for legitimate relayers to reach the threshold — a permanent
    /// denial-of-service on that bridge request.
    ///
    /// Checks (all must pass — first failure returns an error, nothing is mutated):
    /// 1. `signer` is in the authorized relayer set.
    /// 2. ECDSA recovery of `sig` over `msg_hash` yields exactly `signer`.
    pub fn verify_single(
        &self,
        msg_hash: &H256,
        signer: Address,
        sig: &Signature,
    ) -> Result<(), BridgeError> {
        if !self.keys.iter().any(|k| k.address == signer) {
            warn!(addr = ?signer, "MS1: rejecting confirmation from unknown relayer");
            return Err(BridgeError::InvalidSignature(
                format!("unknown relayer {}", hex::encode(signer.as_bytes()))
            ));
        }

        match recover_signer(msg_hash, sig) {
            Ok(recovered) if recovered == signer => Ok(()),
            Ok(other) => {
                warn!(
                    expected = ?signer,
                    recovered = ?other,
                    "MS1: signature mismatch — wrong key for claimed signer"
                );
                Err(BridgeError::InvalidSignature(format!(
                    "sig for {} recovers to {} — key mismatch",
                    hex::encode(signer.as_bytes()),
                    hex::encode(other.as_bytes()),
                )))
            }
            Err(e) => {
                warn!(addr = ?signer, error = %e, "MS1: malformed signature rejected");
                Err(BridgeError::InvalidSignature(format!(
                    "malformed sig from {}: {e}",
                    hex::encode(signer.as_bytes()),
                )))
            }
        }
    }

    /// Verify that `sigs` contains at least `threshold` valid signatures over `msg_hash`.
    ///
    /// Strict mode: the *entire* batch is rejected if any supplied signature is:
    /// - From an unknown relayer         → `InvalidSignature`
    /// - Cryptographically invalid        → `InvalidSignature`
    /// - A duplicate from the same signer → `InvalidSignature`
    ///
    /// Only returns `InsufficientConfirmations` when sigs are all valid but fewer
    /// than threshold. This lets callers distinguish "bad batch" from "wait for more".
    pub fn verify_threshold(
        &self,
        msg_hash: &H256,
        sigs: &[(Address, Signature)],
    ) -> Result<(), BridgeError> {
        let mut confirmed: HashSet<Address> = HashSet::new();

        for (addr, sig) in sigs {
            // Unknown relayer — reject whole batch immediately.
            if !self.keys.iter().any(|k| k.address == *addr) {
                warn!(addr = ?addr, "rejecting batch: signature from unknown relayer");
                return Err(BridgeError::InvalidSignature(
                    format!("unknown relayer {}", hex::encode(addr.as_bytes()))
                ));
            }

            // Duplicate signature — one relayer cannot fill multiple threshold slots.
            if confirmed.contains(addr) {
                warn!(addr = ?addr, "rejecting batch: duplicate signature");
                return Err(BridgeError::InvalidSignature(
                    format!("duplicate signature from {}", hex::encode(addr.as_bytes()))
                ));
            }

            match recover_signer(msg_hash, sig) {
                Ok(recovered) if recovered == *addr => {
                    confirmed.insert(*addr);
                }
                Ok(other) => {
                    warn!(expected = ?addr, got = ?other, "rejecting batch: signature mismatch");
                    return Err(BridgeError::InvalidSignature(
                        format!("sig from {} recovers to {}", 
                            hex::encode(addr.as_bytes()),
                            hex::encode(other.as_bytes()))
                    ));
                }
                Err(e) => {
                    warn!(addr = ?addr, error = %e, "rejecting batch: bad signature");
                    return Err(BridgeError::InvalidSignature(
                        format!("malformed signature from {}: {}", hex::encode(addr.as_bytes()), e)
                    ));
                }
            }
        }

        if confirmed.len() >= self.threshold {
            info!(confirmed = confirmed.len(), threshold = self.threshold, "multisig authorized");
            Ok(())
        } else {
            Err(BridgeError::InsufficientConfirmations {
                got: confirmed.len(),
                required: self.threshold,
            })
        }
    }

    /// Verify threshold signatures AND atomically mark the operation as spent.
    ///
    /// This is the replay-safe counterpart to `verify_threshold`.  Call this
    /// exactly once per bridge execution (not during signature accumulation).
    ///
    /// Returns `Err(BridgeError::ReplayedOperation)` if `msg_hash` has already
    /// been consumed by a prior call to this function.
    pub fn verify_and_consume(
        &mut self,
        msg_hash: &H256,
        sigs: &[(Address, Signature)],
    ) -> Result<(), BridgeError> {
        // Replay check — must happen BEFORE crypto verification to prevent
        // timing-based replay (attacker re-submits with a superset of sigs).
        if self.spent_operations.contains(msg_hash) {
            warn!(hash = ?msg_hash, "rejecting: operation already executed (replay attempt)");
            return Err(BridgeError::ReplayedOperation(
                hex::encode(msg_hash.as_bytes())
            ));
        }

        // Threshold verification (cryptographic check).
        self.verify_threshold(msg_hash, sigs)?;

        // Atomically mark as spent.
        self.spent_operations.insert(*msg_hash);
        info!(hash = ?msg_hash, "operation consumed — replay protection recorded");
        Ok(())
    }

    /// Whether an operation hash has already been executed.
    pub fn is_spent(&self, msg_hash: &H256) -> bool {
        self.spent_operations.contains(msg_hash)
    }

    /// Insert `hash` into the in-memory spent-operations set.
    ///
    /// ## Caller contract
    ///
    /// This MUST only be called AFTER the persistence layer has durably
    /// written the hash to RocksDB (via `BridgeSpentOpsStore::persist_one`).
    /// The correct call order in `BridgeRelayer::execute()` is:
    ///
    /// ```text
    /// 1. is_spent(&h)         — fast replay check (no DB hit)
    /// 2. verify_threshold(…)  — ECDSA
    /// 3. store.persist_one(h) — fsync to Column::BridgeSpentOps
    /// 4. mark_spent(h)        — THIS function
    /// ```
    ///
    /// Violating the order (calling `mark_spent` before `persist_one`)
    /// recreates the MAINNET-BLOCKER (OUT1) crash-replay vulnerability:
    /// a process crash between 4 and 3 would leave the hash in memory but
    /// not on disk; after restart neither the DB nor the in-memory set would
    /// know the hash was spent, enabling a replay.
    pub fn mark_spent(&mut self, hash: H256) {
        self.spent_operations.insert(hash);
    }
}
