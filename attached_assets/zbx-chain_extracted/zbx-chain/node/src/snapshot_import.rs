//! Task #22 — Snapshot manifest verification at the fast-sync import
//! boundary.
//!
//! # Why this exists
//!
//! Pass-19 hardened `zbx_state::snapshot::SignedSnapshotManifest::verify`
//! by adding the `expected_checkpoint: Option<(u64, H256)>` argument so
//! that a *validly-signed but stale* manifest from an authorised
//! producer is rejected at the import boundary (CRIT #3 — same-chain
//! stale-replay defence). That check is library-only until a real
//! consumer passes `Some(...)`.
//!
//! This module is the actual consumer. It:
//!
//!   1. Defines a [`TrustedCheckpoint`] type with private fields whose
//!      *only* constructor takes a `(height, hash)` pair from a
//!      caller-attested source. Constructing one is a positive
//!      acknowledgement that the caller has an externally-trusted
//!      anchor for "what block I am syncing to".
//!   2. Defines [`ImportMode`] which is either [`ImportMode::Live`]
//!      (carries a `TrustedCheckpoint`) or [`ImportMode::Tooling`]
//!      (no checkpoint, only valid for offline tooling like
//!      `zbx-snapshot inspect`). The two-variant enum is the
//!      compile-time gate the task requires: a mainnet/testnet sync
//!      path that builds an `ImportMode::Tooling` is a code-review
//!      red flag, not a runtime config knob.
//!   3. Refuses to construct `ImportMode::Live` for a syncing node
//!      whose chain id is mainnet/testnet UNLESS a
//!      `TrustedCheckpoint` is provided — enforced by
//!      [`ImportMode::for_live_chain`] returning
//!      `Err(SnapshotImportError::CheckpointRequired)` when the
//!      caller passes `None` on a live network.
//!   4. Reads the trusted (height, hash) from the chain config's new
//!      `[chain.trusted_snapshot_checkpoint]` table. Operators pin
//!      this to the latest finalised block-hash served by their
//!      checkpoint provider (typically a governance-published hash
//!      or a hard-coded chain-config pin per release).
//!   5. Calls `SignedSnapshotManifest::verify(.., Some(ckpt), ..)`
//!      so the new freshness defence is enforced on the live caller.
//!
//! # Where this is wired
//!
//! `node/src/main.rs` calls [`maybe_import_snapshot`] right after the
//! Task #14 mainnet readiness check passes and before
//! `ZbxNode::new(..)`. When `<data_dir>/snapshot.manifest.bin` is
//! present, the node verifies it against the chain config's allowed
//! producer set + trusted checkpoint. A mismatch is fatal — the
//! operator must either delete the stale manifest, or update the
//! checkpoint pin to the newer one.

use std::path::{Path, PathBuf};

use bincode;
use thiserror::Error;
use tracing::{info, warn};
use zbx_crypto::bls::BlsPubKey;
use zbx_state::snapshot::{SignedSnapshotManifest, SnapshotError};
use zbx_types::H256;

/// Filename the importer looks for inside `<data_dir>`. Pre-Pass-19
/// snapshot exporters wrote manifest blobs to assorted ad-hoc paths;
/// the importer pins one canonical name so an operator who copies the
/// snapshot bundle into `data_dir` doesn't have to re-configure
/// anything. A future SST exporter must write to this same name.
pub const MANIFEST_FILENAME: &str = "snapshot.manifest.bin";

/// Externally-trusted (height, hash) the syncing node anchors snapshot
/// imports against. Constructed via [`TrustedCheckpoint::new`] so the
/// caller's source of trust (chain config pin, governance message,
/// etc.) is documented at the call site.
///
/// The fields are private; the only way to read them out is via
/// [`TrustedCheckpoint::height`] / [`TrustedCheckpoint::hash`]. This
/// keeps the "I deliberately attested to a checkpoint" property
/// auditable — `grep TrustedCheckpoint::new` enumerates every place
/// where the freshness anchor enters the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedCheckpoint {
    height: u64,
    hash:   H256,
    source: CheckpointSource,
}

/// Where a [`TrustedCheckpoint`] came from. Surfaces in logs so
/// operators see which trust anchor was used for any given import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointSource {
    /// Pinned in the chain config TOML (`[chain.trusted_snapshot_checkpoint]`).
    /// This is the production path on mainnet/testnet — the pin is
    /// rotated on each release whose snapshot the operator wants to
    /// accept.
    ChainConfigPin,
    /// Passed in via CLI / API for ad-hoc imports (currently unused
    /// by the binary; reserved for the future `zbx-cli sync-from`
    /// command).
    OperatorOverride,
    /// Used in tests to thread a known checkpoint into the importer.
    /// Production binaries must NOT construct this variant — the
    /// `#[cfg(test)]`-gated constructor is the only way to obtain it.
    #[cfg(test)]
    TestFixture,
}

impl TrustedCheckpoint {
    /// Construct from a chain-config pin. The (height, hash) MUST
    /// have been read from the operator's trusted source (config
    /// TOML, governance feed, hard-coded release pin); this
    /// constructor does no validation of its own — the type only
    /// documents intent.
    pub fn from_chain_config(height: u64, hash: H256) -> Self {
        Self { height, hash, source: CheckpointSource::ChainConfigPin }
    }

    /// Construct from an operator CLI / API override. Reserved for
    /// future use; not currently called by the node binary.
    pub fn from_operator_override(height: u64, hash: H256) -> Self {
        Self { height, hash, source: CheckpointSource::OperatorOverride }
    }

    #[cfg(test)]
    pub(crate) fn for_test(height: u64, hash: H256) -> Self {
        Self { height, hash, source: CheckpointSource::TestFixture }
    }

    pub fn height(&self) -> u64 { self.height }
    pub fn hash(&self) -> H256 { self.hash }
    pub fn source(&self) -> CheckpointSource { self.source }
}

/// Two-variant enum that gates whether a snapshot import is allowed
/// to proceed without a freshness anchor. Only [`ImportMode::Tooling`]
/// passes `expected_checkpoint = None` to the underlying verifier;
/// [`ImportMode::for_live_chain`] refuses to produce that variant on
/// any chain id that matches a known live network.
#[derive(Debug, Clone, Copy)]
pub enum ImportMode {
    /// Production sync path. The carried [`TrustedCheckpoint`] is
    /// passed as `Some(...)` to
    /// `SignedSnapshotManifest::verify` and enforces the
    /// stale-replay defence.
    Live(TrustedCheckpoint),
    /// Offline tooling path (e.g. `zbx-snapshot inspect <file>`,
    /// developer CLI exploration). Skips the checkpoint binding.
    /// MUST NOT be used by any sync code path on a chain id that
    /// matches a live network — see [`ImportMode::for_live_chain`].
    Tooling,
}

impl ImportMode {
    /// Build the import mode for a live (mainnet/testnet/devnet) chain.
    /// Returns `Err(CheckpointRequired)` when the operator passed
    /// `None` — the live chain id paths cannot legally skip the
    /// freshness binding.
    ///
    /// `chain_id` is accepted purely for the error message; every
    /// live chain id (mainnet 8989, testnet/devnet 8990) requires
    /// the checkpoint. A future "private chain id" branch could opt
    /// out, but doing so explicitly here keeps the gate visible.
    pub fn for_live_chain(
        chain_id: u64,
        ckpt: Option<TrustedCheckpoint>,
    ) -> Result<Self, SnapshotImportError> {
        match ckpt {
            Some(c) => Ok(ImportMode::Live(c)),
            None => Err(SnapshotImportError::CheckpointRequired { chain_id }),
        }
    }

    /// Explicit opt-out for offline tooling. Documented as an
    /// `unsafe`-named constructor (no actual `unsafe` keyword — Rust
    /// reserves that for memory invariants — but the name surfaces
    /// the intent in code review).
    pub fn unsafe_tooling_no_checkpoint() -> Self {
        Self::Tooling
    }

    fn as_verify_arg(&self) -> Option<(u64, H256)> {
        match self {
            ImportMode::Live(c) => Some((c.height(), c.hash())),
            ImportMode::Tooling => None,
        }
    }
}

#[derive(Debug, Error)]
pub enum SnapshotImportError {
    #[error(
        "snapshot import on live chain {chain_id} requires a trusted (height, hash) \
         checkpoint — set [chain.trusted_snapshot_checkpoint] in the node config or \
         remove the snapshot manifest from <data_dir>"
    )]
    CheckpointRequired { chain_id: u64 },

    #[error("snapshot manifest at {path}: cannot read: {source}")]
    Io { path: PathBuf, source: std::io::Error },

    #[error("snapshot manifest at {path}: bincode decode failed: {source}")]
    Decode {
        path: String,
        #[source]
        source: BincodeErrorWrapper,
    },

    #[error("snapshot manifest verification failed: {0}")]
    Verify(#[from] SnapshotError),

    #[error(
        "snapshot manifest's allowed-producer set is empty — refusing to import. \
         Configure [chain.snapshot_allowed_producers] with at least one BLS pubkey."
    )]
    EmptyAllowedProducers,

    #[error("snapshot manifest's allowed producer pubkey #{idx} is malformed: {detail}")]
    BadAllowedProducerHex { idx: usize, detail: String },

    #[error("trusted-checkpoint hash field is malformed: {0}")]
    BadCheckpointHash(String),
}

/// Wrap `bincode::Error` as `Send + Sync + std::error::Error` so it
/// composes with `thiserror`'s `#[source]`.
#[derive(Debug, Error)]
#[error("{0}")]
pub struct BincodeErrorWrapper(pub String);

/// Verify a signed snapshot manifest using the typed import-mode
/// gate. The `verify` core lives in `zbx-state`; this wrapper exists
/// to *make `expected_checkpoint = None` impossible by accident*.
///
/// On success returns the verified manifest height + state root for
/// the caller to log / compare against the local DB.
pub fn verify_signed_manifest(
    signed: &SignedSnapshotManifest,
    expected_chain_id: u64,
    allowed_producers: &[BlsPubKey],
    mode: ImportMode,
    pinned_state_root: Option<H256>,
) -> Result<(u64, H256), SnapshotImportError> {
    if allowed_producers.is_empty() {
        return Err(SnapshotImportError::EmptyAllowedProducers);
    }
    signed.verify(
        expected_chain_id,
        allowed_producers,
        mode.as_verify_arg(),
        pinned_state_root,
    )?;
    Ok((signed.manifest.block_height, signed.manifest.state_root))
}

/// Returns `true` if `chain_id` is a known live network (mainnet 8989
/// or testnet/devnet 8990). The single source of truth for the
/// live-vs-private distinction the import-mode gate relies on.
pub fn is_live_chain_id(chain_id: u64) -> bool {
    matches!(
        chain_id,
        zbx_types::CHAIN_ID_MAINNET | zbx_types::CHAIN_ID_TESTNET
    )
}

/// Read + verify the snapshot manifest at `<data_dir>/<MANIFEST_FILENAME>`.
///
/// This is the canonical entry point for the node binary. It accepts
/// the trusted checkpoint as `Option<TrustedCheckpoint>` (rather than
/// a pre-built [`ImportMode`]) so the live-chain freshness binding is
/// decided **here, atomically with the file read** — eliminating the
/// TOCTOU window where a manifest could appear between an external
/// "do I need a checkpoint?" check and the actual verify call.
///
/// Decision table:
///
/// | chain_id  | manifest file | checkpoint  | outcome                          |
/// |-----------|---------------|-------------|----------------------------------|
/// | live      | absent        | any         | `Ok(None)` (no-op)               |
/// | live      | present       | `Some`      | verify with `Live(ckpt)`         |
/// | live      | present       | `None`      | `Err(CheckpointRequired)` FATAL  |
/// | non-live  | absent        | any         | `Ok(None)` (no-op)               |
/// | non-live  | present       | `Some`      | verify with `Live(ckpt)`         |
/// | non-live  | present       | `None`      | verify with `Tooling` + `warn!`  |
///
/// Returns `Ok(Some((height, state_root)))` on a successful verify;
/// `Err(_)` is fatal at the call site.
pub fn maybe_import_snapshot(
    data_dir: &Path,
    expected_chain_id: u64,
    allowed_producers: &[BlsPubKey],
    checkpoint: Option<TrustedCheckpoint>,
) -> Result<Option<(u64, H256)>, SnapshotImportError> {
    let path = data_dir.join(MANIFEST_FILENAME);
    // Single authoritative read: open() succeeds iff the file is
    // present. We do NOT pre-check `path.exists()` separately — that
    // would re-open the TOCTOU window the typed gate is meant to
    // close on live chains.
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            info!(
                data_dir = %data_dir.display(),
                "Task #22: no snapshot manifest at {} — sync will proceed from \
                 local DB tip without snapshot import",
                path.display(),
            );
            return Ok(None);
        }
        Err(source) => {
            return Err(SnapshotImportError::Io {
                path: path.clone(),
                source,
            });
        }
    };
    // File IS present. Now (and only now) build the mode under the
    // typed gate. On live chains a missing checkpoint here is FATAL —
    // we can NOT silently fall through to `Tooling` because that would
    // pass `expected_checkpoint = None` to the verifier and re-open
    // the same-chain stale-replay window Pass-19 closed.
    let mode = if is_live_chain_id(expected_chain_id) {
        ImportMode::for_live_chain(expected_chain_id, checkpoint)?
    } else {
        match checkpoint {
            Some(c) => ImportMode::Live(c),
            None => ImportMode::unsafe_tooling_no_checkpoint(),
        }
    };
    info!(
        manifest = %path.display(),
        "Task #22: snapshot manifest found — verifying with trusted-checkpoint binding"
    );
    let signed: SignedSnapshotManifest = bincode::deserialize(&bytes).map_err(|e| {
        SnapshotImportError::Decode {
            path: path.display().to_string(),
            source: BincodeErrorWrapper(e.to_string()),
        }
    })?;
    let (h, root) = verify_signed_manifest(
        &signed,
        expected_chain_id,
        allowed_producers,
        mode,
        None,
    )?;
    match mode {
        ImportMode::Live(c) => info!(
            manifest_height = h,
            checkpoint_height = c.height(),
            checkpoint_source = ?c.source(),
            state_root = %hex::encode(root.as_bytes()),
            "Task #22: snapshot manifest VERIFIED against trusted checkpoint"
        ),
        ImportMode::Tooling => {
            // Defence-in-depth: even though the gate above already
            // refuses Tooling on live chains, assert it here so a
            // future refactor that loses the gate cannot silently
            // re-introduce the bypass.
            debug_assert!(
                !is_live_chain_id(expected_chain_id),
                "Tooling mode reached on live chain {expected_chain_id}"
            );
            if is_live_chain_id(expected_chain_id) {
                return Err(SnapshotImportError::CheckpointRequired {
                    chain_id: expected_chain_id,
                });
            }
            warn!(
                manifest_height = h,
                state_root = %hex::encode(root.as_bytes()),
                "Task #22: snapshot manifest verified WITHOUT freshness binding \
                 (tooling mode) — this code path must NEVER run on a live network"
            );
        }
    }
    Ok(Some((h, root)))
}

/// Parse a hex-encoded 32-byte checkpoint hash from chain config.
pub fn parse_checkpoint_hash(s: &str) -> Result<H256, SnapshotImportError> {
    let trimmed = s.trim().trim_start_matches("0x");
    let bytes = hex::decode(trimmed)
        .map_err(|e| SnapshotImportError::BadCheckpointHash(e.to_string()))?;
    if bytes.len() != 32 {
        return Err(SnapshotImportError::BadCheckpointHash(format!(
            "expected 32 bytes (64 hex chars), got {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(H256(out))
}

/// Parse a hex-encoded BLS pubkey (48 bytes, compressed G1) from
/// chain config.
pub fn parse_allowed_producer(idx: usize, s: &str) -> Result<BlsPubKey, SnapshotImportError> {
    let trimmed = s.trim().trim_start_matches("0x");
    let bytes = hex::decode(trimmed).map_err(|e| {
        SnapshotImportError::BadAllowedProducerHex {
            idx,
            detail: e.to_string(),
        }
    })?;
    BlsPubKey::from_bytes(&bytes).map_err(|e| {
        SnapshotImportError::BadAllowedProducerHex {
            idx,
            detail: format!("BLS decode: {e:?}"),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbx_crypto::bls::BlsPrivKey;
    use zbx_state::snapshot::{SnapshotManifest, SNAPSHOT_MANIFEST_VERSION};

    fn fixture_signed(height: u64, hash: [u8; 32]) -> (SignedSnapshotManifest, BlsPubKey) {
        let sk = BlsPrivKey::from_bytes(&[9u8; 32]).unwrap();
        let pk = sk.to_pubkey();
        let m = SnapshotManifest {
            version: SNAPSHOT_MANIFEST_VERSION,
            chain_id: 8990,
            block_height: height,
            block_hash: H256(hash),
            state_root: H256([0x22; 32]),
            code_hashes_root: H256([0x33; 32]),
            validator_set_root: H256([0x44; 32]),
            chunks_root: H256([0x55; 32]),
            timestamp_unix: 1_700_000_000,
        };
        (m.sign(&sk).unwrap(), pk)
    }

    #[test]
    fn live_mode_requires_checkpoint() {
        let err = ImportMode::for_live_chain(8989, None).unwrap_err();
        assert!(matches!(
            err,
            SnapshotImportError::CheckpointRequired { chain_id: 8989 }
        ));
    }

    #[test]
    fn live_mode_passes_checkpoint_through_to_verify() {
        let (signed, pk) = fixture_signed(1_000_000, [0xAB; 32]);
        let ckpt = TrustedCheckpoint::for_test(1_000_000, H256([0xAB; 32]));
        let mode = ImportMode::for_live_chain(8990, Some(ckpt)).unwrap();
        let (h, _root) =
            verify_signed_manifest(&signed, 8990, &[pk], mode, None).unwrap();
        assert_eq!(h, 1_000_000);
    }

    #[test]
    fn stale_manifest_rejected_at_import_boundary() {
        // Authorised producer signed an old (height=999_000) manifest;
        // the syncing node's trusted checkpoint says we want
        // 1_000_000. The new freshness param MUST reject.
        let (signed, pk) = fixture_signed(999_000, [0x42; 32]);
        let fresh_ckpt =
            TrustedCheckpoint::for_test(1_000_000, H256([0xAB; 32]));
        let mode = ImportMode::for_live_chain(8990, Some(fresh_ckpt)).unwrap();
        let err = verify_signed_manifest(&signed, 8990, &[pk], mode, None)
            .unwrap_err();
        match err {
            SnapshotImportError::Verify(SnapshotError::CheckpointMismatch {
                got_height, exp_height, ..
            }) => {
                assert_eq!(got_height, 999_000);
                assert_eq!(exp_height, 1_000_000);
            }
            other => panic!("expected CheckpointMismatch, got {other:?}"),
        }
    }

    #[test]
    fn tooling_mode_skips_checkpoint_binding() {
        let (signed, pk) = fixture_signed(1, [0x11; 32]);
        let mode = ImportMode::unsafe_tooling_no_checkpoint();
        let (h, _) = verify_signed_manifest(&signed, 8990, &[pk], mode, None)
            .unwrap();
        assert_eq!(h, 1);
    }

    #[test]
    fn empty_allowed_producers_rejected() {
        let (signed, _) = fixture_signed(1, [0x11; 32]);
        let mode = ImportMode::unsafe_tooling_no_checkpoint();
        let err = verify_signed_manifest(&signed, 8990, &[], mode, None)
            .unwrap_err();
        assert!(matches!(err, SnapshotImportError::EmptyAllowedProducers));
    }

    #[test]
    fn maybe_import_returns_none_when_file_absent() {
        let dir = tempfile::tempdir().unwrap();
        let ckpt = Some(TrustedCheckpoint::for_test(1, H256([0; 32])));
        let r = maybe_import_snapshot(dir.path(), 8990, &[/* any */], ckpt);
        // Empty allowed_producers SHOULD be irrelevant when no file
        // exists: we early-return Ok(None) before validation.
        assert!(matches!(r, Ok(None)));
    }

    #[test]
    fn maybe_import_verifies_when_file_present() {
        let dir = tempfile::tempdir().unwrap();
        let (signed, pk) = fixture_signed(42, [0xCC; 32]);
        let bytes = bincode::serialize(&signed).unwrap();
        std::fs::write(dir.path().join(MANIFEST_FILENAME), bytes).unwrap();
        let ckpt = Some(TrustedCheckpoint::for_test(42, H256([0xCC; 32])));
        let (h, _) =
            maybe_import_snapshot(dir.path(), 8990, &[pk], ckpt).unwrap().unwrap();
        assert_eq!(h, 42);
    }

    #[test]
    fn maybe_import_rejects_stale_file_against_fresh_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let (signed, pk) = fixture_signed(100, [0xAA; 32]);
        std::fs::write(
            dir.path().join(MANIFEST_FILENAME),
            bincode::serialize(&signed).unwrap(),
        )
        .unwrap();
        let fresh = Some(TrustedCheckpoint::for_test(200, H256([0xBB; 32])));
        let err = maybe_import_snapshot(dir.path(), 8990, &[pk], fresh).unwrap_err();
        assert!(
            matches!(
                err,
                SnapshotImportError::Verify(SnapshotError::CheckpointMismatch { .. })
            ),
            "expected CheckpointMismatch, got {err:?}"
        );
    }

    /// Critical invariant test (addresses code-review TOCTOU finding):
    /// on a live chain id, a manifest file present in `data_dir`
    /// without a configured trusted checkpoint MUST fail at the
    /// import boundary — never silently fall through to tooling
    /// mode. Exercises both live chain ids the gate recognises.
    #[test]
    fn live_chain_with_manifest_but_no_checkpoint_is_fatal() {
        for chain_id in [
            zbx_types::CHAIN_ID_MAINNET,
            zbx_types::CHAIN_ID_TESTNET,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (signed, pk) = fixture_signed(7, [0x77; 32]);
            std::fs::write(
                dir.path().join(MANIFEST_FILENAME),
                bincode::serialize(&signed).unwrap(),
            )
            .unwrap();
            let err =
                maybe_import_snapshot(dir.path(), chain_id, &[pk], None)
                    .expect_err("must refuse to verify with None checkpoint on live chain");
            match err {
                SnapshotImportError::CheckpointRequired { chain_id: cid } => {
                    assert_eq!(cid, chain_id);
                }
                other => panic!(
                    "expected CheckpointRequired on chain {chain_id}, got {other:?}"
                ),
            }
        }
    }

    /// Conversely, an absent file on a live chain with no checkpoint
    /// is *not* an error — there is nothing to verify, and forcing
    /// every operator to pin a checkpoint just to boot a fresh node
    /// would be a usability regression.
    #[test]
    fn live_chain_no_manifest_no_checkpoint_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let r = maybe_import_snapshot(
            dir.path(),
            zbx_types::CHAIN_ID_MAINNET,
            &[],
            None,
        );
        assert!(matches!(r, Ok(None)));
    }

    /// `is_live_chain_id` is the single source of truth the gate
    /// inside `maybe_import_snapshot` keys off. If a future commit
    /// silently drops a chain id from this set, the gate would
    /// disappear without compiler help — pin the contract here.
    #[test]
    fn is_live_chain_id_recognises_known_networks() {
        assert!(is_live_chain_id(zbx_types::CHAIN_ID_MAINNET));
        assert!(is_live_chain_id(zbx_types::CHAIN_ID_TESTNET));
        assert!(!is_live_chain_id(1)); // ETH mainnet (not us)
        assert!(!is_live_chain_id(31337)); // anvil/foundry private
        assert!(!is_live_chain_id(0));
    }

    #[test]
    fn parse_checkpoint_hash_round_trip() {
        let h = parse_checkpoint_hash(
            "0x000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        )
        .unwrap();
        assert_eq!(h.0[0], 0x00);
        assert_eq!(h.0[31], 0x1f);
        // No 0x prefix also works.
        let h2 = parse_checkpoint_hash(
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        )
        .unwrap();
        assert_eq!(h, h2);
    }

    #[test]
    fn parse_checkpoint_hash_rejects_short() {
        assert!(parse_checkpoint_hash("0xdeadbeef").is_err());
    }

    #[test]
    fn parse_allowed_producer_round_trip() {
        let sk = BlsPrivKey::from_bytes(&[5u8; 32]).unwrap();
        let pk = sk.to_pubkey();
        let hex_pk = format!("0x{}", hex::encode(pk.as_bytes()));
        let parsed = parse_allowed_producer(0, &hex_pk).unwrap();
        assert_eq!(parsed.as_bytes(), pk.as_bytes());
    }
}
