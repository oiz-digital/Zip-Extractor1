//! DA (Data Availability) Layer — Model Weight Verification.
//!
//! Model weights are stored on the ZBX DA layer (ZEP-003), content-addressed
//! by their SHA3-256 hash. Before running inference, validators verify that
//! the weights match the registered hash in the ModelRegistry.
//!
//! This prevents:
//! - Tampered model weights (produces wrong predictions)
//! - Model substitution attacks (replacing one model with another)
//! - Version drift between validators
//!
//! Security properties:
//! - SHA3-256 collision resistance: finding two weight sets with same hash
//!   requires 2^128 operations (quantum-safe at this level).
//! - All validators check the same hash → any tampering is immediately detected.
//! - Model weights are immutable once deployed (content-addressed = immutable).

use crate::error::AiError;

/// Maximum model weight size: 4 MB.
pub const MAX_WEIGHT_SIZE: usize = 4 * 1024 * 1024;

/// DA layer model weight entry.
#[derive(Debug, Clone)]
pub struct WeightEntry {
    /// SHA3-256 hash of the raw weight bytes.
    pub da_hash: [u8; 32],
    /// Raw weight bytes (loaded from DA layer).
    pub weights: Vec<u8>,
    /// Model format version.
    pub version: u32,
}

impl WeightEntry {
    /// Create a new weight entry and verify hash on construction.
    pub fn new(weights: Vec<u8>, version: u32) -> Result<Self, AiError> {
        if weights.len() > MAX_WEIGHT_SIZE {
            return Err(AiError::ModelWeightsUnavailable);
        }
        let da_hash = sha3_256(&weights);
        Ok(Self { da_hash, weights, version })
    }

    /// Verify that loaded weights match the expected hash.
    pub fn verify(&self, expected_hash: &[u8; 32]) -> Result<(), AiError> {
        let actual = sha3_256(&self.weights);
        if actual != *expected_hash {
            return Err(AiError::WeightHashMismatch {
                expected: hex_encode(expected_hash),
                actual:   hex_encode(&actual),
            });
        }
        Ok(())
    }
}

/// DA layer reference (content-addressed pointer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaRef {
    /// SHA3-256 hash of the content.
    pub hash: [u8; 32],
    /// Size hint (bytes).
    pub size: u32,
}

impl DaRef {
    pub fn new(hash: [u8; 32], size: u32) -> Self {
        Self { hash, size }
    }

    pub fn hash_hex(&self) -> String {
        hex_encode(&self.hash)
    }

    /// Verify that a blob matches this DA reference.
    pub fn verify_blob(&self, blob: &[u8]) -> Result<(), AiError> {
        let actual = sha3_256(blob);
        if actual != self.hash {
            return Err(AiError::WeightHashMismatch {
                expected: hex_encode(&self.hash),
                actual:   hex_encode(&actual),
            });
        }
        if blob.len() != self.size as usize {
            return Err(AiError::Inference(
                format!("DA blob size mismatch: expected {} got {}", self.size, blob.len())
            ));
        }
        Ok(())
    }
}

/// Model header embedded in the weight blob (first 64 bytes).
#[derive(Debug, Clone)]
pub struct ModelHeader {
    /// Magic bytes: "ZBXAI001"
    pub magic:      [u8; 8],
    /// Model ID (matches ModelId enum).
    pub model_id:   u8,
    /// Format version (must be 1 for this implementation).
    pub version:    u8,
    /// Number of quantization layers.
    pub num_layers: u8,
    /// Reserved padding.
    pub _reserved:  [u8; 5],
    /// Input size (u32 LE).
    pub input_size: u32,
    /// Hidden size (u32 LE).
    pub hidden_size: u32,
    /// Output size (u32 LE).
    pub output_size: u32,
    /// SHA3-256 of the weight data following the header.
    pub weight_hash: [u8; 32],
}

impl ModelHeader {
    pub const MAGIC: &'static [u8; 8] = b"ZBXAI001";
    pub const SIZE: usize = 64;

    /// Parse header from the first 64 bytes of a weight blob.
    pub fn parse(blob: &[u8]) -> Result<Self, AiError> {
        if blob.len() < Self::SIZE {
            return Err(AiError::InvalidModelWeights(
                format!("blob too short for header: {} < {}", blob.len(), Self::SIZE)
            ));
        }
        let mut magic = [0u8; 8];
        magic.copy_from_slice(&blob[0..8]);
        if &magic != Self::MAGIC {
            return Err(AiError::InvalidModelWeights(
                format!("wrong magic: {:?}", &magic)
            ));
        }
        let model_id   = blob[8];
        let version    = blob[9];
        let num_layers = blob[10];
        let mut reserved = [0u8; 5];
        reserved.copy_from_slice(&blob[11..16]);
        let input_size  = u32::from_le_bytes(blob[16..20].try_into().unwrap());
        let hidden_size = u32::from_le_bytes(blob[20..24].try_into().unwrap());
        let output_size = u32::from_le_bytes(blob[24..28].try_into().unwrap());
        let mut weight_hash = [0u8; 32];
        weight_hash.copy_from_slice(&blob[32..64]);

        if version != 1 {
            return Err(AiError::InvalidModelWeights(
                format!("unsupported model version: {version}")
            ));
        }

        Ok(Self {
            magic, model_id, version, num_layers, _reserved: reserved,
            input_size, hidden_size, output_size, weight_hash,
        })
    }

    /// Verify the weight data following the header.
    pub fn verify_weights(&self, blob: &[u8]) -> Result<(), AiError> {
        if blob.len() < Self::SIZE {
            return Err(AiError::ModelWeightsUnavailable);
        }
        let weight_data = &blob[Self::SIZE..];
        let actual_hash = sha3_256(weight_data);
        if actual_hash != self.weight_hash {
            return Err(AiError::WeightHashMismatch {
                expected: hex_encode(&self.weight_hash),
                actual:   hex_encode(&actual_hash),
            });
        }
        Ok(())
    }
}

/// Deterministic stub weight blob generator for testing.
/// Produces a valid header + dummy weights deterministically from model_id.
pub fn stub_weight_blob(model_id: u8, input: u32, hidden: u32, output: u32) -> Vec<u8> {
    // Build dummy weight data
    let weight_data: Vec<u8> = (0..(hidden * input + output * hidden + hidden + output) as usize)
        .map(|i| ((model_id as usize ^ i) % 256) as u8)
        .collect();
    let weight_hash = sha3_256(&weight_data);

    let mut blob = Vec::with_capacity(ModelHeader::SIZE + weight_data.len());
    blob.extend_from_slice(ModelHeader::MAGIC);  // 0..8
    blob.push(model_id);    // 8
    blob.push(1u8);         // 9: version
    blob.push(2u8);         // 10: num_layers
    blob.extend_from_slice(&[0u8; 5]); // 11..16: reserved
    blob.extend_from_slice(&input.to_le_bytes());   // 16..20
    blob.extend_from_slice(&hidden.to_le_bytes());  // 20..24
    blob.extend_from_slice(&output.to_le_bytes());  // 24..28
    blob.extend_from_slice(&[0u8; 4]);  // 28..32: padding
    blob.extend_from_slice(&weight_hash); // 32..64
    blob.extend_from_slice(&weight_data);
    blob
}

/// Pure Rust SHA3-256 (Keccak) — no external crate needed, uses sha3 crate.
fn sha3_256(data: &[u8]) -> [u8; 32] {
    use sha3::{Digest, Sha3_256};
    let mut h = Sha3_256::new();
    h.update(data);
    let out = h.finalize();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&out);
    arr
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_blob_valid_header() {
        let blob = stub_weight_blob(0x01, 8, 16, 4);
        let hdr = ModelHeader::parse(&blob).unwrap();
        assert_eq!(hdr.model_id, 0x01);
        assert_eq!(hdr.input_size, 8);
        assert_eq!(hdr.hidden_size, 16);
        assert_eq!(hdr.output_size, 4);
    }

    #[test]
    fn stub_blob_weight_hash_verifies() {
        let blob = stub_weight_blob(0x02, 16, 32, 8);
        let hdr = ModelHeader::parse(&blob).unwrap();
        hdr.verify_weights(&blob).unwrap();
    }

    #[test]
    fn tampered_blob_fails_verification() {
        let mut blob = stub_weight_blob(0x03, 8, 16, 4);
        blob[ModelHeader::SIZE + 10] ^= 0xFF; // flip a bit in weights
        let hdr = ModelHeader::parse(&blob).unwrap();
        let err = hdr.verify_weights(&blob).unwrap_err();
        assert!(matches!(err, AiError::WeightHashMismatch { .. }));
    }

    #[test]
    fn da_ref_verify() {
        let data = b"model weight data here";
        let hash = sha3_256(data);
        let da_ref = DaRef::new(hash, data.len() as u32);
        da_ref.verify_blob(data).unwrap();
    }

    #[test]
    fn da_ref_tampered_data_fails() {
        let data = b"model weight data here";
        let hash = sha3_256(data);
        let da_ref = DaRef::new(hash, data.len() as u32);
        let mut tampered = data.to_vec();
        tampered[0] ^= 0xFF;
        da_ref.verify_blob(&tampered).unwrap_err();
    }

    #[test]
    fn weight_entry_new_computes_hash() {
        let w = vec![1u8, 2, 3, 4, 5];
        let entry = WeightEntry::new(w.clone(), 1).unwrap();
        entry.verify(&entry.da_hash.clone()).unwrap();
    }
}
