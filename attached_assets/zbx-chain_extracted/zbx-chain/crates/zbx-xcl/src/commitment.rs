//! On-chain commitment and receipt store.
//!
//! This is the "XCL state" embedded in each chain's state trie.
//! It maps packet keys → commitment hashes so that counterparty light clients
//! can verify packet existence without trusting any intermediary.
//!
//! ## Storage layout
//!
//! | Key                              | Value                     | Written by |
//! |----------------------------------|---------------------------|------------|
//! | `xcl/commitment/{ch}/{seq}`      | keccak256(packet_bytes)   | send       |
//! | `xcl/receipt/{ch}/{seq}`         | 0x01                      | recv       |
//! | `xcl/ack/{ch}/{seq}`             | keccak256(ack_bytes)      | recv       |
//! | `xcl/escrow/{ch}`                | u128 BE (total escrowed)  | send/refund|
//! | `xcl/client/{client_id}/height`  | u64 BE                    | update     |

use crate::error::XclError;
use crate::packet::{ChannelId, XclPacket, PacketAck, ack_key, commitment_key, receipt_key};
use sha3::{Digest, Keccak256};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use zbx_types::H256;

/// A pending (unacknowledged) send.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingSend {
    pub packet:         XclPacket,
    pub commitment:     H256,
    pub escrowed_amount: u128,
    pub sent_height:    u64,
}

/// The local XCL state store (in-memory; written to the state trie at block end).
#[derive(Debug, Default)]
pub struct CommitmentStore {
    /// packet commitments written on send: (channel, seq) → commitment hash
    commitments: HashMap<(ChannelId, u64), H256>,
    /// packet receipts written on recv: (channel, seq) → true
    receipts:    HashMap<(ChannelId, u64), bool>,
    /// packet acks written on recv-side after processing: (channel, seq) → ack hash
    acks:        HashMap<(ChannelId, u64), H256>,
    /// Escrowed native ZBX per channel (in wei). Increases on send, decreases on recv/timeout.
    escrow:      HashMap<ChannelId, u128>,
    /// Pending sends awaiting acknowledgement.
    pending:     HashMap<(ChannelId, u64), PendingSend>,
}

impl CommitmentStore {
    pub fn new() -> Self {
        Self::default()
    }

    // ── Send-side operations ─────────────────────────────────────────────

    /// Record a packet commitment on send. Escrowed amount is tracked.
    pub fn set_commitment(
        &mut self,
        packet:   &XclPacket,
        escrowed: u128,
        sent_at:  u64,
    ) {
        let commitment = packet.commitment();
        let key = (packet.src_channel, packet.sequence);
        self.commitments.insert(key, commitment);
        self.pending.insert(key, PendingSend {
            packet:          packet.clone(),
            commitment,
            escrowed_amount: escrowed,
            sent_height:     sent_at,
        });
        *self.escrow.entry(packet.src_channel).or_insert(0) += escrowed;
    }

    /// Verify that a commitment exists and matches (called by counterparty relayer path).
    pub fn verify_commitment(&self, channel: &ChannelId, seq: u64, expected: &H256) -> bool {
        self.commitments.get(&(*channel, seq)).map_or(false, |c| c == expected)
    }

    /// Remove commitment after ack or timeout.
    pub fn delete_commitment(&mut self, channel: &ChannelId, seq: u64) -> Option<PendingSend> {
        self.commitments.remove(&(*channel, seq));
        let pending = self.pending.remove(&(*channel, seq));
        if let Some(ref p) = pending {
            let escrow = self.escrow.entry(*channel).or_insert(0);
            *escrow = escrow.saturating_sub(p.escrowed_amount);
        }
        pending
    }

    /// Retrieve a pending send.
    pub fn get_pending(&self, channel: &ChannelId, seq: u64) -> Option<&PendingSend> {
        self.pending.get(&(*channel, seq))
    }

    pub fn has_commitment(&self, channel: &ChannelId, seq: u64) -> bool {
        self.commitments.contains_key(&(*channel, seq))
    }

    // ── Recv-side operations ─────────────────────────────────────────────

    /// Check if a receipt already exists (idempotency guard).
    pub fn has_receipt(&self, channel: &ChannelId, seq: u64) -> bool {
        self.receipts.get(&(*channel, seq)).copied().unwrap_or(false)
    }

    /// Record a packet receipt. Returns error if already received (replay protection).
    pub fn set_receipt(&mut self, channel: &ChannelId, seq: u64) -> Result<(), XclError> {
        let key = (*channel, seq);
        if self.receipts.get(&key).copied().unwrap_or(false) {
            return Err(XclError::PacketAlreadyReceived(hex::encode(channel), seq));
        }
        self.receipts.insert(key, true);
        Ok(())
    }

    /// Store ack hash (written on destination chain after processing the recv).
    pub fn set_ack(
        &mut self,
        channel: &ChannelId,
        seq:     u64,
        ack:     &PacketAck,
    ) -> Result<(), XclError> {
        let key = (*channel, seq);
        if self.acks.contains_key(&key) {
            return Err(XclError::AlreadyAcknowledged(hex::encode(channel), seq));
        }
        let ack_bytes = ack.encode();
        let hash = Keccak256::digest(&ack_bytes);
        let mut out = [0u8; 32];
        out.copy_from_slice(&hash);
        self.acks.insert(key, H256(out));
        Ok(())
    }

    pub fn get_ack(&self, channel: &ChannelId, seq: u64) -> Option<H256> {
        self.acks.get(&(*channel, seq)).copied()
    }

    pub fn has_ack(&self, channel: &ChannelId, seq: u64) -> bool {
        self.acks.contains_key(&(*channel, seq))
    }

    // ── Escrow ───────────────────────────────────────────────────────────

    /// Total ZBX escrowed for outbound packets on `channel`.
    pub fn escrow_balance(&self, channel: &ChannelId) -> u128 {
        self.escrow.get(channel).copied().unwrap_or(0)
    }

    /// Credit escrow on recv (for source-chain packets that arrived at us).
    pub fn credit_escrow(&mut self, channel: &ChannelId, amount: u128) {
        *self.escrow.entry(*channel).or_insert(0) += amount;
    }

    /// Debit escrow on release (refund or counterparty recv confirmed).
    pub fn debit_escrow(&mut self, channel: &ChannelId, amount: u128) -> Result<(), XclError> {
        let bal = self.escrow.entry(*channel).or_insert(0);
        if *bal < amount {
            return Err(XclError::InsufficientEscrow { need: amount, have: *bal });
        }
        *bal -= amount;
        Ok(())
    }

    // ── Serialisation helpers (written to state trie) ────────────────────

    /// Produce all pending state trie key-value pairs for this store.
    /// Called at block-seal time to flush XCL state into the persistent trie.
    pub fn state_entries(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut entries = Vec::new();

        for ((ch, seq), hash) in &self.commitments {
            entries.push((commitment_key(ch, *seq), hash.as_bytes().to_vec()));
        }
        for ((ch, seq), _) in &self.receipts {
            entries.push((receipt_key(ch, *seq), vec![0x01]));
        }
        for ((ch, seq), hash) in &self.acks {
            entries.push((ack_key(ch, *seq), hash.as_bytes().to_vec()));
        }
        for (ch, amount) in &self.escrow {
            let mut key = b"xcl/escrow/".to_vec();
            key.extend_from_slice(&ch[..]);
            entries.push((key, amount.to_be_bytes().to_vec()));
        }

        entries
    }
}
