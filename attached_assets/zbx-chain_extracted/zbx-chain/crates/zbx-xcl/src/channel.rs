//! Channel state machine.
//!
//! A channel is a logical, ordered (or unordered) pipe between two chains.
//! Channels are established through a 4-step handshake:
//!
//!   Chain A                         Chain B
//!   ───────                         ───────
//!   CHAN_OPEN_INIT  ──────────────► CHAN_OPEN_TRY
//!   CHAN_OPEN_ACK   ◄────────────── CHAN_OPEN_ACK
//!   CHAN_OPEN_CONFIRM ────────────► OPEN
//!
//! Once OPEN, packets flow freely. Either side may initiate CHAN_CLOSE_INIT /
//! CHAN_CLOSE_CONFIRM to orderly close the channel.

use crate::packet::{ChannelId, ClientId};
use crate::error::XclError;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Message ordering guarantee.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ordering {
    /// Packets may be received and processed in any order.
    Unordered,
    /// Packets must be received in strict sequence order.
    Ordered,
}

/// Channel lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelState {
    /// Channel initialization submitted on the initiating chain.
    Init,
    /// Counterparty received the INIT and replied with TRY.
    TryOpen,
    /// Both sides have completed the handshake — packets may flow.
    Open,
    /// Channel has been closed (either orderly or due to timeout).
    Closed,
}

impl std::fmt::Display for ChannelState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelState::Init    => write!(f, "Init"),
            ChannelState::TryOpen => write!(f, "TryOpen"),
            ChannelState::Open    => write!(f, "Open"),
            ChannelState::Closed  => write!(f, "Closed"),
        }
    }
}

/// A cross-chain channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    /// Local channel ID.
    pub id:                    ChannelId,
    /// Current handshake / lifecycle state.
    pub state:                 ChannelState,
    /// Packet ordering guarantee.
    pub ordering:              Ordering,
    /// Light client tracking the counterparty chain.
    pub client_id:             ClientId,
    /// Counterparty's channel ID (known after TryOpen).
    pub counterparty_channel:  ChannelId,
    /// Counterparty chain ID.
    pub counterparty_chain_id: u64,
    /// Next outbound sequence number (1-indexed, incremented on each send).
    pub next_seq_send:         u64,
    /// Next expected inbound sequence number (incremented on each recv).
    pub next_seq_recv:         u64,
    /// Next expected ack sequence number (incremented on each ack).
    pub next_seq_ack:          u64,
    /// Block height at which this channel was created.
    pub created_height:        u64,
}

impl Channel {
    /// Create a new channel in `Init` state.
    pub fn new(
        id:                    ChannelId,
        ordering:              Ordering,
        client_id:             ClientId,
        counterparty_chain_id: u64,
        created_height:        u64,
    ) -> Self {
        Channel {
            id,
            state: ChannelState::Init,
            ordering,
            client_id,
            counterparty_channel:  [0u8; 32],
            counterparty_chain_id,
            next_seq_send: 1,
            next_seq_recv: 1,
            next_seq_ack:  1,
            created_height,
        }
    }

    /// Transition to TryOpen — called on the counterparty after receiving INIT proof.
    pub fn try_open(
        &mut self,
        counterparty_channel: ChannelId,
    ) -> Result<(), XclError> {
        if self.state != ChannelState::Init {
            return Err(XclError::ChannelNotOpen(
                hex::encode(self.id),
                self.state.to_string(),
            ));
        }
        self.counterparty_channel = counterparty_channel;
        self.state = ChannelState::TryOpen;
        info!(
            channel = %hex::encode(self.id),
            counterparty = %hex::encode(counterparty_channel),
            "channel → TryOpen"
        );
        Ok(())
    }

    /// Transition to Open — called on both sides to complete the handshake.
    pub fn open(&mut self) -> Result<(), XclError> {
        match self.state {
            ChannelState::TryOpen | ChannelState::Init => {
                self.state = ChannelState::Open;
                info!(channel = %hex::encode(self.id), "channel → Open");
                Ok(())
            }
            ChannelState::Open => Ok(()),
            _ => Err(XclError::ChannelNotOpen(
                hex::encode(self.id),
                self.state.to_string(),
            )),
        }
    }

    /// Transition to Closed.
    pub fn close(&mut self) {
        if self.state != ChannelState::Closed {
            warn!(channel = %hex::encode(self.id), "channel → Closed");
            self.state = ChannelState::Closed;
        }
    }

    /// Assert the channel is in Open state.
    pub fn assert_open(&self) -> Result<(), XclError> {
        if self.state != ChannelState::Open {
            return Err(XclError::ChannelNotOpen(
                hex::encode(self.id),
                self.state.to_string(),
            ));
        }
        Ok(())
    }

    /// Consume the next outbound sequence number.
    pub fn next_send_seq(&mut self) -> u64 {
        let seq = self.next_seq_send;
        self.next_seq_send += 1;
        seq
    }

    /// Verify and advance the next expected recv sequence for Ordered channels.
    pub fn check_recv_seq(&mut self, seq: u64) -> Result<(), XclError> {
        if self.ordering == Ordering::Ordered && seq != self.next_seq_recv {
            return Err(XclError::SequenceOutOfOrder {
                expected: self.next_seq_recv,
                got: seq,
            });
        }
        if self.ordering == Ordering::Ordered {
            self.next_seq_recv += 1;
        }
        Ok(())
    }

    /// Advance ack sequence for Ordered channels.
    pub fn advance_ack_seq(&mut self) {
        if self.ordering == Ordering::Ordered {
            self.next_seq_ack += 1;
        }
    }
}

/// Registry of all local channels, keyed by channel ID.
#[derive(Debug, Default)]
pub struct ChannelRegistry {
    channels: std::collections::HashMap<ChannelId, Channel>,
}

impl ChannelRegistry {
    pub fn new() -> Self {
        Self { channels: std::collections::HashMap::new() }
    }

    pub fn insert(&mut self, ch: Channel) {
        self.channels.insert(ch.id, ch);
    }

    pub fn get(&self, id: &ChannelId) -> Option<&Channel> {
        self.channels.get(id)
    }

    pub fn get_mut(&mut self, id: &ChannelId) -> Option<&mut Channel> {
        self.channels.get_mut(id)
    }

    pub fn require_mut(&mut self, id: &ChannelId) -> Result<&mut Channel, XclError> {
        self.channels.get_mut(id).ok_or_else(|| XclError::ChannelNotFound(hex::encode(id)))
    }

    pub fn all(&self) -> impl Iterator<Item = &Channel> {
        self.channels.values()
    }
}
