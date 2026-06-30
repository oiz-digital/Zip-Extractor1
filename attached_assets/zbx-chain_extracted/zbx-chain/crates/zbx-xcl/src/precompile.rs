//! XCL EVM precompile — address 0x0b (decimal 11).
//!
//! Allows smart contracts to initiate cross-chain token transfers natively
//! without going through a bridge contract. The precompile is callable from
//! any Solidity contract via `CALL 0x0b`.
//!
//! ## ABI
//!
//! ```solidity
//! // Send a cross-chain fungible token transfer.
//! // Returns 32 bytes: 0-padded uint64 sequence number.
//! function xcl_send(
//!     bytes32 channel_id,    // local channel ID
//!     bytes32 receiver,      // receiver on the destination chain (right-padded)
//!     uint128 amount,        // amount in wei
//!     uint64  timeout_height // 0 for no height timeout
//! ) external returns (uint64 sequence);
//! ```
//!
//! Input encoding (fixed, no ABI overhead):
//!   channel_id(32) || receiver(32) || amount(16 BE) || timeout_height(8 BE)
//!
//! Output: sequence(8 BE), zero-padded to 32 bytes.

use crate::error::XclError;

/// Address of the XCL precompile: `0x000000000000000000000000000000000000000b`.
pub const XCL_PRECOMPILE_ADDR: [u8; 20] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0x0b,
];

/// Minimum gas cost for an XCL send call.
///
/// Higher than a simple CALL because it writes to the state trie and emits a
/// cross-chain event. Intentionally conservative.
pub const XCL_SEND_GAS: u64 = 25_000;

/// Parsed arguments from a `xcl_send` precompile call.
#[derive(Debug, Clone)]
pub struct XclSendArgs {
    pub channel_id:     [u8; 32],
    pub receiver:       [u8; 20],
    pub amount:         u128,
    pub timeout_height: u64,
}

/// Parse the ABI-encoded input for `xcl_send`.
///
/// Input layout:
///   bytes[0..32]  = channel_id
///   bytes[32..64] = receiver (right-padded — use last 20 bytes)
///   bytes[64..80] = amount (u128 BE)
///   bytes[80..88] = timeout_height (u64 BE)
pub fn parse_xcl_send(input: &[u8]) -> Result<XclSendArgs, XclError> {
    if input.len() < 88 {
        return Err(XclError::DecodeFailed(format!(
            "xcl_send: need 88 bytes, got {}",
            input.len()
        )));
    }

    let mut channel_id = [0u8; 32];
    channel_id.copy_from_slice(&input[0..32]);

    let mut receiver = [0u8; 20];
    // receiver is right-padded in a 32-byte word — take last 20 bytes.
    receiver.copy_from_slice(&input[44..64]);

    let amount = u128::from_be_bytes(input[64..80].try_into().unwrap());
    let timeout_height = u64::from_be_bytes(input[80..88].try_into().unwrap());

    Ok(XclSendArgs { channel_id, receiver, amount, timeout_height })
}

/// Encode the precompile return value: sequence number as 32-byte zero-padded u64.
pub fn encode_xcl_send_output(sequence: u64) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[24..32].copy_from_slice(&sequence.to_be_bytes());
    out
}

/// Gas cost for the precompile call (checked before execution).
pub fn xcl_gas_cost(input_len: usize, available_gas: u64) -> Result<u64, XclError> {
    let cost = XCL_SEND_GAS + (input_len as u64 / 32) * 100;
    if available_gas < cost {
        return Err(XclError::Internal(format!(
            "out of gas: need {cost}, have {available_gas}"
        )));
    }
    Ok(cost)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xcl_precompile_addr() {
        assert_eq!(XCL_PRECOMPILE_ADDR[19], 0x0b);
        assert_eq!(XCL_PRECOMPILE_ADDR[..19], [0u8; 19]);
    }

    #[test]
    fn test_parse_xcl_send_roundtrip() {
        let mut input = [0u8; 88];
        // channel_id = [1; 32]
        input[0..32].fill(1u8);
        // receiver = 0xdeadbeef... (last 20 bytes of word)
        input[44..64].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef, 0, 0, 0, 0, 0, 0,
                                        0,    0,    0,    0,    0, 0, 0, 0, 0, 0xff]);
        // amount = 1e18
        let amount: u128 = 1_000_000_000_000_000_000;
        input[64..80].copy_from_slice(&amount.to_be_bytes());
        // timeout_height = 100
        input[80..88].copy_from_slice(&100u64.to_be_bytes());

        let args = parse_xcl_send(&input).unwrap();
        assert_eq!(args.channel_id, [1u8; 32]);
        assert_eq!(args.receiver[0], 0xde);
        assert_eq!(args.receiver[19], 0xff);
        assert_eq!(args.amount, 1_000_000_000_000_000_000);
        assert_eq!(args.timeout_height, 100);
    }

    #[test]
    fn test_encode_xcl_send_output() {
        let out = encode_xcl_send_output(42);
        assert_eq!(&out[..24], &[0u8; 24]);
        assert_eq!(&out[24..32], &42u64.to_be_bytes());
    }

    #[test]
    fn test_xcl_send_input_too_short() {
        let input = [0u8; 50];
        assert!(parse_xcl_send(&input).is_err());
    }
}
