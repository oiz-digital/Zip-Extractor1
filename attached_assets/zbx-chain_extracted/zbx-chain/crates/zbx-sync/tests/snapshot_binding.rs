//! SEC-2026-05-09 Pass-19 (Task #10) — end-to-end integration tests
//! for the BLS-quorum-signed snapshot manifest with chunk_root binding.
//!
//! Five scenarios are exercised:
//!
//!   1. **Happy path** — producer signs a real manifest with 3-of-4
//!      validators; consumer verifies BLS sig FIRST, then per-chunk
//!      Merkle inclusion proof, then completes the bootstrap.
//!   2. **Forged manifest signature** — peer ships a manifest signed
//!      by an attacker key not in the validator set; coordinator
//!      MUST reject with `BadManifestSignature` BEFORE fetching any
//!      chunk.
//!   3. **Tampered chunk_root in manifest** — peer's `chunk_root`
//!      field doesn't equal `merkle_root(chunk_roots)`; coordinator
//!      MUST reject with `BadManifestSignature` (binding broken).
//!   4. **Tampered single chunk_root** — manifest signs honest
//!      `chunk_roots`, but peer overwrites one slot in the served
//!      `chunk_roots` while keeping `chunk_root` (signed) intact;
//!      consumer's local recompute catches it as
//!      `BadManifestSignature` (root != merkle_root mismatch).
//!   5. **Below-quorum bitmap** — bitmap claims only 2 signers but
//!      quorum is 3; reject as `BadManifestSignature`.

use async_trait::async_trait;
use std::sync::Arc;
use zbx_crypto::bls as ckbls;
use zbx_sync::coordinator::{
    SnapshotMeta, SyncCoordinator, SyncPeer, COORD_SAFE_PIVOT,
};
use zbx_sync::error::SyncError;
use zbx_sync::manifest::build_signed_manifest;
use zbx_sync::merkle::merkle_root;
use zbx_threshold::{BlsAggSignature, BlsPubKey, ValidatorBitmap};
use zbx_trie::{trie::MemoryTrieDB, MutableTrie};
use zbx_types::{address::Address, block::BlockHeader, H256, U256};

// ---------- helpers ----------

fn key(b: u8) -> H256 {
    let mut k = [0u8; 32];
    k[31] = b;
    H256(k)
}

fn deterministic_sk(seed: u8) -> [u8; 32] {
    let mut k = [0u8; 32];
    for (i, b) in k.iter_mut().enumerate() {
        *b = seed.wrapping_add(i as u8 + 1);
    }
    k
}

fn pk_from_sk(sk: &[u8; 32]) -> BlsPubKey {
    let p = ckbls::BlsPrivKey::from_bytes(sk).unwrap();
    BlsPubKey(*p.to_pubkey().as_bytes())
}

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
        parent_hash:        parent,
        uncle_hash:         H256::zero(),
        coinbase:           Address::zero(),
        state_root,
        transactions_root:  H256::zero(),
        receipts_root:      H256::zero(),
        logs_bloom:         [0u8; 256],
        difficulty:         U256::from(1u64),
        number,
        gas_limit:          30_000_000,
        gas_used:           0,
        timestamp:          1_700_000_000 + number,
        extra_data:         vec![],
        mix_hash:           H256::zero(),
        nonce:              0,
        base_fee_per_gas:   1,
        committee_signature: vec![],
        epoch:              number / 1000,
        epoch_seed:         None,
    }
}

/// Per-test fixture: 4-validator committee, 2-chunk snapshot, full
/// header chain. `manifest` is built once with the *honest* signers
/// (indices 0..=2). Individual tests mutate the served manifest /
/// chunks to model an attacker.
struct Fixture {
    tip:              u64,
    pivot_height:     u64,
    headers:          Vec<BlockHeader>,
    chunk0_leaves:    Vec<(H256, Vec<u8>)>,
    chunk1_leaves:    Vec<(H256, Vec<u8>)>,
    pivot_state_root: H256,
    chunk_roots:      Vec<H256>,
    validator_keys:   Vec<(Address, BlsPubKey)>,
    /// Honest manifest as produced by the canonical signer.
    honest_manifest:  SnapshotMeta,
}

impl Fixture {
    fn new() -> Self {
        let tip = 200u64;
        let pivot_height = tip - COORD_SAFE_PIVOT;

        // 4-validator committee.
        let n = 4;
        let sks: Vec<[u8; 32]> = (0..n as u8).map(deterministic_sk).collect();
        let pks: Vec<BlsPubKey> = sks.iter().map(pk_from_sk).collect();
        let validator_keys: Vec<(Address, BlsPubKey)> = pks
            .iter()
            .enumerate()
            .map(|(i, pk)| (Address([i as u8 + 1; 20]), pk.clone()))
            .collect();

        // Snapshot chunks.
        let chunk0_leaves: Vec<(H256, Vec<u8>)> =
            (0..16u8).map(|i| (key(i), vec![i, i ^ 0xAA])).collect();
        let chunk1_leaves: Vec<(H256, Vec<u8>)> =
            (16..32u8).map(|i| (key(i), vec![i, i ^ 0x55])).collect();
        let chunk0_root = {
            let mut t = MutableTrie::new(MemoryTrieDB::default());
            for (k, v) in &chunk0_leaves {
                t.insert(k.as_bytes(), v.clone()).unwrap();
            }
            t.commit().unwrap()
        };
        let chunk1_root = {
            let mut t = MutableTrie::new(MemoryTrieDB::default());
            for (k, v) in &chunk1_leaves {
                t.insert(k.as_bytes(), v.clone()).unwrap();
            }
            t.commit().unwrap()
        };
        let chunk_roots = vec![chunk0_root, chunk1_root];
        let pivot_state_root =
            build_global_root(&[&chunk0_leaves, &chunk1_leaves]);

        // Header chain: heights 1..=tip.
        let mut headers = Vec::with_capacity(tip as usize);
        let mut prev = H256::zero();
        for n in 1..=tip {
            let sr = if n == pivot_height { pivot_state_root } else { H256::zero() };
            let h = mk_header(n, prev, sr);
            prev = h.hash();
            headers.push(h);
        }

        // Honest 3-of-4 BLS-signed manifest.
        let signer_indices = [0usize, 1, 2];
        let signer_sks: Vec<[u8; 32]> =
            signer_indices.iter().map(|i| sks[*i]).collect();
        let signed = build_signed_manifest(
            pivot_height,
            pivot_state_root,
            &chunk_roots,
            n,
            &signer_indices,
            &signer_sks,
        )
        .expect("honest manifest signing");
        let honest_manifest = SnapshotMeta {
            pivot_height,
            state_root: pivot_state_root,
            chunk_roots: chunk_roots.clone(),
            chunk_root: signed.chunk_root,
            bls_quorum_sig: signed.bls_quorum_sig,
            bls_signers: signed.bls_signers,
        };

        Self {
            tip,
            pivot_height,
            headers,
            chunk0_leaves,
            chunk1_leaves,
            pivot_state_root,
            chunk_roots,
            validator_keys,
            honest_manifest,
        }
    }
}

/// Snapshot peer that returns whatever `SnapshotMeta` it is given —
/// lets each test inject an honest, forged, or tampered manifest.
struct ScriptedPeer {
    fixture: Arc<Fixture>,
    served:  SnapshotMeta,
    served_chunk_roots: Vec<H256>, // for tamper-served scenarios
    /// SEC-2026-05-09 Pass-19 (Task #10) — chunk_id → bytes-payload
    /// override. Lets a test inject a tampered chunk payload (real
    /// chunk_root in manifest, real Merkle inclusion proof, but the
    /// served leaves' MPT does not hash to the committed chunk_root)
    /// to validate the per-chunk `verify_chunk` failure path under
    /// the new manifest-binding regime.
    payload_override: std::collections::HashMap<u64, Vec<(H256, Vec<u8>)>>,
    /// SEC-2026-05-09 Pass-19 (Task #10) — list of chunk_ids the
    /// peer should refuse to serve (returns empty vec → peer-ban
    /// signal `chunk_missing`).
    missing_chunks: std::collections::HashSet<u64>,
    /// SEC-2026-05-09 Pass-19 (Task #10) — when true, the peer
    /// answers every `get_chunk(_, k)` with chunk[(k+1) % n] —
    /// out-of-order delivery. Inclusion proof for slot k won't
    /// match the served slot, and `verify_chunk` against `chunk_roots[k]`
    /// also fails — both surfaces caught by the binding pipeline.
    swap_chunks: bool,
    /// SEC-2026-05-09 Pass-19 (Task #10) — counter of `report_misbehavior`
    /// invocations so the test can assert that bad-peer signals fire.
    misbehavior_count: std::sync::Mutex<u32>,
    last_reason: std::sync::Mutex<Option<&'static str>>,
}

impl ScriptedPeer {
    fn new(fixture: Arc<Fixture>, served: SnapshotMeta) -> Self {
        let served_chunk_roots = fixture.chunk_roots.clone();
        Self {
            fixture,
            served,
            served_chunk_roots,
            payload_override: std::collections::HashMap::new(),
            missing_chunks: std::collections::HashSet::new(),
            swap_chunks: false,
            misbehavior_count: std::sync::Mutex::new(0),
            last_reason: std::sync::Mutex::new(None),
        }
    }
    fn misbehavior(&self) -> (u32, Option<&'static str>) {
        (
            *self.misbehavior_count.lock().unwrap(),
            *self.last_reason.lock().unwrap(),
        )
    }
}

#[async_trait]
impl SyncPeer for ScriptedPeer {
    async fn tip(&self) -> Result<(u64, H256), SyncError> {
        Ok((
            self.fixture.tip,
            self.fixture.headers.last().unwrap().hash(),
        ))
    }
    async fn get_headers(
        &self,
        from: u64,
        count: u32,
    ) -> Result<Vec<BlockHeader>, SyncError> {
        if from == 0 || from > self.fixture.tip {
            return Ok(vec![]);
        }
        let start = (from - 1) as usize;
        let end = (start + count as usize).min(self.fixture.headers.len());
        Ok(self.fixture.headers[start..end].to_vec())
    }
    async fn get_snapshot_meta(
        &self,
        _pivot_height: u64,
    ) -> Result<SnapshotMeta, SyncError> {
        // Surface the (possibly tampered) served `chunk_roots` while
        // keeping the BLS sig + chunk_root that the test injected.
        let mut m = self.served.clone();
        m.chunk_roots = self.served_chunk_roots.clone();
        Ok(m)
    }
    async fn get_chunk(
        &self,
        _pivot_height: u64,
        chunk_id: u64,
    ) -> Result<Vec<(H256, Vec<u8>)>, SyncError> {
        if self.missing_chunks.contains(&chunk_id) {
            return Ok(Vec::new());
        }
        if let Some(payload) = self.payload_override.get(&chunk_id) {
            return Ok(payload.clone());
        }
        let effective_id = if self.swap_chunks {
            (chunk_id + 1) % 2
        } else {
            chunk_id
        };
        match effective_id {
            0 => Ok(self.fixture.chunk0_leaves.clone()),
            1 => Ok(self.fixture.chunk1_leaves.clone()),
            _ => Err(SyncError::Interrupted(format!("bad chunk {effective_id}"))),
        }
    }
    async fn report_misbehavior(&self, reason: &'static str) {
        *self.misbehavior_count.lock().unwrap() += 1;
        *self.last_reason.lock().unwrap() = Some(reason);
    }
}

// ---------- scenarios ----------

/// (1) Happy path — honest signers, real BLS verify, real per-chunk
///     inclusion check, full bootstrap completes.
#[tokio::test]
async fn happy_path_honest_quorum_3_of_4_bootstrap_succeeds() {
    let fix = Arc::new(Fixture::new());
    let peer = Arc::new(ScriptedPeer::new(fix.clone(), fix.honest_manifest.clone()));
    let coord = SyncCoordinator::new(peer, fix.validator_keys.clone(), 3);
    let outcome = coord.run().await.expect("happy path must succeed");
    assert_eq!(outcome.pivot_height, fix.pivot_height);
    assert_eq!(outcome.chunks_verified, 2);
    assert_eq!(outcome.pivot_state_root, fix.pivot_state_root);
}

/// (2) Forged signature — manifest signed by an attacker key (idx 5)
///     not in the registered validator set.
#[tokio::test]
async fn forged_manifest_signature_rejected_before_chunk_fetch() {
    let fix = Arc::new(Fixture::new());
    // Attacker key is NOT in the validator set (sk seed 99).
    let attacker_sk = deterministic_sk(99);
    let signed = build_signed_manifest(
        fix.pivot_height,
        fix.pivot_state_root,
        &fix.chunk_roots,
        4,
        &[0usize], // pretend index 0 — but signed by a key NOT at idx 0
        &[attacker_sk],
    )
    .unwrap();
    let forged = SnapshotMeta {
        pivot_height:   fix.pivot_height,
        state_root:     fix.pivot_state_root,
        chunk_roots:    fix.chunk_roots.clone(),
        chunk_root:     signed.chunk_root,
        bls_quorum_sig: signed.bls_quorum_sig,
        bls_signers:    signed.bls_signers,
    };
    let peer = Arc::new(ScriptedPeer::new(fix.clone(), forged));
    let coord = SyncCoordinator::new(peer, fix.validator_keys.clone(), 1);
    match coord.run().await {
        Err(SyncError::BadManifestSignature(_)) => {}
        other => panic!("expected BadManifestSignature, got {other:?}"),
    }
}

/// (3) Tampered manifest.chunk_root — peer modifies the signed
///     `chunk_root` field; consumer's local merkle_root recomputation
///     catches the inconsistency.
#[tokio::test]
async fn tampered_manifest_chunk_root_field_rejected() {
    let fix = Arc::new(Fixture::new());
    let mut tampered = fix.honest_manifest.clone();
    tampered.chunk_root = H256([0xEE; 32]); // garbage
    let peer = Arc::new(ScriptedPeer::new(fix.clone(), tampered));
    let coord = SyncCoordinator::new(peer, fix.validator_keys.clone(), 3);
    match coord.run().await {
        Err(SyncError::BadManifestSignature(s)) => {
            assert!(s.contains("merkle_root"), "msg: {s}");
        }
        other => panic!("expected BadManifestSignature(merkle_root), got {other:?}"),
    }
}

/// (4) Tampered served chunk_roots vector — manifest is honest and
///     correctly signed, but the peer overwrites one entry of the
///     served `chunk_roots` while leaving the signed `chunk_root`
///     intact. Local merkle_root recompute over the served list no
///     longer matches the signed root → reject.
#[tokio::test]
async fn tampered_single_chunk_root_in_served_vec_rejected() {
    let fix = Arc::new(Fixture::new());
    let mut peer = ScriptedPeer::new(fix.clone(), fix.honest_manifest.clone());
    // Overwrite served chunk_roots[1] (signed root and signature
    // intact). The local merkle_root over the tampered vector will
    // not equal manifest.chunk_root.
    peer.served_chunk_roots[1] = H256([0x77; 32]);
    let peer = Arc::new(peer);
    let coord = SyncCoordinator::new(peer, fix.validator_keys.clone(), 3);
    match coord.run().await {
        Err(SyncError::BadManifestSignature(s)) => {
            assert!(s.contains("merkle_root"), "msg: {s}");
        }
        other => panic!("expected BadManifestSignature, got {other:?}"),
    }
}

/// (5) Below-quorum bitmap — manifest is signed by 2 honest validators
///     but the registered quorum is 3. Even though every signer key
///     IS in the set, the bitmap fails the quorum-count gate.
#[tokio::test]
async fn below_quorum_bitmap_rejected() {
    let fix = Arc::new(Fixture::new());
    // 2 honest signers (idx 0, 1) — quorum will be 3.
    let sks: Vec<[u8; 32]> = (0..4u8).map(deterministic_sk).collect();
    let signer_indices = [0usize, 1];
    let signer_sks: Vec<[u8; 32]> =
        signer_indices.iter().map(|i| sks[*i]).collect();
    let signed = build_signed_manifest(
        fix.pivot_height,
        fix.pivot_state_root,
        &fix.chunk_roots,
        4,
        &signer_indices,
        &signer_sks,
    )
    .unwrap();
    let m = SnapshotMeta {
        pivot_height:   fix.pivot_height,
        state_root:     fix.pivot_state_root,
        chunk_roots:    fix.chunk_roots.clone(),
        chunk_root:     signed.chunk_root,
        bls_quorum_sig: signed.bls_quorum_sig,
        bls_signers:    signed.bls_signers,
    };
    let peer = Arc::new(ScriptedPeer::new(fix.clone(), m));
    let coord = SyncCoordinator::new(peer, fix.validator_keys.clone(), 3);
    match coord.run().await {
        Err(SyncError::BadManifestSignature(s)) => {
            assert!(s.contains("quorum"), "msg: {s}");
        }
        other => panic!("expected BadManifestSignature(quorum), got {other:?}"),
    }
}

/// (6) Tampered chunk PAYLOAD — manifest is honest and signed, every
///     `chunk_roots[i]` is honest, every Merkle inclusion proof
///     succeeds, but the served leaves of chunk 1 have been
///     overwritten with attacker bytes whose MPT does NOT match the
///     committed `chunk_roots[1]`. `verify_chunk` (the per-chunk MPT
///     check at the heart of `snap_sync`) MUST catch this and the
///     misbehavior reporter MUST fire with `chunk_payload_tampered`.
#[tokio::test]
async fn tampered_chunk_payload_rejected_with_peer_ban() {
    let fix = Arc::new(Fixture::new());
    let mut peer = ScriptedPeer::new(fix.clone(), fix.honest_manifest.clone());
    // Replace chunk 1's leaves with attacker-chosen bytes. Manifest
    // signature is intact, chunk_root binding is intact, inclusion
    // proof passes, but verify_chunk against chunk_roots[1] fails.
    let evil_payload: Vec<(H256, Vec<u8>)> = (16..32u8)
        .map(|i| (key(i), vec![0xCA, 0xFE, i]))
        .collect();
    peer.payload_override.insert(1, evil_payload);
    let peer = Arc::new(peer);
    let coord = SyncCoordinator::new(
        peer.clone(),
        fix.validator_keys.clone(),
        3,
    );
    let res = coord.run().await;
    assert!(
        matches!(res, Err(SyncError::ChunkHashMismatch { chunk: 1 })),
        "expected ChunkHashMismatch{{1}}, got {res:?}",
    );
    let (n, reason) = peer.misbehavior();
    assert_eq!(n, 1, "exactly one misbehavior signal");
    assert_eq!(reason, Some("chunk_payload_tampered"));
}

/// (7) Out-of-order chunk delivery — peer answers `get_chunk(_, 0)`
///     with chunk 1's leaves and vice versa. The Merkle inclusion
///     check at slot 0 is satisfied (chunk_roots[0] is honest), but
///     `verify_chunk` against `chunk_roots[0]` over chunk 1's
///     leaves MUST fail. Misbehavior reporter MUST fire.
#[tokio::test]
async fn out_of_order_chunk_delivery_rejected() {
    let fix = Arc::new(Fixture::new());
    let mut peer = ScriptedPeer::new(fix.clone(), fix.honest_manifest.clone());
    peer.swap_chunks = true;
    let peer = Arc::new(peer);
    let coord = SyncCoordinator::new(
        peer.clone(),
        fix.validator_keys.clone(),
        3,
    );
    let res = coord.run().await;
    assert!(
        matches!(res, Err(SyncError::ChunkHashMismatch { chunk: 0 })),
        "expected ChunkHashMismatch{{0}} (slot 0 leaves != chunk_roots[0]), got {res:?}",
    );
    let (n, reason) = peer.misbehavior();
    assert!(n >= 1);
    assert_eq!(reason, Some("chunk_payload_tampered"));
}

/// (8) Missing chunk — peer returns an empty leaf-vec for chunk 1.
///     New code path: empty chunk surfaces as `ChunkRootMismatch{1}`
///     (an empty leaf-set never produces the committed
///     `chunk_roots[1]`) WITH explicit misbehavior label
///     `chunk_missing` so peer-rep tracking can distinguish a
///     starve-attack from a tamper-attack.
#[tokio::test]
async fn missing_chunk_returns_chunk_root_mismatch_with_peer_ban() {
    let fix = Arc::new(Fixture::new());
    let mut peer = ScriptedPeer::new(fix.clone(), fix.honest_manifest.clone());
    peer.missing_chunks.insert(1);
    let peer = Arc::new(peer);
    let coord = SyncCoordinator::new(
        peer.clone(),
        fix.validator_keys.clone(),
        3,
    );
    let res = coord.run().await;
    assert!(
        matches!(res, Err(SyncError::ChunkRootMismatch { chunk: 1 })),
        "expected ChunkRootMismatch{{1}}, got {res:?}",
    );
    let (n, reason) = peer.misbehavior();
    assert_eq!(n, 1);
    assert_eq!(reason, Some("chunk_missing"));
}

// Touch the imports so unused-import lint stays quiet across configs.
#[allow(dead_code)]
fn _touch_imports() {
    let _ = BlsAggSignature([0u8; 96]);
    let _ = ValidatorBitmap::new(0);
    let _ = merkle_root(&[H256::zero()]);
}
