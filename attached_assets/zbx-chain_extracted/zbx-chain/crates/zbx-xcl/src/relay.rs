//! Permissionless XCL relayer.
//!
//! Any full node can act as a relayer — there is no trusted-relayer role,
//! no multisig key, no special permission. The relayer merely:
//!
//!   1. Observes packet events on chain A.
//!   2. Fetches the Merkle proof of the commitment from chain A.
//!   3. Updates the local foreign-client header on chain B.
//!   4. Submits the proof + header to chain B's `recv_packet` handler.
//!   5. Observes the ack on chain B and relays it back to chain A.
//!
//! If a relayer misbehaves (submits a bad proof), the verification in
//! `recv_packet` rejects it — no harm done. Correct relayers are incentivised
//! by a small fee taken from the packet (configurable per channel).

use crate::{
    channel::Channel,
    client::ForeignHeader,
    packet::XclPacket,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// A query to fetch a packet commitment proof from a remote node.
///
/// Real implementations use the ZBX JSON-RPC method `zbx_xcl_getCommitmentProof`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitmentProofQuery {
    pub channel:  [u8; 32],
    pub sequence: u64,
    pub height:   u64,
}

/// Response: the proof data needed to call `recv_packet`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitmentProof {
    /// The foreign header at `proof_height` (including QC for light-client update).
    pub header:      ForeignHeader,
    /// The absolute block height at which the proof was generated.
    pub proof_height: u64,
    /// Ordered RLP-encoded MPT nodes (root → leaf) for `commitment_key`.
    pub proof_nodes: Vec<Vec<u8>>,
}

/// A query to fetch a packet receipt / ack proof from a remote node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckProofQuery {
    pub channel:  [u8; 32],
    pub sequence: u64,
    pub height:   u64,
}

/// Response: ack proof for the send-side chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckProof {
    pub header:      ForeignHeader,
    pub proof_height: u64,
    pub proof_nodes: Vec<Vec<u8>>,
    /// The raw ack bytes (for the on-chain ack handler to verify).
    pub ack_bytes:   Vec<u8>,
}

/// Metadata about a pending relay job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayJob {
    pub packet:       XclPacket,
    pub relay_stage:  RelayStage,
    pub retry_count:  u32,
    pub max_retries:  u32,
    pub first_seen:   u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelayStage {
    /// Waiting to relay the commitment proof from src → dst.
    PendingRecv,
    /// recv delivered; waiting to relay ack from dst → src.
    PendingAck,
    /// Timed out; will relay timeout proof src → src.
    PendingTimeout,
    /// Complete.
    Done,
}

/// Permissionless relayer state — tracks pending jobs across both directions.
pub struct XclRelayer {
    pub local_chain_id: u64,
    pending:            Vec<RelayJob>,
    max_retries:        u32,
}

impl XclRelayer {
    pub fn new(local_chain_id: u64) -> Self {
        XclRelayer {
            local_chain_id,
            pending:     Vec::new(),
            max_retries: 5,
        }
    }

    /// Enqueue a new packet for relaying (called when a `PacketSent` event is observed).
    pub fn enqueue(&mut self, packet: XclPacket, current_height: u64) {
        info!(
            src      = packet.src_chain_id,
            dst      = packet.dst_chain_id,
            channel  = %hex::encode(packet.src_channel),
            sequence = packet.sequence,
            "xcl-relayer: queued packet"
        );
        self.pending.push(RelayJob {
            packet,
            relay_stage: RelayStage::PendingRecv,
            retry_count: 0,
            max_retries: self.max_retries,
            first_seen:  current_height,
        });
    }

    /// Advance relay stage after successful recv delivery.
    pub fn on_recv_delivered(&mut self, channel: [u8; 32], sequence: u64) {
        for job in &mut self.pending {
            if job.packet.src_channel == channel && job.packet.sequence == sequence {
                job.relay_stage = RelayStage::PendingAck;
                debug!(sequence, "xcl-relayer: recv delivered, awaiting ack");
                return;
            }
        }
    }

    /// Advance relay stage after ack delivery — remove from pending.
    pub fn on_ack_delivered(&mut self, channel: [u8; 32], sequence: u64) {
        self.pending.retain(|job| {
            !(job.packet.src_channel == channel && job.packet.sequence == sequence)
        });
        info!(sequence, "xcl-relayer: ack delivered — relay complete");
    }

    /// Mark a packet as timed-out so we switch to timeout relay path.
    pub fn on_timeout(&mut self, channel: [u8; 32], sequence: u64) {
        for job in &mut self.pending {
            if job.packet.src_channel == channel && job.packet.sequence == sequence {
                job.relay_stage = RelayStage::PendingTimeout;
                warn!(sequence, "xcl-relayer: packet timed out");
                return;
            }
        }
    }

    /// Increment retry counter. Returns `false` if max retries exceeded (drop job).
    pub fn record_retry(&mut self, channel: [u8; 32], sequence: u64) -> bool {
        for job in &mut self.pending {
            if job.packet.src_channel == channel && job.packet.sequence == sequence {
                job.retry_count += 1;
                if job.retry_count > job.max_retries {
                    warn!(sequence, retries = job.retry_count, "xcl-relayer: dropping job (max retries)");
                    return false;
                }
                return true;
            }
        }
        false
    }

    /// Returns all pending recv jobs for a given destination chain.
    pub fn pending_recv(&self, dst_chain_id: u64) -> Vec<&RelayJob> {
        self.pending.iter()
            .filter(|j| j.packet.dst_chain_id == dst_chain_id && j.relay_stage == RelayStage::PendingRecv)
            .collect()
    }

    /// Returns all pending ack jobs for a given source chain.
    pub fn pending_ack(&self, src_chain_id: u64) -> Vec<&RelayJob> {
        self.pending.iter()
            .filter(|j| j.packet.src_chain_id == src_chain_id && j.relay_stage == RelayStage::PendingAck)
            .collect()
    }

    /// Summary of relay queue state.
    pub fn stats(&self) -> RelayStats {
        let mut stats = RelayStats::default();
        for job in &self.pending {
            match job.relay_stage {
                RelayStage::PendingRecv    => stats.pending_recv    += 1,
                RelayStage::PendingAck     => stats.pending_ack     += 1,
                RelayStage::PendingTimeout => stats.pending_timeout += 1,
                RelayStage::Done           => {}
            }
        }
        stats
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RelayStats {
    pub pending_recv:    usize,
    pub pending_ack:     usize,
    pub pending_timeout: usize,
}
