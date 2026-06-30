//! Solidity ABI encoding/decoding for the 0xCA AIINFER precompile.
//!
//! EVM contracts call 0xCA with ABI-encoded input:
//!   abi.encode(uint8 model_id, bytes input_data)
//!
//! The precompile returns:
//!   abi.encode(bytes output, uint16 confidence)
//!
//! This module handles the encoding/decoding without any external ABI crate.
//! All operations are pure byte manipulation — safe and deterministic.

use crate::error::AiError;
use crate::model::ModelId;

/// Decoded call from EVM → precompile.
#[derive(Debug, Clone)]
pub struct AiCallInput {
    pub model_id: ModelId,
    pub data:     Vec<u8>,
}

/// Encoded return value from precompile → EVM.
#[derive(Debug, Clone)]
pub struct AiCallOutput {
    pub output:     Vec<u8>,   // raw model output bytes
    pub confidence: u16,       // basis points 0–10000
}

impl AiCallInput {
    /// Decode ABI-encoded input from EVM.
    ///
    /// Layout (ABI static+dynamic encoding):
    ///   [0..32]  : model_id as uint8 (right-padded in 32-byte slot)
    ///   [32..64] : offset to bytes data (always 0x40 = 64)
    ///   [64..96] : length of bytes data
    ///   [96..]   : bytes data (padded to 32-byte boundary)
    pub fn decode(raw: &[u8]) -> Result<Self, AiError> {
        if raw.len() < 96 {
            return Err(AiError::AbiDecodeError(
                "input too short: need at least 96 bytes".into()
            ));
        }

        // model_id: last byte of first 32-byte slot
        let model_byte = raw[31];
        let model_id = ModelId::from_byte(model_byte)
            .ok_or_else(|| AiError::AbiDecodeError(
                format!("unknown model_id: 0x{model_byte:02x}")
            ))?;

        // data offset (slot 2) — we expect 0x40
        let _offset = u256_from_be(&raw[32..64]);

        // data length (slot 3)
        let data_len = u256_from_be(&raw[64..96]) as usize;
        if data_len > 1024 {
            return Err(AiError::InputTooLarge(data_len));
        }
        if raw.len() < 96 + data_len {
            return Err(AiError::AbiDecodeError(
                format!("data truncated: need {} bytes after offset 96, got {}", data_len, raw.len() - 96)
            ));
        }

        let data = raw[96..96 + data_len].to_vec();
        Ok(Self { model_id, data })
    }
}

impl AiCallOutput {
    /// ABI-encode the output for return to EVM.
    ///
    /// Layout:
    ///   [0..32]  : offset to bytes output (0x40)
    ///   [32..64] : confidence as uint16 (right-padded)
    ///   [64..96] : length of output bytes
    ///   [96..]   : output bytes (padded to 32-byte boundary)
    pub fn encode(&self) -> Vec<u8> {
        let out_len = self.output.len();
        let padded_len = round_up_32(out_len);
        let mut buf = Vec::with_capacity(96 + padded_len);

        // slot 0: offset to bytes data = 64 (0x40)
        buf.extend_from_slice(&u256_to_be(64u64));

        // slot 1: confidence (u16, right-padded in 32 bytes)
        buf.extend_from_slice(&u256_to_be(self.confidence as u64));

        // slot 2: length of output
        buf.extend_from_slice(&u256_to_be(out_len as u64));

        // output data padded to 32-byte boundary
        buf.extend_from_slice(&self.output);
        for _ in 0..padded_len - out_len {
            buf.push(0u8);
        }
        buf
    }
}

/// Read a uint256 from 32 big-endian bytes (we only use lower 64 bits).
fn u256_from_be(bytes: &[u8]) -> u64 {
    let mut arr = [0u8; 8];
    let src = &bytes[24..32]; // lower 8 bytes
    arr.copy_from_slice(src);
    u64::from_be_bytes(arr)
}

/// Write u64 as 32-byte big-endian uint256.
fn u256_to_be(v: u64) -> [u8; 32] {
    let mut arr = [0u8; 32];
    arr[24..32].copy_from_slice(&v.to_be_bytes());
    arr
}

/// Round up n to the next multiple of 32.
fn round_up_32(n: usize) -> usize {
    if n == 0 { 32 } else { (n + 31) & !31 }
}

/// Helper: encode a simple error response (class=0xFF, confidence=0).
pub fn encode_error_response() -> Vec<u8> {
    AiCallOutput { output: vec![0xFFu8], confidence: 0 }.encode()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelId;

    fn make_encoded_input(model_id: u8, data: &[u8]) -> Vec<u8> {
        let data_len = data.len();
        let padded = round_up_32(data_len);
        let mut buf = Vec::with_capacity(96 + padded);
        // slot 0: model_id
        buf.extend_from_slice(&u256_to_be(model_id as u64));
        // slot 1: offset (64)
        buf.extend_from_slice(&u256_to_be(64));
        // slot 2: length
        buf.extend_from_slice(&u256_to_be(data_len as u64));
        // data
        buf.extend_from_slice(data);
        for _ in 0..padded - data_len { buf.push(0); }
        buf
    }

    #[test]
    fn roundtrip_spam_classifier() {
        let data = b"token_address_xyz_test";
        let encoded = make_encoded_input(0x01, data);
        let decoded = AiCallInput::decode(&encoded).unwrap();
        assert_eq!(decoded.model_id, ModelId::SpamClassifier);
        assert_eq!(decoded.data, data);
    }

    #[test]
    fn output_encode_decode_length() {
        let out = AiCallOutput { output: vec![1u8, 2, 3], confidence: 8500 };
        let enc = out.encode();
        // Must be at least 96 bytes (3 slots) + padded output
        assert!(enc.len() >= 96);
        // Confidence in slot 1 bytes 24..32
        let conf_bytes: [u8; 8] = enc[32+24..32+32].try_into().unwrap();
        assert_eq!(u64::from_be_bytes(conf_bytes), 8500);
    }

    #[test]
    fn empty_input_rejects() {
        let err = AiCallInput::decode(&[]).unwrap_err();
        assert!(matches!(err, AiError::AbiDecodeError(_)));
    }

    #[test]
    fn unknown_model_id_rejects() {
        let encoded = make_encoded_input(0xFF, b"test");
        let err = AiCallInput::decode(&encoded).unwrap_err();
        assert!(matches!(err, AiError::AbiDecodeError(_)));
    }

    #[test]
    fn error_response_is_valid() {
        let enc = encode_error_response();
        assert!(!enc.is_empty());
    }
}
