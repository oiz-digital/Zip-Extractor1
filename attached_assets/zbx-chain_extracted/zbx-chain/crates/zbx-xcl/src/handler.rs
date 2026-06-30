//! XCL packet handlers — the core protocol logic.
//!
//! All operations are trustless:
//!
//! ## send_packet
//!   - Deducts `amount` from sender's balance into escrow.
//!   - Stores `keccak256(canonical_packet)` in the local commitment store.
//!   - Emits `PacketSent` event.
//!   - Any permissionless relayer can then deliver the commitment proof to the
//!     counterparty chain.
//!
//! ## recv_packet
//!   - Verifies a Merkle proof of the commitment against the source chain's
//!     state root (obtained from the local foreign-client light client).
//!   - Checks the packet has not already been received (replay protection).
//!   - Executes the application logic (FT release or credit).
//!   - Writes a receipt + ack hash into the commitment store for the
//!     counterparty to later verify.
//!
//! ## ack_packet
//!   - Verifies a Merkle proof of the ack against the destination chain's
//!     state root.
//!   - On success ack: deletes the local commitment (packet complete).
//!   - On error ack: refunds the escrowed amount to the original sender.
//!
//! ## timeout_packet
//!   - Verifies absence of a receipt on the destination (non-inclusion proof
//!     OR height/timestamp past timeout).
//!   - Refunds the escrowed amount to the original sender.

use crate::{
    channel::{Channel, ChannelRegistry},
    client::ClientRegistry,
    commitment::CommitmentStore,
    error::XclError,
    message::{MsgPacketData, detect_app, PacketApp},
    packet::{PacketAck, XclPacket, commitment_key, receipt_key, ack_key},
    transfer::{DenomAction, FtPacketData, resolve_denom_action},
};
use tracing::{info, warn};
use zbx_types::H256;

/// A state-change requested by the XCL handler.
/// The execution layer applies these atomically to chain state.
#[derive(Debug, Clone)]
pub enum StateChange {
    /// Debit `amount` from `from` and credit escrow for `channel`.
    EscrowDeposit { from: [u8; 20], channel: [u8; 32], amount: u128 },
    /// Credit `to` from escrow for `channel`.
    EscrowRelease { to: [u8; 20], channel: [u8; 32], amount: u128 },
    /// Refund `to` (timeout or error ack).
    Refund { to: [u8; 20], amount: u128 },
    /// Credit a namespaced IOU balance (foreign asset arrived).
    CreditIou { to: [u8; 20], denom: String, amount: u128 },
    /// Burn a namespaced IOU balance (returning to home chain).
    BurnIou { from: [u8; 20], denom: String, amount: u128 },
    /// Deliver an arbitrary cross-chain message (MSG-1) to `receiver` on this chain.
    ///
    /// The execution layer calls `receiver` as a contract with `payload` as calldata,
    /// from the perspective of the XCL system address (0x00..0b XCL_PRECOMPILE_ADDR).
    /// If the call reverts, the receipt is still written and a NACK ack is returned.
    DeliverMessage {
        src_chain_id: u64,
        sender:       [u8; 20],
        receiver:     [u8; 20],
        channel:      [u8; 32],
        sequence:     u64,
        payload:      Vec<u8>,
    },
}

/// Cross-chain event for indexing / explorers.
#[derive(Debug, Clone)]
pub enum XclEvent {
    PacketSent     { channel: [u8; 32], sequence: u64, dst_chain: u64 },
    PacketReceived { channel: [u8; 32], sequence: u64, src_chain: u64 },
    PacketAcked    { channel: [u8; 32], sequence: u64, success: bool   },
    PacketTimeout  { channel: [u8; 32], sequence: u64                  },
}

/// Result returned by each handler — state changes + events to apply.
#[derive(Debug, Default)]
pub struct HandlerResult {
    pub state_changes: Vec<StateChange>,
    pub events:        Vec<XclEvent>,
}

/// The XCL protocol handler — stateless logic over mutable store references.
pub struct XclHandler<'a> {
    pub channels:    &'a mut ChannelRegistry,
    pub clients:     &'a mut ClientRegistry,
    pub commitment:  &'a mut CommitmentStore,
    pub local_chain: u64,
    pub block_height: u64,
    pub block_timestamp: u64,
}

impl<'a> XclHandler<'a> {

    // ────────────────────────────────────────────────────────────────────────
    // send_packet
    // ────────────────────────────────────────────────────────────────────────

    /// Initiate an outbound cross-chain packet.
    ///
    /// Caller must supply:
    /// - `channel_id` — the open local channel to send on.
    /// - `ft_data` — parsed FtPacketData (denom, amount, sender, receiver).
    /// - `timeout_height` — absolute dst-chain height for expiry (0 = none).
    /// - `timeout_timestamp` — unix seconds for expiry (0 = none).
    pub fn send_packet(
        &mut self,
        channel_id:        [u8; 32],
        ft_data:           FtPacketData,
        timeout_height:    u64,
        timeout_timestamp: u64,
    ) -> Result<(XclPacket, HandlerResult), XclError> {
        let channel = self.channels.require_mut(&channel_id)?;
        channel.assert_open()?;

        let sequence = channel.next_send_seq();
        let packet   = XclPacket {
            sequence,
            src_chain_id:      self.local_chain,
            dst_chain_id:      channel.counterparty_chain_id,
            src_channel:       channel_id,
            dst_channel:       channel.counterparty_channel,
            data:              ft_data.encode(),
            timeout_height,
            timeout_timestamp,
        };

        let amount = ft_data.amount;
        let sender = ft_data.sender;

        self.commitment.set_commitment(&packet, amount, self.block_height);

        info!(
            channel  = %hex::encode(channel_id),
            sequence = sequence,
            amount   = amount,
            dst      = channel.counterparty_chain_id,
            "xcl: packet sent"
        );

        let result = HandlerResult {
            state_changes: vec![StateChange::EscrowDeposit {
                from:    sender,
                channel: channel_id,
                amount,
            }],
            events: vec![XclEvent::PacketSent {
                channel:  channel_id,
                sequence,
                dst_chain: packet.dst_chain_id,
            }],
        };

        Ok((packet, result))
    }

    // ────────────────────────────────────────────────────────────────────────
    // send_message  (MSG-1: arbitrary cross-chain message)
    // ────────────────────────────────────────────────────────────────────────

    /// Send an arbitrary cross-chain message to a contract on the counterparty chain.
    ///
    /// Unlike `send_packet` (which moves tokens), this carries an arbitrary byte
    /// payload (`msg_data.payload`) that the receiving chain will deliver to
    /// `msg_data.receiver` as a contract call.
    ///
    /// No tokens are escrowed — the commitment records 0 escrowed amount.
    /// Timeouts and acks work exactly like FT-1 packets.
    pub fn send_message(
        &mut self,
        channel_id:        [u8; 32],
        msg_data:          MsgPacketData,
        timeout_height:    u64,
        timeout_timestamp: u64,
    ) -> Result<(XclPacket, HandlerResult), XclError> {
        let channel = self.channels.require_mut(&channel_id)?;
        channel.assert_open()?;

        let sequence = channel.next_send_seq();
        let packet   = XclPacket {
            sequence,
            src_chain_id:      self.local_chain,
            dst_chain_id:      channel.counterparty_chain_id,
            src_channel:       channel_id,
            dst_channel:       channel.counterparty_channel,
            data:              msg_data.encode(),
            timeout_height,
            timeout_timestamp,
        };

        // MSG-1 packets carry no escrowed funds — record 0.
        self.commitment.set_commitment(&packet, 0, self.block_height);

        info!(
            channel  = %hex::encode(channel_id),
            sequence = sequence,
            receiver = %hex::encode(msg_data.receiver),
            payload  = msg_data.payload.len(),
            dst      = channel.counterparty_chain_id,
            "xcl: message packet sent"
        );

        let result = HandlerResult {
            state_changes: vec![],   // no escrow for MSG-1
            events: vec![XclEvent::PacketSent {
                channel:   channel_id,
                sequence,
                dst_chain: packet.dst_chain_id,
            }],
        };

        Ok((packet, result))
    }

    // ────────────────────────────────────────────────────────────────────────
    // recv_packet
    // ────────────────────────────────────────────────────────────────────────

    /// Receive a packet from the counterparty, verifying the commitment proof.
    ///
    /// `proof_height` — the foreign-chain block height at which the proof was generated.
    /// `proof_nodes`  — MPT proof nodes (root → leaf) for the commitment key.
    pub fn recv_packet(
        &mut self,
        packet:       XclPacket,
        proof_height: u64,
        proof_nodes:  &[Vec<u8>],
    ) -> Result<HandlerResult, XclError> {
        // 1. Channel validation.
        let channel = self.channels.require_mut(&packet.dst_channel)?;
        channel.assert_open()?;
        if channel.counterparty_chain_id != packet.src_chain_id {
            return Err(XclError::ChainIdMismatch {
                expected: channel.counterparty_chain_id,
                got:      packet.src_chain_id,
            });
        }

        // 2. Timeout check — if already timed out at current height, reject.
        if packet.is_timed_out_height(self.block_height) {
            return Err(XclError::PacketTimeout {
                packet_height: self.block_height,
                timeout:       packet.timeout_height,
            });
        }
        if packet.is_timed_out_timestamp(self.block_timestamp) {
            return Err(XclError::PacketTimeout {
                packet_height: self.block_timestamp,
                timeout:       packet.timeout_timestamp,
            });
        }

        // 3. Sequence check (Ordered channels only).
        channel.check_recv_seq(packet.sequence)?;

        let dst_channel = packet.dst_channel;
        let src_chain_id = packet.src_chain_id;

        // 4. Replay protection — reject if already received.
        if self.commitment.has_receipt(&dst_channel, packet.sequence) {
            return Err(XclError::PacketAlreadyReceived(
                hex::encode(dst_channel),
                packet.sequence,
            ));
        }

        // 5. Verify commitment proof against foreign-client state root.
        let client_id = channel.client_id;
        let client    = self.clients.require_mut(&client_id)?;

        let commitment   = packet.commitment();
        let trie_key     = commitment_key(&packet.src_channel, packet.sequence);
        let trie_value   = commitment.as_bytes();

        client.verify_state_proof(proof_height, &trie_key, trie_value, proof_nodes)?;

        // 6. Dispatch to application layer based on app_id byte.
        let mut state_changes = Vec::new();
        match detect_app(&packet.data) {
            // ── FT-1: fungible token transfer ────────────────────────────
            PacketApp::FungibleTransfer => {
                let ft     = FtPacketData::decode(&packet.data)?;
                let action = resolve_denom_action(
                    &ft.denom, src_chain_id, &dst_channel, ft.amount, ft.receiver,
                );
                match action {
                    DenomAction::ReleaseFromEscrow { channel: ch, amount, receiver, .. } => {
                        self.commitment.debit_escrow(&ch, amount)?;
                        state_changes.push(StateChange::EscrowRelease {
                            to: receiver,
                            channel: dst_channel,
                            amount,
                        });
                    }
                    DenomAction::CreditNamespaced { namespaced_denom, amount, receiver } => {
                        state_changes.push(StateChange::CreditIou {
                            to:    receiver,
                            denom: namespaced_denom.0,
                            amount,
                        });
                    }
                }
                info!(
                    channel  = %hex::encode(dst_channel),
                    sequence = packet.sequence,
                    amount   = ft.amount,
                    src      = src_chain_id,
                    "xcl: FT-1 packet received"
                );
            }

            // ── MSG-1: arbitrary cross-chain message ─────────────────────
            PacketApp::Message => {
                let msg = MsgPacketData::decode(&packet.data)?;
                info!(
                    channel  = %hex::encode(dst_channel),
                    sequence = packet.sequence,
                    sender   = %hex::encode(msg.sender),
                    receiver = %hex::encode(msg.receiver),
                    payload  = msg.payload.len(),
                    src      = src_chain_id,
                    "xcl: MSG-1 packet received"
                );
                // Execution layer will call receiver contract with payload.
                state_changes.push(StateChange::DeliverMessage {
                    src_chain_id,
                    sender:   msg.sender,
                    receiver: msg.receiver,
                    channel:  dst_channel,
                    sequence: packet.sequence,
                    payload:  msg.payload,
                });
            }

            // ── Unknown app_id: reject ───────────────────────────────────
            PacketApp::Unknown(id) => {
                return Err(XclError::UnsupportedApp(id));
            }
        }

        // 7. Write receipt + ack into commitment store.
        let ack = PacketAck::success();
        self.commitment.set_receipt(&dst_channel, packet.sequence)?;
        self.commitment.set_ack(&dst_channel, packet.sequence, &ack)?;

        info!(
            channel  = %hex::encode(dst_channel),
            sequence = packet.sequence,
            src      = src_chain_id,
            "xcl: packet receipt written"
        );

        Ok(HandlerResult {
            state_changes,
            events: vec![XclEvent::PacketReceived {
                channel:   dst_channel,
                sequence:  packet.sequence,
                src_chain: src_chain_id,
            }],
        })
    }

    // ────────────────────────────────────────────────────────────────────────
    // ack_packet
    // ────────────────────────────────────────────────────────────────────────

    /// Acknowledge a sent packet, verifying the ack proof from the destination.
    ///
    /// On success ack → delete commitment (packet complete, escrow released).
    /// On error ack   → delete commitment + refund sender.
    pub fn ack_packet(
        &mut self,
        packet:       XclPacket,
        ack:          PacketAck,
        proof_height: u64,
        proof_nodes:  &[Vec<u8>],
    ) -> Result<HandlerResult, XclError> {
        // 1. Channel validation.
        let channel = self.channels.require_mut(&packet.src_channel)?;
        channel.assert_open()?;

        // 2. Verify commitment exists.
        if !self.commitment.has_commitment(&packet.src_channel, packet.sequence) {
            return Err(XclError::NoCommitment(
                hex::encode(packet.src_channel),
                packet.sequence,
            ));
        }

        // 3. Verify ack proof against destination-chain state root.
        let client_id = channel.client_id;
        let client    = self.clients.require_mut(&client_id)?;

        let ack_bytes   = ack.encode();
        use sha3::{Digest, Keccak256};
        let ack_hash    = Keccak256::digest(&ack_bytes);
        let trie_key    = ack_key(&packet.dst_channel, packet.sequence);

        client.verify_state_proof(proof_height, &trie_key, &ack_hash, proof_nodes)?;

        channel.advance_ack_seq();

        // 4. Delete commitment and determine outcome.
        let pending = self.commitment.delete_commitment(&packet.src_channel, packet.sequence)
            .ok_or_else(|| XclError::NoCommitment(hex::encode(packet.src_channel), packet.sequence))?;

        let success = ack.is_success();
        let mut state_changes = Vec::new();

        if !success {
            // Error ack → refund sender.
            let ft = FtPacketData::decode(&packet.data)?;
            warn!(
                channel  = %hex::encode(packet.src_channel),
                sequence = packet.sequence,
                reason   = ack.error,
                "xcl: error ack received — refunding sender"
            );
            state_changes.push(StateChange::Refund {
                to:     ft.sender,
                amount: pending.escrowed_amount,
            });
        }

        info!(
            channel  = %hex::encode(packet.src_channel),
            sequence = packet.sequence,
            success  = success,
            "xcl: packet acknowledged"
        );

        Ok(HandlerResult {
            state_changes,
            events: vec![XclEvent::PacketAcked {
                channel:  packet.src_channel,
                sequence: packet.sequence,
                success,
            }],
        })
    }

    // ────────────────────────────────────────────────────────────────────────
    // timeout_packet
    // ────────────────────────────────────────────────────────────────────────

    /// Timeout a sent packet, verifying that no receipt exists on the dst chain.
    ///
    /// Two timeout modes:
    /// - Height timeout: `proof_nodes` proves absence of receipt at `proof_height ≥ packet.timeout_height`.
    /// - Timestamp timeout: block timestamp has passed `packet.timeout_timestamp`.
    pub fn timeout_packet(
        &mut self,
        packet:       XclPacket,
        proof_height: u64,
        proof_nodes:  &[Vec<u8>],
    ) -> Result<HandlerResult, XclError> {
        // 1. Channel validation.
        let channel = self.channels.require_mut(&packet.src_channel)?;
        channel.assert_open()?;

        // 2. Check that the timeout has actually elapsed.
        let height_expired = packet.is_timed_out_height(self.block_height)
            || (packet.timeout_height > 0 && proof_height >= packet.timeout_height);
        let ts_expired     = packet.is_timed_out_timestamp(self.block_timestamp);
        if !height_expired && !ts_expired {
            return Err(XclError::PacketNotTimedOut);
        }

        // 3. Verify non-inclusion of receipt on dst chain.
        //    For height timeouts, verify_proof with empty value = non-inclusion proof.
        let client_id = channel.client_id;
        let client    = self.clients.require_mut(&client_id)?;

        let trie_key = receipt_key(&packet.dst_channel, packet.sequence);
        if !proof_nodes.is_empty() {
            // Non-inclusion proof: value = empty slice.
            client.verify_state_proof(proof_height, &trie_key, &[], proof_nodes)?;
        }

        // 4. Delete commitment and refund.
        let pending = self.commitment.delete_commitment(&packet.src_channel, packet.sequence)
            .ok_or_else(|| XclError::NoCommitment(hex::encode(packet.src_channel), packet.sequence))?;

        let ft = FtPacketData::decode(&packet.data)?;

        warn!(
            channel  = %hex::encode(packet.src_channel),
            sequence = packet.sequence,
            amount   = pending.escrowed_amount,
            "xcl: packet timed out — refunding sender"
        );

        Ok(HandlerResult {
            state_changes: vec![StateChange::Refund {
                to:     ft.sender,
                amount: pending.escrowed_amount,
            }],
            events: vec![XclEvent::PacketTimeout {
                channel:  packet.src_channel,
                sequence: packet.sequence,
            }],
        })
    }
}
