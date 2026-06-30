//! XCL arbitrary cross-chain messaging — MSG-1 protocol.
//!
//! While FT-1 (`transfer.rs`) is specialised for fungible token moves,
//! MSG-1 lets any Solidity contract or off-chain sender deliver an arbitrary
//! byte payload to a contract on a foreign chain — trustlessly, via the same
//! XCL packet + light-client machinery.
//!
//! ## Packet layout (encoded into XclPacket.data)
//!
//! ```text
//! [0]         : APP_ID = 0x02  (distinguishes MSG-1 from FT-1 = 0x01)
//! [1..21]     : sender address  (20 bytes)
//! [21..41]    : receiver address on dst chain (20 bytes)
//! [41..43]    : payload length  (u16 big-endian, max 65535)
//! [43..]      : payload bytes
//! ```
//!
//! ## Example: ZBX Chain A calls a contract on ZBX Chain B
//!
//! ```rust
//! let msg = MsgPacketData::new(
//!     sender,
//!     receiver_on_chain_b,
//!     b"executeStrategy(uint256,address)".to_vec(),
//! );
//! handler.send_message(channel_id, msg, timeout_height, timeout_timestamp)?;
//! ```
//!
//! On Chain B, `recv_packet` delivers the payload to the receiver address
//! via a low-level call (handled by the execution layer using `StateChange::DeliverMessage`).

use crate::error::XclError;
use serde::{Deserialize, Serialize};

/// Application ID byte — distinguishes MSG-1 from FT-1 (0x01).
pub const MSG_APP_ID: u8 = 0x02;

/// Maximum payload size: 64 KiB.
pub const MAX_MSG_PAYLOAD: usize = 65_535;

/// Arbitrary cross-chain message payload (MSG-1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MsgPacketData {
    /// Sender address on the source chain (20 bytes).
    pub sender:   [u8; 20],
    /// Receiver contract address on the destination chain (20 bytes).
    pub receiver: [u8; 20],
    /// Arbitrary payload — ABI-encoded calldata, JSON, or any bytes.
    pub payload:  Vec<u8>,
}

impl MsgPacketData {
    /// Construct a new cross-chain message.
    pub fn new(sender: [u8; 20], receiver: [u8; 20], payload: Vec<u8>) -> Result<Self, XclError> {
        if payload.len() > MAX_MSG_PAYLOAD {
            return Err(XclError::InvalidPacketData(format!(
                "MSG-1 payload too large: {} bytes (max {})",
                payload.len(), MAX_MSG_PAYLOAD
            )));
        }
        Ok(MsgPacketData { sender, receiver, payload })
    }

    /// Encode into XclPacket.data bytes.
    pub fn encode(&self) -> Vec<u8> {
        let payload_len = self.payload.len() as u16;
        let mut out = Vec::with_capacity(1 + 20 + 20 + 2 + self.payload.len());
        out.push(MSG_APP_ID);
        out.extend_from_slice(&self.sender);
        out.extend_from_slice(&self.receiver);
        out.extend_from_slice(&payload_len.to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    /// Decode from XclPacket.data bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, XclError> {
        if bytes.len() < 43 {
            return Err(XclError::InvalidPacketData(
                "MSG-1: data too short (need at least 43 bytes)".into()
            ));
        }
        if bytes[0] != MSG_APP_ID {
            return Err(XclError::InvalidPacketData(format!(
                "MSG-1: wrong app_id byte 0x{:02x} (expected 0x{:02x})",
                bytes[0], MSG_APP_ID
            )));
        }

        let mut sender   = [0u8; 20];
        let mut receiver = [0u8; 20];
        sender.copy_from_slice(&bytes[1..21]);
        receiver.copy_from_slice(&bytes[21..41]);

        let payload_len = u16::from_be_bytes([bytes[41], bytes[42]]) as usize;

        if bytes.len() < 43 + payload_len {
            return Err(XclError::InvalidPacketData(format!(
                "MSG-1: declared payload length {} but only {} bytes remain",
                payload_len,
                bytes.len() - 43
            )));
        }

        let payload = bytes[43..43 + payload_len].to_vec();

        Ok(MsgPacketData { sender, receiver, payload })
    }
}

/// Which application protocol is this packet carrying?
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PacketApp {
    /// FT-1: fungible token transfer (app_id = 0x01).
    FungibleTransfer,
    /// MSG-1: arbitrary cross-chain message (app_id = 0x02).
    Message,
    /// Unknown application — reject.
    Unknown(u8),
}

/// Sniff the app_id byte from raw packet data without full decode.
pub fn detect_app(data: &[u8]) -> PacketApp {
    match data.first() {
        Some(0x01) => PacketApp::FungibleTransfer,
        Some(0x02) => PacketApp::Message,
        Some(&b)   => PacketApp::Unknown(b),
        None       => PacketApp::Unknown(0x00),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_encode_decode() {
        let sender   = [0x11u8; 20];
        let receiver = [0x22u8; 20];
        let payload  = b"executeStrategy(uint256)".to_vec();

        let msg = MsgPacketData::new(sender, receiver, payload.clone()).unwrap();
        let encoded = msg.encode();
        let decoded = MsgPacketData::decode(&encoded).unwrap();

        assert_eq!(decoded.sender,   sender);
        assert_eq!(decoded.receiver, receiver);
        assert_eq!(decoded.payload,  payload);
    }

    #[test]
    fn app_id_is_0x02() {
        let msg = MsgPacketData::new([0u8; 20], [0u8; 20], vec![1, 2, 3]).unwrap();
        let encoded = msg.encode();
        assert_eq!(encoded[0], MSG_APP_ID);
        assert_eq!(detect_app(&encoded), PacketApp::Message);
    }

    #[test]
    fn ft_detected_as_fungible() {
        // FT-1 starts with 0x01
        let ft_bytes = vec![0x01u8, 0, 0, 0];
        assert_eq!(detect_app(&ft_bytes), PacketApp::FungibleTransfer);
    }

    #[test]
    fn payload_too_large_rejected() {
        let big = vec![0u8; MAX_MSG_PAYLOAD + 1];
        assert!(MsgPacketData::new([0u8; 20], [0u8; 20], big).is_err());
    }

    #[test]
    fn wrong_app_id_rejected() {
        let mut bad = MsgPacketData::new([0u8; 20], [0u8; 20], vec![42]).unwrap().encode();
        bad[0] = 0x99;
        assert!(MsgPacketData::decode(&bad).is_err());
    }
}
