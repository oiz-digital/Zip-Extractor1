//! Blob types and blob transaction format.

use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;
use crate::{commitment::KzgCommitment, error::DaError};

/// Inner 128 KB buffer, wrapped so we can impl Serde via serde-big-array.
#[derive(Clone, Debug)]
pub struct BlobInner(pub [u8; crate::BLOB_SIZE]);

impl Serialize for BlobInner {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        BigArray::serialize(&self.0, s)
    }
}
impl<'de> Deserialize<'de> for BlobInner {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(BlobInner(BigArray::deserialize(d)?))
    }
}

/// A 128 KB blob of arbitrary data.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Blob(pub Box<BlobInner>);

impl Blob {
    /// Create a new zeroed blob.
    pub fn zeroed() -> Self {
        Blob(Box::new(BlobInner([0u8; crate::BLOB_SIZE])))
    }

    /// Create a blob from bytes, zero-padding if shorter.
    pub fn from_bytes(data: &[u8]) -> Result<Self, DaError> {
        if data.len() > crate::BLOB_SIZE {
            return Err(DaError::BlobTooLarge(data.len()));
        }
        let mut inner = BlobInner([0u8; crate::BLOB_SIZE]);
        inner.0[..data.len()].copy_from_slice(data);
        Ok(Blob(Box::new(inner)))
    }

    /// Return the versioned hash (sha256) of this blob, prefixed with 0x01.
    pub fn versioned_hash(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(&self.0.0);
        let mut out = [0u8; 32];
        out.copy_from_slice(&h.finalize());
        out[0] = 0x01; // version byte
        out
    }
}

/// A sidecar attached to a blob transaction: blob + KZG commitment + proof.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlobSidecar {
    /// The raw blob data.
    pub blob: Blob,
    /// KZG polynomial commitment to the blob.
    pub commitment: KzgCommitment,
    /// KZG proof that commitment corresponds to blob.
    pub proof: crate::commitment::KzgProof,
}

/// Extended transaction type that carries blob sidecars (type 0x03).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlobTransaction {
    /// Chain ID (8989 for ZBX mainnet, 8990 for testnet+devnet).
    pub chain_id: u64,
    /// Sender nonce.
    pub nonce: u64,
    /// Max fee per gas (wei).
    pub max_fee_per_gas: u128,
    /// Max priority fee per gas (wei).
    pub max_priority_fee_per_gas: u128,
    /// Max fee per blob gas (wei). Separate fee market for blobs.
    pub max_fee_per_blob_gas: u128,
    /// Target contract address (usually a rollup inbox).
    pub to: [u8; 20],
    /// ETH value (usually 0).
    pub value: u128,
    /// Calldata (rollup batch pointer / metadata).
    pub input: Vec<u8>,
    /// Versioned hashes of each blob in this transaction.
    pub blob_versioned_hashes: Vec<[u8; 32]>,
    /// Full sidecars (not included in tx hash, broadcast separately).
    #[serde(skip)]
    pub sidecars: Vec<BlobSidecar>,
}

impl BlobTransaction {
    /// Validate that versioned hashes match the sidecar commitments.
    pub fn validate_sidecars(&self) -> Result<(), DaError> {
        if self.sidecars.len() != self.blob_versioned_hashes.len() {
            return Err(DaError::SidecarCountMismatch {
                expected: self.blob_versioned_hashes.len(),
                got: self.sidecars.len(),
            });
        }
        if self.sidecars.len() > crate::MAX_BLOBS_PER_BLOCK {
            return Err(DaError::TooManyBlobs(self.sidecars.len()));
        }
        for (sidecar, expected_hash) in self.sidecars.iter().zip(&self.blob_versioned_hashes) {
            let actual_hash = sidecar.blob.versioned_hash();
            if &actual_hash != expected_hash {
                return Err(DaError::HashMismatch);
            }
        }
        Ok(())
    }
}