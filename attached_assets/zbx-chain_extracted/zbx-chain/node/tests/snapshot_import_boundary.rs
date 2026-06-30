//! Task #22 — Integration test for the snapshot manifest import
//! boundary.
//!
//! This is a true integration test (lives under `node/tests/` rather
//! than as an in-module `#[cfg(test)]`) so it links the `zbx-node`
//! binary's public surface end-to-end and proves the import-boundary
//! behaviour without depending on private items.
//!
//! Coverage:
//!
//! 1. **Stale-manifest rejection at the import boundary.** A
//!    validly-signed manifest at height H_old is presented to the
//!    importer with a trusted checkpoint at height H_new; the call
//!    must surface `SnapshotError::CheckpointMismatch` (the Pass-19
//!    CRIT #3 defence).
//! 2. **Live-chain TOCTOU closure.** On mainnet/testnet chain ids,
//!    a manifest file present in `data_dir` with `None` checkpoint
//!    cannot reach `verify` in tooling mode — `maybe_import_snapshot`
//!    must return `CheckpointRequired` *atomically* with the file
//!    read. This is the regression guard for the first code-review
//!    pass.

use bincode;
use tempfile::tempdir;

use zbx_crypto::bls::BlsPrivKey;
use zbx_node::snapshot_import::{
    maybe_import_snapshot, TrustedCheckpoint, MANIFEST_FILENAME,
    SnapshotImportError,
};
use zbx_state::snapshot::{
    SnapshotError, SnapshotManifest, SNAPSHOT_MANIFEST_VERSION,
};
use zbx_types::{H256, CHAIN_ID_MAINNET, CHAIN_ID_TESTNET};

fn write_signed_manifest(
    dir: &std::path::Path,
    height: u64,
    block_hash: [u8; 32],
    chain_id: u64,
) -> zbx_crypto::bls::BlsPubKey {
    let sk = BlsPrivKey::from_bytes(&[7u8; 32]).unwrap();
    let pk = sk.to_pubkey();
    let m = SnapshotManifest {
        version: SNAPSHOT_MANIFEST_VERSION,
        chain_id,
        block_height: height,
        block_hash: H256(block_hash),
        state_root: H256([0x22; 32]),
        code_hashes_root: H256([0x33; 32]),
        validator_set_root: H256([0x44; 32]),
        chunks_root: H256([0x55; 32]),
        timestamp_unix: 1_700_000_000,
    };
    let signed = m.sign(&sk).unwrap();
    let bytes = bincode::serialize(&signed).unwrap();
    std::fs::write(dir.join(MANIFEST_FILENAME), bytes).unwrap();
    pk
}

#[test]
fn integration_stale_manifest_rejected_at_import_boundary() {
    let dir = tempdir().unwrap();
    // Producer signs a manifest at height 1_000.
    let pk = write_signed_manifest(dir.path(), 1_000, [0xAA; 32], CHAIN_ID_TESTNET);
    // Operator's trusted checkpoint anchors against a NEWER block.
    let fresh = TrustedCheckpoint::from_chain_config(2_000, H256([0xBB; 32]));
    let err = maybe_import_snapshot(
        dir.path(),
        CHAIN_ID_TESTNET,
        &[pk],
        Some(fresh),
    )
    .expect_err("stale manifest must be rejected at import boundary");
    match err {
        SnapshotImportError::Verify(SnapshotError::CheckpointMismatch {
            got_height,
            exp_height,
            ..
        }) => {
            assert_eq!(got_height, 1_000);
            assert_eq!(exp_height, 2_000);
        }
        other => panic!("expected CheckpointMismatch, got {other:?}"),
    }
}

#[test]
fn integration_live_chain_manifest_without_checkpoint_is_fatal() {
    for chain_id in [CHAIN_ID_MAINNET, CHAIN_ID_TESTNET] {
        let dir = tempdir().unwrap();
        let pk = write_signed_manifest(dir.path(), 5, [0x05; 32], chain_id);
        // No trusted checkpoint -> on a live chain id, the importer
        // MUST refuse atomically (no fallback to tooling mode).
        let err = maybe_import_snapshot(dir.path(), chain_id, &[pk], None)
            .expect_err("live chain with manifest + no checkpoint must fail");
        assert!(
            matches!(
                err,
                SnapshotImportError::CheckpointRequired { chain_id: cid }
                    if cid == chain_id
            ),
            "expected CheckpointRequired on chain {chain_id}, got {err:?}"
        );
    }
}

#[test]
fn integration_live_chain_no_manifest_is_noop() {
    // Fresh data_dir with no manifest file: even on mainnet with no
    // checkpoint configured, this must NOT block boot.
    let dir = tempdir().unwrap();
    let r = maybe_import_snapshot(dir.path(), CHAIN_ID_MAINNET, &[], None);
    assert!(matches!(r, Ok(None)), "no-manifest case must be a no-op, got {r:?}");
}
