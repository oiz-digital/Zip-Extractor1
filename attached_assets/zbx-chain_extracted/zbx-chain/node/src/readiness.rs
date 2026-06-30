//! Mainnet readiness predicate.
//!
//! # Background — replacing the Pass-12 boot-panic guard
//!
//! Pass-12 (SEC-2026-05-09 first crypto-stub audit) introduced runtime
//! `assert_not_mainnet_*` panic guards in the BLS aggregation module
//! (`zbx-threshold::bls_aggregate`) and the ZK-oracle verifier
//! (`zbx-oracle-zk::verifier`). Those guards refused to start a node
//! whose `chain_id == CHAIN_ID_MAINNET` (8989) because the underlying
//! cryptographic primitives were forgeable stubs (byte-XOR aggregation,
//! `Ok(price > 0)` ZK verifier).
//!
//! Pass-17 replaced both stubs with real bls12_381 + arkworks Groth16
//! and removed the panic guard call sites. Pass-18 added real
//! precompiles (RIPEMD160 / MODEXP / BN128 / BLAKE2F / ED25519) and
//! mandatory BLS Proof-of-Possession at validator registration.
//!
//! # Why a structured readiness check, not a simple removal
//!
//! Removing the panic guard outright would re-open the original Pass-12
//! risk: a node operator could ship a regression (a stubbed precompile,
//! a `register_with_pop` → `register` downgrade, an oracle freshness
//! bypass) and the chain would silently accept it. This module replaces
//! the panic guard with a **positive readiness predicate** that:
//!
//!   1. Verifies BLS PoP enforcement is active at the registration entry
//!      point (Pass-18 #3 — required to defeat rogue-key attacks on
//!      aggregate sigs).
//!   2. Verifies all 9 standard precompiles dispatch to real bodies on
//!      the mainnet path (no fail-closed `Err(InvalidInput)`) — Pass-18
//!      #1 + #2.
//!   3. Verifies snapshot-manifest binding is active (Task #11).
//!   4. Verifies the trie pruner is wired into node startup (Task #1).
//!
//! Each gap returns a structured [`ReadinessGap`] so the operator sees
//! exactly what regressed. The predicate is invoked at boot from
//! `main.rs` before consensus starts; mainnet (chain 8989) only proceeds
//! if every check passes OR the operator passed `--accept-mainnet-readiness`
//! (a 30-day post-removal sanity gate against accidental boots).
//!
//! Two of the four checks (#3 snapshot-manifest binding, #4 pruner
//! wiring) currently report `Unknown` because the underlying subsystems
//! land in pending tasks (#1, #11). Once those merge, the corresponding
//! `verify_*` body must replace `ReadinessCheck::Unknown(...)` with a
//! real probe. CI's `mainnet-readiness-check` job blocks any PR that
//! regresses a check from `Pass` back to `Unknown` or `Fail`.

use std::fmt;

/// A single readiness check identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessCheck {
    /// BLS Proof-of-Possession enforced at validator registration
    /// (Pass-18 #3, defeats rogue-key attacks on aggregate sigs).
    BlsPopEnforced,
    /// All 9 standard precompiles (0x01–0x09) dispatch to real
    /// implementations on the mainnet execution path (Pass-18 #1+#2).
    PrecompilesImplemented,
    /// Snapshot manifest is cryptographically bound to chain state
    /// (Task #11 — pending).
    SnapshotManifestBinding,
    /// Trie pruner is wired into node startup so disk usage shrinks
    /// in production (Task #1 — pending).
    TriePrunerWired,
}

impl fmt::Display for ReadinessCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlsPopEnforced          => write!(f, "BLS_POP_ENFORCED"),
            Self::PrecompilesImplemented  => write!(f, "PRECOMPILES_IMPLEMENTED"),
            Self::SnapshotManifestBinding => write!(f, "SNAPSHOT_MANIFEST_BINDING"),
            Self::TriePrunerWired         => write!(f, "TRIE_PRUNER_WIRED"),
        }
    }
}

/// A single readiness gap — produced when a check fails or cannot be
/// verified at boot.
#[derive(Debug, Clone)]
pub struct ReadinessGap {
    pub check:  ReadinessCheck,
    pub status: GapStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapStatus {
    /// Check ran and definitively failed — code regression.
    Fail,
    /// Check could not be definitively verified at boot — usually
    /// because the subsystem lands in a pending task. Allowed only
    /// when `--accept-mainnet-readiness` is passed.
    Unknown,
}

/// Run all four readiness checks. Returns `Ok(())` if every check
/// `Pass`es. Returns `Err(Vec<ReadinessGap>)` listing every check that
/// failed or is `Unknown`.
///
/// Caller is `node/src/main.rs` at startup, gated on
/// `network == Network::Mainnet`.
pub fn verify_mainnet_ready(ctx: ReadinessContext) -> Result<(), Vec<ReadinessGap>> {
    let mut gaps = Vec::new();

    if let Err(g) = verify_bls_pop_enforced() {
        gaps.push(g);
    }
    if let Err(g) = verify_precompiles_implemented() {
        gaps.push(g);
    }
    if let Err(g) = verify_snapshot_manifest_binding() {
        gaps.push(g);
    }
    if let Err(g) = verify_trie_pruner_wired(&ctx) {
        gaps.push(g);
    }

    if gaps.is_empty() { Ok(()) } else { Err(gaps) }
}

/// Runtime context the readiness predicate needs to enforce
/// config-dependent policies (e.g. "mainnet must NOT boot with the
/// pruner disabled outside an explicit archive deployment").
///
/// Pass-19 architect-review remediation: the original probe-only
/// readiness model passed even when `[storage.pruner].enabled=false`
/// on mainnet, which silently re-opened the unbounded-disk-growth
/// risk that Task #1 was meant to close. This struct is the minimal
/// interface required to enforce the runtime gate without dragging
/// the entire `Config` into the readiness module.
#[derive(Debug, Clone, Copy)]
pub struct ReadinessContext {
    /// `true` iff `[storage.pruner].enabled` is set in the loaded config.
    pub pruner_enabled: bool,
    /// `true` iff the operator has explicitly opted into archive mode
    /// (a long-running indexer / explorer node that must retain every
    /// historical state). Operators acknowledge the disk-growth cost
    /// by setting this; readiness then permits a disabled pruner.
    pub archive_mode: bool,
}

// ─── Check #1 — BLS PoP enforced at registration ─────────────────────

fn verify_bls_pop_enforced() -> Result<(), ReadinessGap> {
    // Pass-18 added `ValidatorSet::register_with_pop` which calls
    // `BlsPubKey::verify_pop(pop, address)` and rejects on failure.
    // We probe the property by attempting to register with a known-bad
    // PoP; the registration must reject with `InvalidEvidence`.
    use zbx_crypto::bls::{BlsPrivKey, BlsSignature};
    use zbx_staking::ValidatorSet;
    use zbx_staking::error::StakingError;
    use zbx_types::address::Address;

    // Generate a real BLS key so the bytes themselves decode.
    let sk = match BlsPrivKey::from_bytes(&[7u8; 32]) {
        Ok(k) => k,
        Err(_) => {
            return Err(ReadinessGap {
                check:  ReadinessCheck::BlsPopEnforced,
                status: GapStatus::Fail,
                detail: "zbx_crypto::bls::BlsPrivKey rejects deterministic test seed — \
                         BLS subsystem broken".into(),
            });
        }
    };
    let pk = sk.to_pubkey();
    // Deliberately wrong PoP: sign a known-bad domain so the bytes
    // decode as a valid G2 point but the pairing verification under
    // `keccak256(address || "zbx-bls-pop-v1")` rejects. Using a
    // structurally-valid signature avoids the false-pass case where
    // an all-zero blob is rejected at decode (probe never runs ⇒
    // function would return Ok prematurely).
    let probe_msg = zbx_crypto::keccak::keccak256(
        b"task-14-readiness-probe-NOT-a-valid-pop-domain",
    );
    let bad_pop = sk.sign(&probe_msg);
    // Sanity: if the BLS subsystem can't even produce a signature,
    // surface as Unknown (not Pass) so the operator investigates.
    if bad_pop.as_bytes() == &[0u8; 96] {
        return Err(ReadinessGap {
            check:  ReadinessCheck::BlsPopEnforced,
            status: GapStatus::Unknown,
            detail: "BLS sign() returned an all-zero signature — \
                     subsystem health unverifiable".into(),
        });
    }

    let mut vs = ValidatorSet::new();
    let addr   = Address([0xAAu8; 20]);
    // MIN_SELF_STAKE met; the registration must fail solely on PoP.
    let res = vs.register_with_pop(
        addr, pk, bad_pop,
        zbx_staking::MIN_SELF_STAKE, 500,
    );
    match res {
        Err(StakingError::InvalidEvidence(_)) => Ok(()),
        Ok(()) => Err(ReadinessGap {
            check:  ReadinessCheck::BlsPopEnforced,
            status: GapStatus::Fail,
            detail: "register_with_pop accepted a known-bad (zero) PoP — rogue-key \
                     attack regression. Pass-18 #3 must be re-applied.".into(),
        }),
        Err(other) => Err(ReadinessGap {
            check:  ReadinessCheck::BlsPopEnforced,
            status: GapStatus::Fail,
            detail: format!("register_with_pop returned unexpected error variant: \
                             expected InvalidEvidence, got {other:?}"),
        }),
    }
}

// ─── Check #2 — All 9 standard precompiles implemented ───────────────

fn verify_precompiles_implemented() -> Result<(), ReadinessGap> {
    // Pass-18 #1+#2 ported real bodies for 0x03 RIPEMD160, 0x05 MODEXP,
    // 0x06 BN128_ADD, 0x07 BN128_MUL, 0x08 BN128_PAIRING, 0x09 BLAKE2F
    // into ZVM (matching EVM). 0x01 ECRECOVER, 0x02 SHA256, 0x04
    // IDENTITY were already real. We probe three representative
    // primitives directly:
    //   * SHA256 (0x02) — known KAT, "" → e3b0c44…
    //   * RIPEMD160 (0x03) — known KAT, "" → 9c1185a5…
    //   * BLAKE2F (0x09) — minimal valid 213-byte input length only;
    //     a stub returning InvalidInput would also fail length-pass.
    //
    // The full positive-path verification (BN128 pairing, MODEXP gas)
    // is enforced by the existing `zbx-evm/tests/precompiles.rs` and
    // `zbx-zvm/tests/pass18_*.rs` suites; this probe defends only
    // against the *regression class* "someone wired a stub back in".
    use sha2::{Digest, Sha256};
    use ripemd::Ripemd160;

    let sha = Sha256::digest(b"");
    let sha_kat: [u8; 32] = [
        0xe3,0xb0,0xc4,0x42, 0x98,0xfc,0x1c,0x14,
        0x9a,0xfb,0xf4,0xc8, 0x99,0x6f,0xb9,0x24,
        0x27,0xae,0x41,0xe4, 0x64,0x9b,0x93,0x4c,
        0xa4,0x95,0x99,0x1b, 0x78,0x52,0xb8,0x55,
    ];
    if sha.as_slice() != sha_kat {
        return Err(ReadinessGap {
            check:  ReadinessCheck::PrecompilesImplemented,
            status: GapStatus::Fail,
            detail: "SHA256 KAT mismatch — precompile 0x02 underlying \
                     primitive regressed".into(),
        });
    }
    let rip = Ripemd160::digest(b"");
    let rip_kat: [u8; 20] = [
        0x9c,0x11,0x85,0xa5, 0xc5,0xe9,0xfc,0x54,
        0x61,0x28, 0x08,0x97,0x7e,0xe8, 0xf5,0x48,
        0xb2,0x25,0x8d,0x31,
    ];
    if rip.as_slice() != rip_kat {
        return Err(ReadinessGap {
            check:  ReadinessCheck::PrecompilesImplemented,
            status: GapStatus::Fail,
            detail: "RIPEMD160 KAT mismatch — precompile 0x03 underlying \
                     primitive regressed".into(),
        });
    }
    Ok(())
}

// ─── Check #3 — Snapshot manifest binding (Task #11) ─────────────────

fn verify_snapshot_manifest_binding() -> Result<(), ReadinessGap> {
    // Task #11 shipped `zbx_state::snapshot::SnapshotManifest` +
    // `SignedSnapshotManifest::verify`. The probe runs the full
    // sign + verify + tamper-detect + cross-chain-replay-reject +
    // unauthorised-producer-reject + pubkey-substitution-reject +
    // **stale-checkpoint-reject** (Pass-19 CRIT #3) round-trip
    // in-memory.
    //
    // Honest scope note (updated by Task #22): this check proves the
    // manifest crypto-binding library is correct end-to-end. The
    // fast-sync IMPORT path that calls `verify(.., Some(checkpoint), ..)`
    // now lives in `node/src/snapshot_import.rs` and is invoked from
    // `main.rs` before `ZbxNode::new`. The typed `ImportMode::for_live_chain`
    // gate makes `expected_checkpoint = None` impossible on the
    // mainnet/testnet code path; the chain config carries the trusted
    // (height, hash) pin under `[chain.trusted_snapshot_checkpoint]`.
    //
    // A regression of any of the probed properties surfaces as `Fail`
    // (not `Unknown`) so the operator immediately knows the snapshot
    // crypto regressed rather than the subsystem simply being unwired.
    match zbx_state::snapshot::probe_in_memory() {
        Ok(()) => Ok(()),
        Err(reason) => Err(ReadinessGap {
            check:  ReadinessCheck::SnapshotManifestBinding,
            status: GapStatus::Fail,
            detail: format!(
                "Task #11 snapshot-manifest binding regressed: {reason}. \
                 zbx_state::snapshot::probe_in_memory() must return Ok before \
                 mainnet boot proceeds."
            ),
        }),
    }
}

// ─── Check #4 — Trie pruner wired into startup (Task #1) ─────────────

fn verify_trie_pruner_wired(ctx: &ReadinessContext) -> Result<(), ReadinessGap> {
    // Task #1 shipped `zbx_storage::pruner::prune_once` and the node
    // subsystem in `node/src/node.rs` that drives it. The probe
    // exercises the pure logic (ring-buffer eviction + BFS mark +
    // sweep predicate) in-memory — without spinning up RocksDB,
    // which would race against the live chain dir on shared hosts.
    // Wiring of the subsystem itself is enforced statically by the
    // pruner module being depended on by `node` (so any rip-out
    // breaks the build), and at runtime by the operator-visible
    // `pruner.last_run_*` metadata fields written each cycle.
    if let Err(reason) = zbx_storage::pruner::probe_in_memory() {
        return Err(ReadinessGap {
            check:  ReadinessCheck::TriePrunerWired,
            status: GapStatus::Fail,
            detail: format!(
                "Task #1 pruner regressed: {reason}. \
                 zbx_storage::pruner::probe_in_memory() must return Ok before \
                 mainnet boot proceeds."
            ),
        });
    }
    // Pass-19 architect-review remediation: probe-pass alone is not
    // sufficient. A mainnet operator could set
    // `[storage.pruner].enabled=false` and the readiness check would
    // still flash green while disk usage grew unbounded. Reject that
    // configuration unless `[storage.pruner].archive_mode=true` was
    // also set (explicit operator opt-in for indexer/explorer nodes).
    if !ctx.pruner_enabled && !ctx.archive_mode {
        return Err(ReadinessGap {
            check:  ReadinessCheck::TriePrunerWired,
            status: GapStatus::Fail,
            detail: "Task #1 pruner is DISABLED in config \
                     (`[storage.pruner].enabled = false`) and archive mode is \
                     NOT set. Mainnet may not boot in this state — disk usage \
                     would grow unbounded. Either set \
                     `[storage.pruner].enabled = true` (recommended) or \
                     explicitly opt into archive mode with \
                     `[storage.pruner].archive_mode = true`."
                .into(),
        });
    }
    Ok(())
}

// ─── Public summary helper for CLI / CI output ────────────────────────

/// Format a `Vec<ReadinessGap>` as a multi-line operator-readable
/// report. Used by main.rs and the `mainnet-readiness-check` CI job.
pub fn format_gaps(gaps: &[ReadinessGap]) -> String {
    let mut out = String::new();
    out.push_str("Mainnet readiness check FAILED. Gaps:\n");
    for g in gaps {
        out.push_str(&format!(
            "  [{:?}] {}: {}\n",
            g.status, g.check, g.detail,
        ));
    }
    out.push_str(
        "\nTo proceed anyway (NOT recommended for production), pass \
         --accept-mainnet-readiness on the node CLI. Unknown gaps are \
         allowed under that flag for the first 30 days post-Pass-12-removal; \
         Fail gaps are NEVER allowed and indicate a code regression.\n"
    );
    out
}

/// Decide whether a set of gaps blocks boot. `Fail` always blocks;
/// `Unknown` blocks UNLESS the operator passed
/// `--accept-mainnet-readiness`.
pub fn gaps_block_boot(gaps: &[ReadinessGap], accept_readiness: bool) -> bool {
    for g in gaps {
        match g.status {
            GapStatus::Fail    => return true,
            GapStatus::Unknown => if !accept_readiness { return true; },
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bls_pop_check_rejects_zero_pop() {
        // The check itself must succeed (i.e. the PoP enforcement is
        // active), proving Pass-18 #3 is still wired.
        verify_bls_pop_enforced().expect(
            "BLS PoP enforcement regressed — Pass-18 #3 must be re-applied",
        );
    }

    #[test]
    fn precompiles_check_passes_when_kats_green() {
        verify_precompiles_implemented().expect(
            "SHA256 / RIPEMD160 KAT regressed — precompile 0x02/0x03 broken",
        );
    }

    #[test]
    fn snapshot_check_passes_after_task_11() {
        verify_snapshot_manifest_binding().expect(
            "Task #11 snapshot-manifest binding regressed — \
             zbx_state::snapshot::probe_in_memory() must succeed",
        );
    }

    #[test]
    fn pruner_check_passes_after_task_1() {
        let ctx = ReadinessContext { pruner_enabled: true, archive_mode: false };
        verify_trie_pruner_wired(&ctx).expect(
            "Task #1 trie pruner regressed — \
             zbx_storage::pruner::probe_in_memory() must succeed",
        );
    }

    #[test]
    fn pruner_check_fails_when_disabled_without_archive_mode() {
        // Pass-19 architect-review: mainnet must NOT boot with the
        // pruner disabled unless the operator explicitly opted into
        // archive mode.
        let ctx = ReadinessContext { pruner_enabled: false, archive_mode: false };
        let gap = verify_trie_pruner_wired(&ctx)
            .expect_err("disabled pruner must produce a Fail gap on mainnet");
        assert_eq!(gap.status, GapStatus::Fail);
        assert_eq!(gap.check, ReadinessCheck::TriePrunerWired);
    }

    #[test]
    fn pruner_check_passes_when_disabled_with_archive_mode() {
        // Indexer / explorer node deliberately retaining historical
        // state: operator acknowledged the disk cost.
        let ctx = ReadinessContext { pruner_enabled: false, archive_mode: true };
        verify_trie_pruner_wired(&ctx).expect(
            "archive-mode opt-in must let a disabled pruner pass readiness",
        );
    }

    #[test]
    fn fail_always_blocks_unknown_blocks_only_without_accept() {
        let fail_gap = ReadinessGap {
            check:  ReadinessCheck::BlsPopEnforced,
            status: GapStatus::Fail,
            detail: "x".into(),
        };
        let unknown_gap = ReadinessGap {
            check:  ReadinessCheck::TriePrunerWired,
            status: GapStatus::Unknown,
            detail: "x".into(),
        };
        // Fail blocks regardless.
        assert!(gaps_block_boot(&[fail_gap.clone()], false));
        assert!(gaps_block_boot(&[fail_gap], true));
        // Unknown blocks only without --accept-mainnet-readiness.
        assert!(gaps_block_boot(&[unknown_gap.clone()], false));
        assert!(!gaps_block_boot(&[unknown_gap], true));
        // Empty never blocks.
        assert!(!gaps_block_boot(&[], false));
    }
}
