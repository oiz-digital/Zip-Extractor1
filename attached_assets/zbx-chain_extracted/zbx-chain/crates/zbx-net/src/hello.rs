//! Hello / Status handshake protocol.
//!
//! After TCP connect and Noise handshake, both peers exchange StatusMsg.
//! Incompatible status (wrong chain ID, wrong fork) -> disconnect.
//!
//! Handshake sequence:
//!   Alice                           Bob
//!   TCP connect  ────────────────>
//!   Noise XX     <──────────────->
//!   StatusMsg    ────────────────>  StatusMsg
//!   StatusMsg    <────────────────  StatusMsg
//!   Compatible -> proceed to RLPx messages
//!   Else       -> Disconnect(UselessPeer)
//!
//! Compatibility rules (ZBX):
//!   1. genesis_hash must match
//!   2. chain_id must be 8989 (mainnet) or 8990 (testnet+devnet)
//!   3. protocol_version must be within SUPPORTED_VERSIONS
//!   4. fork_digest must match current fork (for ZEP upgrades)

use std::time::SystemTime;
use tracing::warn;

pub const SUPPORTED_VERSIONS: &[u64] = &[1, 2];

// Re-exported from `zbx-types` (single source of truth, locked-in 2026-05-01).
pub use zbx_types::{CHAIN_ID_MAINNET, CHAIN_ID_TESTNET};
use zbx_types::pinned_genesis::{
    pinned_for as pinned_genesis_for, is_sentinel, is_all_zero, PinPolicy,
};
use zbx_types::H256;

// ── Hello message (RLPx layer 0) ──────────────────────────────────────────────

/// RLPx Hello message -- first message after Noise handshake.
/// Establishes protocol capabilities before application messages.
#[derive(Debug, Clone)]
pub struct HelloMsg {
    /// Protocol version (RLPx version, currently 5)
    pub p2p_version:  u64,
    /// Client identifier e.g. "zbxctl/v0.1.0-alpha/linux-x86_64/rustc1.77"
    pub client_id:    String,
    /// Supported sub-protocols and versions
    pub capabilities: Vec<Capability>,
    /// TCP port this node listens on (0 if not listening)
    pub listen_port:  u16,
    /// Uncompressed secp256k1 public key (64 bytes, no prefix)
    pub node_id:      [u8; 64],
}

/// A sub-protocol capability (name + version).
/// ZBX supports: zbx/1, zbx/2, snap/1, wit/0
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    pub name:    String,
    pub version: u64,
}

impl Capability {
    pub fn zbx_v1()  -> Self { Self { name: "zbx".into(),  version: 1 } }
    pub fn zbx_v2()  -> Self { Self { name: "zbx".into(),  version: 2 } }
    pub fn snap_v1() -> Self { Self { name: "snap".into(), version: 1 } }
    pub fn wit_v0()  -> Self { Self { name: "wit".into(),  version: 0 } }
    pub fn zbx_defaults() -> Vec<Self> {
        vec![Self::zbx_v1(), Self::zbx_v2(), Self::snap_v1()]
    }
}

// ── Status message (zbx sub-protocol) ────────────────────────────────────────

/// StatusMsg -- ZBX sub-protocol handshake (sent after HelloMsg).
/// Both peers must agree on genesis hash and chain ID or disconnect.
#[derive(Debug, Clone)]
pub struct StatusMsg {
    /// Sub-protocol version (e.g. 2)
    pub protocol_version: u64,
    /// Chain ID -- must be 8989 (mainnet) or 8990 (testnet+devnet)
    pub chain_id:         u64,
    /// Total difficulty of the peer's best chain
    pub total_difficulty: u128,
    /// Hash of the peer's current best (head) block
    pub best_block:       [u8; 32],
    /// Hash of block 0 (genesis) -- used for network isolation
    pub genesis_hash:     [u8; 32],
    /// Fork digest -- 4-byte identifier for the active hard fork.
    /// Changes at each ZEP upgrade to prevent cross-fork connections.
    pub fork_digest:      [u8; 4],
    /// Unix timestamp when this status was created
    pub timestamp:        u64,
}

impl StatusMsg {
    pub fn new(
        chain_id:         u64,
        best_block:       [u8; 32],
        total_difficulty: u128,
        genesis_hash:     [u8; 32],
        fork_digest:      [u8; 4],
    ) -> Self {
        Self {
            protocol_version: SUPPORTED_VERSIONS[SUPPORTED_VERSIONS.len() - 1],
            chain_id, total_difficulty, best_block, genesis_hash, fork_digest,
            timestamp: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default().as_secs(),
        }
    }
}

// ── Handshake validator ───────────────────────────────────────────────────────

/// Validate a received StatusMsg against local chain state.
/// Returns Ok(()) if compatible, Err(DisconnectReason) if not.
pub fn validate_status(local: &StatusMsg, remote: &StatusMsg) -> Result<(), DisconnectReason> {
    if local.chain_id != remote.chain_id {
        return Err(DisconnectReason::WrongChainId { expected: local.chain_id, got: remote.chain_id });
    }
    if local.genesis_hash != remote.genesis_hash {
        return Err(DisconnectReason::GenesisHashMismatch);
    }
    if !SUPPORTED_VERSIONS.contains(&remote.protocol_version) {
        return Err(DisconnectReason::IncompatibleVersion(remote.protocol_version));
    }
    if local.fork_digest != remote.fork_digest {
        return Err(DisconnectReason::ForkDigestMismatch { local: local.fork_digest, remote: remote.fork_digest });
    }
    Ok(())
}

/// Reasons for disconnecting a peer (sent in a Disconnect message).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisconnectReason {
    Requested,
    TcpError,
    IncompatibleVersion(u64),
    GenesisHashMismatch,
    WrongChainId      { expected: u64, got: u64 },
    ForkDigestMismatch { local: [u8; 4], remote: [u8; 4] },
    TooManyPeers,
    UselessPeer,
    InvalidMessage,
    Timeout,
    Banned,
    MessageTooLarge,
    /// S30 — generic genesis-policy violation surfaced to the wire.
    ///
    /// Coarsened reason for ALL pinning-policy failures (sentinel,
    /// all-zero, unpinned local, unregistered chain) so peers cannot
    /// fingerprint our pinning state via differential disconnect
    /// reasons (S30 LOW-3 fix). Detailed sub-reason is logged via
    /// `tracing::warn!` for operator diagnosis but is NEVER sent on
    /// the wire.
    GenesisPolicyViolation,
}

// ── S30 pinning-aware handshake validator ────────────────────────────────────

/// S30 — Pinning-aware handshake validation.
///
/// Performs all checks of [`validate_status`] plus:
///   1. The local node's genesis hash MUST be the pinned canonical
///      hash for its chain_id (rejects sentinel/zero — `LocalGenesisUnpinned`).
///   2. The peer's genesis hash MUST be the pinned canonical hash
///      for the chain (rejects sentinel — `PeerGenesisSentinel`,
///      rejects all-zero — `PeerGenesisAllZero`).
///   3. The peer's `chain_id` MUST be registered in the pinning
///      registry (under `PinPolicy::Required`).
///
/// Under [`PinPolicy::AllowUnregistered`] (devnet), unregistered
/// chain_ids skip the pinning checks but still require chain_id /
/// genesis_hash equality between peers.
pub fn validate_status_pinned(
    local:  &StatusMsg,
    remote: &StatusMsg,
    policy: PinPolicy,
) -> Result<(), DisconnectReason> {
    // Standard checks first (chain_id, genesis equality, version, fork digest).
    validate_status(local, remote)?;

    let local_genesis  = H256(local.genesis_hash);
    let remote_genesis = H256(remote.genesis_hash);

    match (pinned_genesis_for(local.chain_id), policy) {
        (Some(pinned), _) => {
            // Known chain. Sentinel local → operator hasn't pinned.
            if is_sentinel(&pinned) {
                warn!(target: "zbx_net::hello", chain_id = local.chain_id,
                    "S30: local pinned constant is SENTINEL — operator must pin");
                return Err(DisconnectReason::GenesisPolicyViolation);
            }
            if local_genesis != pinned {
                // Local node's own genesis does not match the pinned
                // constant — extreme misconfiguration; refuse to peer.
                warn!(target: "zbx_net::hello", chain_id = local.chain_id,
                    "S30: local genesis does not match pinned constant");
                return Err(DisconnectReason::GenesisPolicyViolation);
            }
            if is_sentinel(&remote_genesis) {
                warn!(target: "zbx_net::hello", chain_id = local.chain_id,
                    "S30: peer reported SENTINEL genesis hash");
                return Err(DisconnectReason::GenesisPolicyViolation);
            }
            if is_all_zero(&remote_genesis) {
                warn!(target: "zbx_net::hello", chain_id = local.chain_id,
                    "S30: peer reported all-zero genesis hash");
                return Err(DisconnectReason::GenesisPolicyViolation);
            }
            // remote_genesis == local_genesis was already enforced by
            // validate_status; combined with local==pinned, peer is OK.
            Ok(())
        }
        (None, PinPolicy::AllowUnregistered) => {
            // Devnet — only the per-peer equality check applies.
            if is_all_zero(&remote_genesis) {
                warn!(target: "zbx_net::hello", chain_id = local.chain_id,
                    "S30: peer reported all-zero genesis hash (devnet)");
                return Err(DisconnectReason::GenesisPolicyViolation);
            }
            Ok(())
        }
        (None, PinPolicy::Required) => {
            warn!(target: "zbx_net::hello", chain_id = local.chain_id,
                "S30: chain_id not registered in pinning registry under Required policy");
            Err(DisconnectReason::GenesisPolicyViolation)
        }
    }
}
// ─── S30 tests — pinning-aware handshake ──────────────────────────────────────

#[cfg(test)]
mod s30_tests {
    use super::*;
    use zbx_types::pinned_genesis::SENTINEL_HASH;

    fn mk(chain_id: u64, genesis: [u8; 32]) -> StatusMsg {
        StatusMsg::new(chain_id, [0u8; 32], 0, genesis, [0u8; 4])
    }

    #[test]
    fn rejects_local_sentinel_on_mainnet_via_coarsened_reason() {
        // Local node believes its own genesis is the sentinel — operator
        // hasn't pinned. validate_status equality passes, but pinning
        // check rejects with the COARSENED wire reason (S30 LOW-3).
        let local  = mk(CHAIN_ID_MAINNET, SENTINEL_HASH.0);
        let remote = mk(CHAIN_ID_MAINNET, SENTINEL_HASH.0);
        let err = validate_status_pinned(&local, &remote, PinPolicy::Required)
            .unwrap_err();
        assert_eq!(err, DisconnectReason::GenesisPolicyViolation);
    }

    #[test]
    fn rejects_unknown_chain_id_under_required_policy_via_coarsened_reason() {
        // chain_id mismatch is caught by validate_status BEFORE pinning logic
        // (chain_id is the first equality check). Use matching chain_ids on
        // both sides so the inner check passes and we exercise pinning.
        let local  = mk(31337, [0x42u8; 32]);
        let remote = mk(31337, [0x42u8; 32]);
        let err = validate_status_pinned(&local, &remote, PinPolicy::Required)
            .unwrap_err();
        assert_eq!(err, DisconnectReason::GenesisPolicyViolation);
    }

    #[test]
    fn devnet_path_accepts_unregistered_with_matching_genesis() {
        let local  = mk(31337, [0x42u8; 32]);
        let remote = mk(31337, [0x42u8; 32]);
        validate_status_pinned(&local, &remote, PinPolicy::AllowUnregistered)
            .expect("devnet path must accept matching unregistered chain");
    }

    #[test]
    fn devnet_path_rejects_all_zero_remote_genesis_via_coarsened_reason() {
        let local  = mk(31337, [0u8; 32]);
        let remote = mk(31337, [0u8; 32]);
        let err = validate_status_pinned(&local, &remote, PinPolicy::AllowUnregistered)
            .unwrap_err();
        assert_eq!(err, DisconnectReason::GenesisPolicyViolation);
    }

    #[test]
    fn standard_chain_id_mismatch_still_caught_first() {
        // chain_id divergence between PEERS is caught by inner validate_status
        // BEFORE pinning logic runs (and reports WrongChainId, not coarsened).
        let local  = mk(CHAIN_ID_MAINNET, SENTINEL_HASH.0);
        let remote = mk(CHAIN_ID_TESTNET, SENTINEL_HASH.0);
        let err = validate_status_pinned(&local, &remote, PinPolicy::Required)
            .unwrap_err();
        assert!(matches!(err, DisconnectReason::WrongChainId { .. }));
    }

    #[test]
    fn standard_genesis_equality_caught_before_pinning() {
        // local and remote have different genesis (validate_status rejects
        // with GenesisHashMismatch, not the coarsened pinning reason).
        let local  = mk(CHAIN_ID_MAINNET, SENTINEL_HASH.0);
        let remote = mk(CHAIN_ID_MAINNET, [0x99u8; 32]);
        let err = validate_status_pinned(&local, &remote, PinPolicy::Required)
            .unwrap_err();
        assert_eq!(err, DisconnectReason::GenesisHashMismatch);
    }

    #[test]
    fn s30_all_pinning_failures_report_same_wire_reason_no_fingerprinting() {
        // S30 LOW-3 fix verification: every pinning-policy failure must
        // surface as the SAME wire reason (GenesisPolicyViolation) so a
        // probe peer cannot fingerprint our pinning state. Cases:
        //   - sentinel local
        //   - sentinel remote (both sides match because validate_status
        //     equality requires it; only meaningful once operator pins)
        //   - unregistered chain under Required policy
        //   - all-zero remote in devnet
        let cases: Vec<(StatusMsg, StatusMsg, PinPolicy)> = vec![
            (mk(CHAIN_ID_MAINNET, SENTINEL_HASH.0), mk(CHAIN_ID_MAINNET, SENTINEL_HASH.0), PinPolicy::Required),
            (mk(31337, [0x42u8; 32]), mk(31337, [0x42u8; 32]), PinPolicy::Required),
            (mk(31337, [0u8; 32]),     mk(31337, [0u8; 32]),    PinPolicy::AllowUnregistered),
        ];
        for (l, r, pol) in cases {
            let err = validate_status_pinned(&l, &r, pol).unwrap_err();
            assert_eq!(err, DisconnectReason::GenesisPolicyViolation,
                "all pinning-policy failures must coarsen to GenesisPolicyViolation");
        }
    }
}
