//! Fungible Token Transfer protocol (FT-1).
//!
//! Defines the application-layer packet data for cross-chain native token
//! transfers. This is NOT a wrapped-token bridge — assets are locked on the
//! source chain and released (not minted) on the destination chain.
//!
//! ## Denom namespacing
//!
//! When ZBX crosses from ZBX chain (8989) to a foreign chain via channel `ch`:
//!   - Source locks `amount` of "ZBX" into escrow.
//!   - Destination receives `amount` of "xcl/8989/ch_hex/ZBX".
//!
//! When the namespaced denom crosses back:
//!   - Source destroys the namespaced balance.
//!   - Destination releases `amount` of "ZBX" from escrow.
//!
//! This means the total supply of ZBX across all chains is always conserved —
//! no mint/burn authority is granted to any bridge contract.

use crate::error::XclError;
use crate::packet::ChannelId;
use serde::{Deserialize, Serialize};

/// Token denomination.
///
/// - Native: `"ZBX"` or any single-segment string.
/// - Namespaced (already crossed at least once): `"xcl/{src_chain}/{channel_hex}/{base}"`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Denom(pub String);

impl Denom {
    /// Construct a native denom.
    pub fn native(name: &str) -> Self {
        Denom(name.to_string())
    }

    /// Namespace this denom when it crosses a channel from `src_chain_id`.
    pub fn namespace(&self, src_chain_id: u64, channel: &ChannelId) -> Self {
        Denom(format!("xcl/{}/{}/{}", src_chain_id, hex::encode(channel), self.0))
    }

    /// If this is a namespaced denom that originated from `src_chain_id` via
    /// `channel`, strip one layer of namespacing and return the inner denom.
    /// Returns `None` if this denom was not namespaced by that chain+channel.
    pub fn unwrap_namespace(&self, src_chain_id: u64, channel: &ChannelId) -> Option<Self> {
        let prefix = format!("xcl/{}/{}/", src_chain_id, hex::encode(channel));
        self.0.strip_prefix(&prefix).map(|inner| Denom(inner.to_string()))
    }

    /// Returns `true` if this denom carries a namespace prefix (i.e. it was
    /// transferred from another chain and represents a locked foreign asset).
    pub fn is_namespaced(&self) -> bool {
        self.0.starts_with("xcl/")
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Denom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Application-layer data for a fungible token transfer packet.
///
/// Encoded deterministically as:
///   `denom_len(4) || denom_bytes || amount(16 BE) || sender(20) || receiver(20) || memo_len(4) || memo`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FtPacketData {
    /// Token denomination (potentially namespaced).
    pub denom:    Denom,
    /// Amount in the denom's smallest unit (wei for ZBX).
    pub amount:   u128,
    /// Sender address on the source chain.
    pub sender:   [u8; 20],
    /// Receiver address on the destination chain.
    pub receiver: [u8; 20],
    /// Optional memo string (max 256 bytes).
    pub memo:     String,
}

impl FtPacketData {
    pub fn new(
        denom:    Denom,
        amount:   u128,
        sender:   [u8; 20],
        receiver: [u8; 20],
        memo:     String,
    ) -> Self {
        FtPacketData { denom, amount, sender, receiver, memo }
    }

    /// Canonical deterministic encoding (not JSON — for on-chain commitments).
    ///
    /// Layout: `[0x01][u32 denom_len][denom][u128 amount][sender][receiver][u32 memo_len][memo]`
    /// The first byte (0x01) is the FT-1 app_id — allows MSG-1 (0x02) packets to share the
    /// same `XclPacket.data` field while being distinguishable at the handler layer.
    pub fn encode(&self) -> Vec<u8> {
        let denom_bytes = self.denom.0.as_bytes();
        let memo_bytes  = self.memo.as_bytes();
        let mut buf = Vec::with_capacity(
            1 + 4 + denom_bytes.len() + 16 + 20 + 20 + 4 + memo_bytes.len()
        );
        buf.push(0x01); // FT-1 app_id
        buf.extend_from_slice(&(denom_bytes.len() as u32).to_be_bytes());
        buf.extend_from_slice(denom_bytes);
        buf.extend_from_slice(&self.amount.to_be_bytes());
        buf.extend_from_slice(&self.sender);
        buf.extend_from_slice(&self.receiver);
        buf.extend_from_slice(&(memo_bytes.len() as u32).to_be_bytes());
        buf.extend_from_slice(memo_bytes);
        buf
    }

    /// Decode from the canonical encoding produced by `encode()`.
    ///
    /// Accepts both legacy (no app_id prefix) and current (0x01 prefix) formats.
    pub fn decode(bytes: &[u8]) -> Result<Self, XclError> {
        if bytes.is_empty() {
            return Err(XclError::DecodeFailed("empty packet data".into()));
        }
        // Skip app_id byte 0x01 if present (current format).
        // If first byte is NOT 0x01, treat as legacy format (no prefix).
        let bytes = if bytes[0] == 0x01 { &bytes[1..] } else { bytes };

        let mut cursor = 0usize;

        macro_rules! read_u32 {
            () => {{
                if bytes.len() < cursor + 4 {
                    return Err(XclError::DecodeFailed("unexpected EOF reading u32".into()));
                }
                let v = u32::from_be_bytes(bytes[cursor..cursor + 4].try_into().unwrap());
                cursor += 4;
                v as usize
            }};
        }

        macro_rules! read_bytes {
            ($n:expr) => {{
                let n = $n;
                if bytes.len() < cursor + n {
                    return Err(XclError::DecodeFailed(format!("unexpected EOF: need {n} bytes")));
                }
                let slice = &bytes[cursor..cursor + n];
                cursor += n;
                slice
            }};
        }

        // denom
        let denom_len  = read_u32!();
        let denom_str  = std::str::from_utf8(read_bytes!(denom_len))
            .map_err(|_| XclError::DecodeFailed("invalid UTF-8 in denom".into()))?;
        let denom      = Denom(denom_str.to_string());

        // amount (16 bytes big-endian u128)
        let amount_bytes = read_bytes!(16);
        let amount = u128::from_be_bytes(amount_bytes.try_into().unwrap());

        // sender / receiver (20 bytes each)
        let sender_bytes   = read_bytes!(20);
        let receiver_bytes = read_bytes!(20);
        let mut sender   = [0u8; 20];
        let mut receiver = [0u8; 20];
        sender.copy_from_slice(sender_bytes);
        receiver.copy_from_slice(receiver_bytes);

        // memo
        let memo_len = read_u32!();
        let memo_str = std::str::from_utf8(read_bytes!(memo_len))
            .map_err(|_| XclError::DecodeFailed("invalid UTF-8 in memo".into()))?;

        Ok(FtPacketData { denom, amount, sender, receiver, memo: memo_str.to_string() })
    }
}

/// Determine how a received denom should be handled on the destination chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenomAction {
    /// The denom is returning home — release from escrow on this chain.
    ReleaseFromEscrow {
        base_denom: Denom,
        channel:    ChannelId,
        amount:     u128,
        receiver:   [u8; 20],
    },
    /// The denom is foreign — credit the namespaced IOU balance.
    CreditNamespaced {
        namespaced_denom: Denom,
        amount:           u128,
        receiver:         [u8; 20],
    },
}

/// Compute the denom action for an incoming packet.
///
/// If `packet_denom` contains the namespace for `src_chain_id + dst_channel`,
/// the packet is returning the denom to its home chain → escrow release.
/// Otherwise the packet introduces a foreign asset → credit namespaced IOU.
pub fn resolve_denom_action(
    packet_denom:   &Denom,
    src_chain_id:   u64,
    dst_channel:    &ChannelId,
    amount:         u128,
    receiver:       [u8; 20],
) -> DenomAction {
    if let Some(base) = packet_denom.unwrap_namespace(src_chain_id, dst_channel) {
        DenomAction::ReleaseFromEscrow {
            base_denom: base,
            channel:    *dst_channel,
            amount,
            receiver,
        }
    } else {
        let namespaced = packet_denom.namespace(src_chain_id, dst_channel);
        DenomAction::CreditNamespaced {
            namespaced_denom: namespaced,
            amount,
            receiver,
        }
    }
}
