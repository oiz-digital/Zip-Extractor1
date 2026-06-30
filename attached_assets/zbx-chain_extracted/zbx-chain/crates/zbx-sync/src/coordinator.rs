//! SEC-2026-05-09 Pass-11 — Fast-sync coordinator.
//!
//! End-to-end orchestration of a fresh node bootstrapping against
//! the network:
//!
//! 1. Pick the highest header the network advertises (Status from
//!    handshake) → derive a finalized **pivot** at `tip - SAFE_PIVOT`.
//! 2. Headers-first: fetch + verify the header chain `[1..=pivot]`
//!    via `SyncPeer::get_headers`. Each header's `parent_hash` must
//!    chain back to the previous one; the pivot's hash is recorded.
//! 3. Snapshot meta: `SyncPeer::get_snapshot_meta(pivot)`. Verify
//!    `meta.state_root == pivot_header.state_root`. The manifest's
//!    `chunk_roots` array is the responder's commitment to each
//!    mini-trie root; the requester verifies each chunk against
//!    the committed root.
//! 4. Snap chunks: for each `chunk_id` in `0..meta.total_chunks`,
//!    fetch `SyncPeer::get_chunk(...)` and call
//!    `snap_sync::verify_chunk(...)`.
//!
//! **Honest scope of the unit test below.** A `MockPeer` serves
//! deterministic headers + a deterministic snapshot built from a
//! fixed key-set. The coordinator drives the full pipeline against
//! that mock peer, exercising every check (header chain, manifest
//! state-root binding, chunk-root verification, malformed responses).
//! Real wire integration with `NetworkServer` (Pass-12) translates
//! these trait calls into request/response message pairs over Noise
//! XX TCP — protocol shape is exactly what `messages.rs` Pass-11
//! defines.

use crate::error::SyncError;
use crate::manifest::{
    build_inclusion_proofs, verify_chunk_inclusion, verify_manifest_signature,
};
use crate::merkle::merkle_root;
use crate::snap_sync::{StateChunk, verify_chunk, verify_global_state_root};
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{info, warn, debug};
use zbx_threshold::{BlsAggSignature, BlsPubKey, ValidatorBitmap};
use zbx_types::{address::Address, block::BlockHeader, H256};

/// SEC-2026-05-09 Pass-11 — confirmations between chain tip and
/// the snap pivot. Same constant as `pivot::SAFE_PIVOT_CONFIRMATIONS`
/// but exposed here so the coordinator can be tested in isolation.
pub const COORD_SAFE_PIVOT: u64 = 64;

/// Manifest as delivered to the coordinator. Mirrors
/// `zbx_network::messages::SnapshotMeta` (kept here as a coordinator-
/// side type so this crate does not import `zbx-network`, avoiding
/// a circular dep — `zbx-network` already depends on consensus crates
/// `zbx-sync` will eventually depend on too).
#[derive(Debug, Clone)]
pub struct SnapshotMeta {
    pub pivot_height: u64,
    pub state_root:   H256,
    pub chunk_roots:  Vec<H256>,
    /// SEC-2026-05-09 Pass-19 (Task #10) — Merkle root over
    /// `chunk_roots` (binary tree, Bitcoin-style odd-padding). Single
    /// 32-byte commitment that the BLS quorum signs; binds every
    /// chunk_root by inclusion proof.
    pub chunk_root: H256,
    /// SEC-2026-05-09 Pass-19 (Task #10) — BLS12-381 aggregate
    /// signature by `bls_signers` over the canonical manifest digest
    /// `manifest_digest(pivot_height, state_root, chunk_root, len)`.
    pub bls_quorum_sig: BlsAggSignature,
    /// SEC-2026-05-09 Pass-19 (Task #10) — bitmap of which validators
    /// in canonical committee order contributed to `bls_quorum_sig`.
    pub bls_signers: ValidatorBitmap,
}

/// Async peer abstraction. Real implementation in Pass-12 wraps the
/// `NetworkServer` request/response machinery; tests use `MockPeer`.
#[async_trait]
pub trait SyncPeer: Send + Sync {
    /// Latest block height + hash this peer claims.
    async fn tip(&self) -> Result<(u64, H256), SyncError>;

    /// Headers `[from .. from+count)`. Returned headers MUST be in
    /// ascending order. Responder may cap `count` at its limit.
    async fn get_headers(&self, from: u64, count: u32)
        -> Result<Vec<BlockHeader>, SyncError>;

    /// Snapshot manifest for the given pivot height.
    async fn get_snapshot_meta(&self, pivot_height: u64)
        -> Result<SnapshotMeta, SyncError>;

    /// Leaves of one snapshot chunk.
    async fn get_chunk(&self, pivot_height: u64, chunk_id: u64)
        -> Result<Vec<(H256, Vec<u8>)>, SyncError>;

    /// SEC-2026-05-09 Pass-19 (Task #10) — peer-misbehavior reporting
    /// hook. Called by the coordinator on every cryptographic
    /// binding failure (bad manifest signature, chunk_root /
    /// merkle_root mismatch, per-chunk inclusion proof failure,
    /// chunk hash mismatch, missing chunk). Real network impl bans
    /// the peer and disconnects; the default no-op is appropriate
    /// for in-process tests where there is no peer table to mutate.
    /// `reason` is a short stable string suitable for metrics labels.
    async fn report_misbehavior(&self, _reason: &'static str) {}
}

/// Outcome of `SyncCoordinator::run`. All chunks verified and the
/// pivot-block state-root is now known good.
#[derive(Debug, Clone)]
pub struct FastSyncOutcome {
    pub pivot_height:       u64,
    pub pivot_state_root:   H256,
    pub headers_downloaded: u64,
    pub chunks_verified:    u64,
}

pub struct SyncCoordinator<P: SyncPeer> {
    peer:     Arc<P>,
    /// Maximum headers requested per call. Matches the responder
    /// ceiling so we don't have to handle short responses specially.
    headers_per_call: u32,
    /// SEC-2026-05-09 Pass-19 (Task #10) — canonical validator set
    /// (Address + BLS pubkey, in committee order) used to verify the
    /// BLS quorum signature on the snapshot manifest. Empty disables
    /// the check entirely (only acceptable for in-process tests with
    /// no validator-set context — see `coordinator_unsigned_*` tests).
    validator_keys: Vec<(Address, BlsPubKey)>,
    /// SEC-2026-05-09 Pass-19 (Task #10) — BLS quorum threshold
    /// (typically `2f + 1`). Honoured only if `validator_keys` is
    /// non-empty.
    quorum: usize,
}

impl<P: SyncPeer> SyncCoordinator<P> {
    /// Production constructor. Panics if the validator set is empty
    /// or `quorum` is out of range — BLS manifest verification cannot
    /// be skipped in production.
    pub fn new(
        peer: Arc<P>,
        validator_keys: Vec<(Address, BlsPubKey)>,
        quorum: usize,
    ) -> Self {
        assert!(
            !validator_keys.is_empty() && quorum > 0 && quorum <= validator_keys.len(),
            "SyncCoordinator::new: non-empty validator set and 0 < quorum <= |set| required"
        );
        Self {
            peer,
            headers_per_call: 256,
            validator_keys,
            quorum,
        }
    }

    /// Test-only constructor that skips BLS manifest verification.
    /// Compiled out in non-test builds — fail-open path is
    /// unreachable from production code.
    #[cfg(test)]
    pub(crate) fn new_unchecked_for_tests(peer: Arc<P>) -> Self {
        Self {
            peer,
            headers_per_call: 256,
            validator_keys: Vec::new(),
            quorum: 0,
        }
    }

    pub fn with_batch(mut self, n: u32) -> Self {
        self.headers_per_call = n.max(1);
        self
    }

    /// VALIDATOR-SYNC FIX: Update the validator key set used for BLS manifest
    /// verification after an epoch rotation.
    ///
    /// The `SyncCoordinator` is constructed once at node startup with the
    /// genesis validator set. When an epoch boundary is crossed and the active
    /// validator set rotates, a new-joiner node that starts syncing AFTER the
    /// rotation would attempt to verify the snapshot manifest BLS quorum
    /// signature against the stale genesis keys — and fail, even for an
    /// honestly-signed manifest. Callers (node / consensus driver) should
    /// invoke this method whenever `ConsensusDriver::do_commit` fires an epoch
    /// transition so that subsequent `run()` calls use the current committee.
    ///
    /// Panics if the new set is empty or `quorum` is out of range — same
    /// invariant as `SyncCoordinator::new`.
    pub fn update_validator_keys(
        &mut self,
        validator_keys: Vec<(Address, BlsPubKey)>,
        quorum: usize,
    ) {
        assert!(
            !validator_keys.is_empty() && quorum > 0 && quorum <= validator_keys.len(),
            "update_validator_keys: non-empty validator set and 0 < quorum <= |set| required"
        );
        self.validator_keys = validator_keys;
        self.quorum = quorum;
        info!(
            validators = self.validator_keys.len(),
            quorum,
            "SyncCoordinator: validator key set updated after epoch rotation"
        );
    }

    /// SEC-2026-05-09 Pass-19 (Task #10) — late binding of the
    /// validator set. Useful when the coordinator is constructed
    /// before the committee for the pivot epoch is known. Once
    /// called with a non-empty set, every subsequent `run()`
    /// mandatorily verifies the BLS quorum signature.
    pub fn with_validator_set(
        mut self,
        validator_keys: Vec<(Address, BlsPubKey)>,
        quorum: usize,
    ) -> Self {
        // Mirror new()'s invariant so the builder cannot reopen the
        // fail-open path after a safe construction.
        assert!(
            !validator_keys.is_empty() && quorum > 0 && quorum <= validator_keys.len(),
            "with_validator_set: non-empty validator set and 0 < quorum <= |set| required"
        );
        self.validator_keys = validator_keys;
        self.quorum = quorum;
        self
    }

    /// Drive a full bootstrap. Returns when every chunk is verified
    /// or the first hard error.
    pub async fn run(&self) -> Result<FastSyncOutcome, SyncError> {
        // 1. Pick pivot.
        let (tip, _tip_hash) = self.peer.tip().await?;
        if tip < COORD_SAFE_PIVOT {
            return Err(SyncError::PivotNotFinalized(tip));
        }
        let pivot_height = tip - COORD_SAFE_PIVOT;
        info!(tip, pivot_height, "Pass-11 coordinator: bootstrapping");

        // 2. Headers-first download with chain-link verification.
        let mut prev_hash: Option<H256> = None;
        let mut next_from: u64 = 1;
        let mut total_headers: u64 = 0;
        let mut pivot_state_root: Option<H256> = None;
        while next_from <= pivot_height {
            let want = ((pivot_height - next_from + 1) as u32).min(self.headers_per_call);
            let batch = self.peer.get_headers(next_from, want).await?;
            if batch.is_empty() {
                return Err(SyncError::Interrupted(format!(
                    "peer returned 0 headers from {next_from}"
                )));
            }
            for h in &batch {
                if h.number != next_from {
                    return Err(SyncError::InvalidBlock(
                        h.number, format!("expected height {next_from}")));
                }
                if let Some(p) = prev_hash {
                    if h.parent_hash != p {
                        return Err(SyncError::InvalidBlock(
                            h.number, "parent hash break".into()));
                    }
                }
                prev_hash = Some(h.hash());
                if h.number == pivot_height {
                    pivot_state_root = Some(h.state_root);
                }
                next_from += 1;
                total_headers += 1;
            }
        }
        let pivot_state_root = pivot_state_root.ok_or_else(|| {
            SyncError::Interrupted("pivot header missing from responses".into())
        })?;
        debug!(total_headers, "Pass-11 coordinator: header chain verified");

        // 3. Manifest. The pivot header's state_root is our trust
        //    anchor; the manifest must claim the same root.
        let meta = self.peer.get_snapshot_meta(pivot_height).await?;
        if meta.pivot_height != pivot_height {
            return Err(SyncError::Interrupted(format!(
                "manifest pivot mismatch: {} vs {pivot_height}",
                meta.pivot_height)));
        }
        if meta.state_root != pivot_state_root {
            warn!(?meta.state_root, ?pivot_state_root,
                  "manifest state-root != pivot header state-root");
            return Err(SyncError::Interrupted(
                "manifest state_root != pivot header state_root".into()));
        }
        if meta.chunk_roots.is_empty() {
            return Err(SyncError::Interrupted("manifest has zero chunks".into()));
        }

        // SEC-2026-05-09 Pass-19 (Task #10) — STAGE 3a: cryptographic
        // binding of the manifest. BEFORE we trust any single field
        // in `meta`, the BLS quorum signature must verify, AND the
        // claimed `chunk_root` must equal the locally-recomputed
        // `merkle_root(chunk_roots)`. This eliminates two attacks:
        //
        //   (i)  Forged manifest: a malicious peer's manifest fails
        //        the BLS pairing check and is rejected before any
        //        chunk fetch wastes bandwidth.
        //   (ii) chunk_root rebind: a malicious peer signs a manifest
        //        for one chunk_root but serves chunks committing to
        //        a different one — the local recomputation catches
        //        the inconsistency.
        //
        // The validator-set check is skipped only when the coordinator
        // was constructed without `with_validator_set` — that path is
        // for in-process tests with no committee context. Production
        // node boots ALWAYS supply a validator set.
        let local_chunk_root = merkle_root(&meta.chunk_roots);
        if local_chunk_root != meta.chunk_root {
            self.peer.report_misbehavior("manifest_chunk_root_mismatch").await;
            return Err(SyncError::BadManifestSignature(format!(
                "manifest.chunk_root {:?} != merkle_root(chunk_roots) {:?}",
                meta.chunk_root, local_chunk_root
            )));
        }
        // BLS manifest verification is mandatory in production
        // (constructor enforces non-empty committee). The empty-set
        // branch only fires via the doc-hidden test constructor.
        if !self.validator_keys.is_empty() {
            if let Err(e) = verify_manifest_signature(
                meta.pivot_height,
                meta.state_root,
                meta.chunk_root,
                meta.chunk_roots.len() as u64,
                &meta.bls_quorum_sig,
                &meta.bls_signers,
                &self.validator_keys,
                self.quorum,
            ) {
                self.peer.report_misbehavior("bad_manifest_signature").await;
                return Err(e);
            }
            debug!("manifest BLS quorum signature verified");
        } else {
            warn!("coordinator running without validator-set check (test-only path)");
        }

        // Trust chain: payload --verify_chunk--> chunk_roots[i]
        //   --inclusion proof--> manifest.chunk_root --BLS sig-->
        //   validator set. Inclusion proofs are pre-computed from the
        //   BLS-bound chunk_roots so a tampered entry is rejected
        //   before fetch; payload is bound by verify_chunk after.
        let inclusion_proofs = build_inclusion_proofs(&meta.chunk_roots);

        // 4. Chunks. THREE-stage verification (Pass-19 adds (a0)):
        //    (a0) inclusion: chunk_roots[i] merkle-proves against
        //         manifest.chunk_root (BLS-quorum-bound). Rejects a
        //         single-chunk tamper IMMEDIATELY on receipt without
        //         waiting for the global state-root rebuild.
        //    (a)  per-chunk against `chunk_roots[i]` (responder's
        //         manifest commitment). Catches per-chunk leaf tamper.
        //    (b)  global trie rebuilt from the union of all chunks
        //         must equal the pivot header's `state_root`. Without
        //         (b), a malicious peer can lie about chunk_roots
        //         because the manifest's chunk_roots are not bound
        //         to state_root. SEC-2026-05-09 Pass-11 architect
        //         review follow-up.
        let mut verified: u64 = 0;
        let mut all_leaves: Vec<Vec<(H256, Vec<u8>)>> =
            Vec::with_capacity(meta.chunk_roots.len());
        for (idx, chunk_root) in meta.chunk_roots.iter().enumerate() {
            let chunk_id = idx as u64;
            // (a0) inclusion proof — fail-fast before bandwidth burn.
            if let Err(e) = verify_chunk_inclusion(
                chunk_id,
                *chunk_root,
                &inclusion_proofs[idx],
                meta.chunk_root,
            ) {
                self.peer.report_misbehavior("chunk_inclusion_proof_failed").await;
                return Err(e);
            }
            let leaves = match self.peer.get_chunk(pivot_height, chunk_id).await {
                Ok(l) if l.is_empty() => {
                    // SEC-2026-05-09 Pass-19 (Task #10): empty chunk
                    // never matches a non-zero `chunk_root`, but the
                    // misbehavior signal needs the explicit "missing"
                    // label (peer is starving the consumer).
                    self.peer.report_misbehavior("chunk_missing").await;
                    return Err(SyncError::ChunkRootMismatch { chunk: chunk_id });
                }
                Ok(l) => l,
                Err(e) => {
                    self.peer.report_misbehavior("chunk_fetch_error").await;
                    return Err(e);
                }
            };
            let chunk = StateChunk {
                id:         chunk_id,
                start_key:  H256::zero(),
                end_key:    H256([0xff; 32]),
                state_root: pivot_state_root,
                chunk_root: Some(*chunk_root),
            };
            if let Err(e) = verify_chunk(&chunk, &leaves) {
                self.peer.report_misbehavior("chunk_payload_tampered").await;
                return Err(e);
            }
            all_leaves.push(leaves);
            verified += 1;
        }
        // (b) Global binding: the union of all chunks MUST hash to
        //     the pivot header's state_root.
        if let Err(e) = verify_global_state_root(pivot_state_root, &all_leaves) {
            self.peer.report_misbehavior("global_state_root_mismatch").await;
            return Err(e);
        }

        info!(pivot_height, total_headers, chunks_verified = verified,
              "Pass-11 coordinator: bootstrap complete");
        Ok(FastSyncOutcome {
            pivot_height,
            pivot_state_root,
            headers_downloaded: total_headers,
            chunks_verified: verified,
        })
    }
}

// =============================================================================
// Tests — drive the coordinator end-to-end against a deterministic mock peer.
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use zbx_trie::{MutableTrie, trie::MemoryTrieDB};
    use zbx_types::{address::Address, U256};
    use std::sync::Mutex;

    /// SEC-2026-05-09 Pass-11 architect follow-up: the test peer's
    /// `pivot_state_root` is now the REAL global MPT root of every
    /// leaf in every chunk combined — not a synthetic
    /// `keccak(chunk0||chunk1)`. Without this the new
    /// `verify_global_state_root` check inside `SyncCoordinator::run`
    /// would always fire for the happy-path test.
    fn build_global_root(chunks: &[&[(H256, Vec<u8>)]]) -> H256 {
        let mut t = MutableTrie::new(MemoryTrieDB::default());
        for c in chunks {
            for (k, v) in *c {
                t.insert(k.as_bytes(), v.clone()).unwrap();
            }
        }
        t.commit().unwrap()
    }

    fn mk_header(number: u64, parent: H256, state_root: H256) -> BlockHeader {
        BlockHeader {
            parent_hash: parent,
            uncle_hash:  H256::zero(),
            coinbase:    Address::zero(),
            state_root,
            transactions_root: H256::zero(),
            receipts_root:     H256::zero(),
            logs_bloom:  [0u8; 256],
            difficulty:  U256::from(1u64),
            number,
            gas_limit:   30_000_000,
            gas_used:    0,
            timestamp:   1_700_000_000 + number,
            extra_data:  vec![],
            mix_hash:    H256::zero(),
            nonce:       0,
            base_fee_per_gas: 1,
            committee_signature: vec![],
            epoch:       number / 1000,
            epoch_seed:  None,
        }
    }

    fn key(b: u8) -> H256 { let mut k = [0u8; 32]; k[31] = b; H256(k) }

    /// Deterministic snapshot peer. Chain length = `tip + 1` (heights
    /// 0..=tip). Snapshot has 2 chunks built from disjoint key-sets;
    /// each chunk's root is computed up-front so the manifest's
    /// commitment is correct.
    struct MockPeer {
        tip: u64,
        headers: Vec<BlockHeader>,
        chunk0_leaves: Vec<(H256, Vec<u8>)>,
        chunk1_leaves: Vec<(H256, Vec<u8>)>,
        chunk0_root: H256,
        chunk1_root: H256,
        pivot_state_root: H256,
        // For mutation tests:
        tamper_chunk: Mutex<Option<u64>>,
        tamper_state_root: Mutex<bool>,
    }

    impl MockPeer {
        fn new(tip: u64) -> Self {
            // Build the snapshot first so we know the pivot's state_root.
            let chunk0_leaves: Vec<(H256, Vec<u8>)> =
                (0..16u8).map(|i| (key(i), vec![i, i ^ 0xAA])).collect();
            let chunk1_leaves: Vec<(H256, Vec<u8>)> =
                (16..32u8).map(|i| (key(i), vec![i, i ^ 0x55])).collect();

            let chunk0_root = {
                let mut t = MutableTrie::new(MemoryTrieDB::default());
                for (k, v) in &chunk0_leaves { t.insert(k.as_bytes(), v.clone()).unwrap(); }
                t.commit().unwrap()
            };
            let chunk1_root = {
                let mut t = MutableTrie::new(MemoryTrieDB::default());
                for (k, v) in &chunk1_leaves { t.insert(k.as_bytes(), v.clone()).unwrap(); }
                t.commit().unwrap()
            };
            // SEC-2026-05-09 Pass-11 architect follow-up: pivot
            // state_root is the REAL global MPT root over the union
            // of every chunk's leaves. This is what the new
            // `verify_global_state_root` step in the coordinator
            // (cryptographic binding from chunks to pivot state-root)
            // requires. The two per-chunk roots above are still used
            // for the per-chunk Merkle proof.
            let pivot_state_root = build_global_root(
                &[&chunk0_leaves, &chunk1_leaves],
            );

            // Header chain: heights 1..=tip. Pivot = tip - COORD_SAFE_PIVOT.
            let pivot = tip.saturating_sub(COORD_SAFE_PIVOT);
            let mut headers = Vec::with_capacity(tip as usize);
            let mut prev = H256::zero();
            for n in 1..=tip {
                let sr = if n == pivot { pivot_state_root } else { H256::zero() };
                let h = mk_header(n, prev, sr);
                prev = h.hash();
                headers.push(h);
            }
            Self {
                tip, headers, chunk0_leaves, chunk1_leaves,
                chunk0_root, chunk1_root, pivot_state_root,
                tamper_chunk: Mutex::new(None),
                tamper_state_root: Mutex::new(false),
            }
        }
    }

    #[async_trait]
    impl SyncPeer for MockPeer {
        async fn tip(&self) -> Result<(u64, H256), SyncError> {
            Ok((self.tip, self.headers.last().unwrap().hash()))
        }
        async fn get_headers(&self, from: u64, count: u32)
            -> Result<Vec<BlockHeader>, SyncError>
        {
            if from == 0 || from > self.tip { return Ok(vec![]); }
            let start = (from - 1) as usize;
            let end = (start + count as usize).min(self.headers.len());
            Ok(self.headers[start..end].to_vec())
        }
        async fn get_snapshot_meta(&self, pivot_height: u64)
            -> Result<SnapshotMeta, SyncError>
        {
            let state_root = if *self.tamper_state_root.lock().unwrap() {
                H256([0xDE; 32])
            } else {
                self.pivot_state_root
            };
            // SEC-2026-05-09 Pass-19 (Task #10): existing legacy
            // tests run WITHOUT a validator set on the coordinator
            // (`with_validator_set` is not called), so the BLS sig
            // and signer bitmap are empty placeholders — the
            // coordinator skips the BLS check on that code path.
            // The local `chunk_root == merkle_root(chunk_roots)`
            // recomputation is still enforced.
            let chunk_roots = vec![self.chunk0_root, self.chunk1_root];
            let chunk_root = crate::merkle::merkle_root(&chunk_roots);
            Ok(SnapshotMeta {
                pivot_height,
                state_root,
                chunk_roots,
                chunk_root,
                bls_quorum_sig: BlsAggSignature([0u8; 96]),
                bls_signers:    ValidatorBitmap::new(0),
            })
        }
        async fn get_chunk(&self, _pivot_height: u64, chunk_id: u64)
            -> Result<Vec<(H256, Vec<u8>)>, SyncError>
        {
            let mut leaves = match chunk_id {
                0 => self.chunk0_leaves.clone(),
                1 => self.chunk1_leaves.clone(),
                _ => return Err(SyncError::Interrupted(format!("bad chunk {chunk_id}"))),
            };
            if let Some(t) = *self.tamper_chunk.lock().unwrap() {
                if t == chunk_id && !leaves.is_empty() {
                    leaves[0].1 = vec![0xFF, 0xFF, 0xFF];
                }
            }
            Ok(leaves)
        }
    }

    #[tokio::test]
    async fn coordinator_full_bootstrap_ok() {
        let peer = Arc::new(MockPeer::new(200));
        let coord = SyncCoordinator::new_unchecked_for_tests(peer.clone()).with_batch(16);
        let outcome = coord.run().await.expect("bootstrap should succeed");
        assert_eq!(outcome.pivot_height, 200 - COORD_SAFE_PIVOT);
        assert_eq!(outcome.headers_downloaded, outcome.pivot_height);
        assert_eq!(outcome.chunks_verified, 2);
        assert_eq!(outcome.pivot_state_root, peer.pivot_state_root);
    }

    #[tokio::test]
    async fn coordinator_rejects_tampered_chunk() {
        let peer = Arc::new(MockPeer::new(200));
        *peer.tamper_chunk.lock().unwrap() = Some(1);
        let coord = SyncCoordinator::new_unchecked_for_tests(peer.clone());
        match coord.run().await {
            Err(SyncError::ChunkHashMismatch { chunk: 1 }) => {}
            other => panic!("expected ChunkHashMismatch{{1}}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn coordinator_rejects_manifest_state_root_mismatch() {
        let peer = Arc::new(MockPeer::new(200));
        *peer.tamper_state_root.lock().unwrap() = true;
        let coord = SyncCoordinator::new_unchecked_for_tests(peer.clone());
        match coord.run().await {
            Err(SyncError::Interrupted(s)) if s.contains("state_root") => {}
            other => panic!("expected Interrupted(state_root), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn coordinator_rejects_chain_below_pivot_floor() {
        let peer = Arc::new(MockPeer::new(10)); // < COORD_SAFE_PIVOT
        let coord = SyncCoordinator::new_unchecked_for_tests(peer);
        match coord.run().await {
            Err(SyncError::PivotNotFinalized(10)) => {}
            other => panic!("expected PivotNotFinalized(10), got {other:?}"),
        }
    }

    /// SEC-2026-05-09 Pass-11 architect follow-up regression test.
    /// Attacker scenario: peer sends a manifest whose `state_root`
    /// matches the pivot header (real), but whose `chunk_roots` are
    /// roots of attacker-chosen leaves. Each chunk's leaves match
    /// its committed chunk_root (so `verify_chunk` passes), but the
    /// union of all chunks does NOT compose to pivot_state_root.
    /// `verify_global_state_root` MUST catch this — without it the
    /// attacker could inject arbitrary state.
    #[tokio::test]
    async fn coordinator_rejects_global_state_root_mismatch_attack() {
        struct AttackerPeer { inner: MockPeer,
                              evil0: Vec<(H256, Vec<u8>)>,
                              evil1: Vec<(H256, Vec<u8>)>,
                              evil0_root: H256,
                              evil1_root: H256 }
        impl AttackerPeer {
            fn new(tip: u64) -> Self {
                let inner = MockPeer::new(tip);
                // Attacker-chosen leaves with self-consistent
                // chunk_roots but different aggregate hash.
                let evil0: Vec<(H256, Vec<u8>)> =
                    (0..16u8).map(|i| (key(i), vec![0xCA, i])).collect();
                let evil1: Vec<(H256, Vec<u8>)> =
                    (16..32u8).map(|i| (key(i), vec![0xFE, i])).collect();
                let evil0_root = build_global_root(&[&evil0]);
                let evil1_root = build_global_root(&[&evil1]);
                Self { inner, evil0, evil1, evil0_root, evil1_root }
            }
        }
        #[async_trait]
        impl SyncPeer for AttackerPeer {
            async fn tip(&self) -> Result<(u64, H256), SyncError> {
                self.inner.tip().await
            }
            async fn get_headers(&self, from: u64, count: u32)
                -> Result<Vec<BlockHeader>, SyncError>
            {
                self.inner.get_headers(from, count).await
            }
            async fn get_snapshot_meta(&self, p: u64)
                -> Result<SnapshotMeta, SyncError>
            {
                // Real pivot state_root, but evil chunk_roots.
                // SEC-2026-05-09 Pass-19 (Task #10): legacy attacker
                // test runs without validator set; placeholder BLS
                // fields. `chunk_root` is locally recomputed.
                let chunk_roots = vec![self.evil0_root, self.evil1_root];
                let chunk_root = crate::merkle::merkle_root(&chunk_roots);
                Ok(SnapshotMeta {
                    pivot_height: p,
                    state_root:   self.inner.pivot_state_root,
                    chunk_roots,
                    chunk_root,
                    bls_quorum_sig: BlsAggSignature([0u8; 96]),
                    bls_signers:    ValidatorBitmap::new(0),
                })
            }
            async fn get_chunk(&self, _p: u64, c: u64)
                -> Result<Vec<(H256, Vec<u8>)>, SyncError>
            {
                Ok(match c {
                    0 => self.evil0.clone(),
                    1 => self.evil1.clone(),
                    _ => return Err(SyncError::Interrupted("bad".into())),
                })
            }
        }
        let peer = Arc::new(AttackerPeer::new(200));
        let coord = SyncCoordinator::new_unchecked_for_tests(peer);
        match coord.run().await {
            Err(SyncError::Interrupted(s)) if s.contains("global state_root") => {}
            other => panic!(
                "expected global state_root mismatch rejection, got {other:?}"
            ),
        }
    }

    /// Header chain integrity: a peer that flips one parent_hash mid-
    /// stream must be detected.
    #[tokio::test]
    async fn coordinator_rejects_broken_header_chain() {
        struct BadHeaderPeer { inner: MockPeer }
        #[async_trait]
        impl SyncPeer for BadHeaderPeer {
            async fn tip(&self) -> Result<(u64, H256), SyncError> { self.inner.tip().await }
            async fn get_headers(&self, from: u64, count: u32)
                -> Result<Vec<BlockHeader>, SyncError>
            {
                let mut hs = self.inner.get_headers(from, count).await?;
                if from <= 5 && from + (hs.len() as u64) > 5 {
                    let idx = (5 - from) as usize;
                    hs[idx].parent_hash = H256([0xBE; 32]);
                }
                Ok(hs)
            }
            async fn get_snapshot_meta(&self, p: u64) -> Result<SnapshotMeta, SyncError> {
                self.inner.get_snapshot_meta(p).await
            }
            async fn get_chunk(&self, p: u64, c: u64)
                -> Result<Vec<(H256, Vec<u8>)>, SyncError>
            {
                self.inner.get_chunk(p, c).await
            }
        }
        let peer = Arc::new(BadHeaderPeer { inner: MockPeer::new(200) });
        let coord = SyncCoordinator::new_unchecked_for_tests(peer).with_batch(8);
        match coord.run().await {
            Err(SyncError::InvalidBlock(5, _)) => {}
            other => panic!("expected InvalidBlock(5,_), got {other:?}"),
        }
    }
}
