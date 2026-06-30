//! Foreign chain light client.
//!
//! Stores and verifies block headers from a foreign ZBX-compatible chain.
//! Verification is fully trustless:
//!   1. Each header's QC (BLS aggregate signature) is verified against the
//!      stored validator set using the real BLS12-381 pairing.
//!   2. State proofs (Merkle Patricia Trie) are verified against the header's
//!      `state_root`.
//!
//! ## Why no bridges?
//!
//! A bridge requires trusting off-chain relayers (or a multisig). This light
//! client instead trusts *math* — any forged header would require forging a
//! BLS aggregate signature from 2f+1 validators, which is computationally
//! infeasible under the DL assumption on BLS12-381.

use crate::error::XclError;
use crate::packet::ClientId;
use sha3::{Digest, Keccak256};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tracing::{debug, info, warn};
use zbx_crypto::bls::{BlsPubKey, BlsSignature, verify_aggregate};
use zbx_trie::verify_proof;
use zbx_types::H256;

/// A compact foreign chain block header stored by the light client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForeignHeader {
    pub number:            u64,
    pub hash:              H256,
    pub parent_hash:       H256,
    pub state_root:        H256,
    pub transactions_root: H256,
    pub receipts_root:     H256,
    pub timestamp:         u64,
    /// Aggregate BLS QC: 96-byte compressed G2 over `keccak256(header_hash)`.
    pub quorum_cert:       Vec<u8>,
    /// Bitmap of which validators contributed to the QC (bit i = validator i).
    pub signer_bitmap:     Vec<u8>,
}

impl ForeignHeader {
    /// Message that validators signed: `keccak256(chain_id_be8 || block_hash)`.
    pub fn signing_msg(&self, chain_id: u64) -> [u8; 32] {
        let mut buf = [0u8; 40];
        buf[..8].copy_from_slice(&chain_id.to_be_bytes());
        buf[8..].copy_from_slice(self.hash.as_bytes());
        let h = Keccak256::digest(buf);
        let mut out = [0u8; 32];
        out.copy_from_slice(&h);
        out
    }
}

/// A registered foreign chain light client.
#[derive(Debug)]
pub struct ForeignClient {
    /// Unique client identifier.
    pub client_id:     ClientId,
    /// Chain ID of the foreign chain.
    pub chain_id:      u64,
    /// Stored headers, keyed by block height.
    headers:           BTreeMap<u64, ForeignHeader>,
    /// Active validator BLS public keys (the 2f+1 committee).
    validators:        Vec<BlsPubKey>,
    /// Latest verified header height.
    pub latest_height: u64,
    /// Maximum headers to keep in memory (prune oldest beyond this).
    max_headers:       usize,
    /// If true, skip BLS QC check (devnet/testing only — never set in production).
    skip_qc:           bool,
}

impl ForeignClient {
    pub fn new(client_id: ClientId, chain_id: u64) -> Self {
        ForeignClient {
            client_id,
            chain_id,
            headers:       BTreeMap::new(),
            validators:    Vec::new(),
            latest_height: 0,
            max_headers:   2048,
            skip_qc:       false,
        }
    }

    /// Set initial or updated validator set. Called on epoch boundaries.
    pub fn set_validators(&mut self, validators: Vec<BlsPubKey>) {
        info!(
            client  = %hex::encode(self.client_id),
            count   = validators.len(),
            "xcl: updated foreign validator set"
        );
        self.validators = validators;
    }

    /// Insert and verify a new foreign header.
    ///
    /// Checks (in order):
    /// 1. Height must be strictly greater than `latest_height`.
    /// 2. Parent hash must match the stored parent (if known).
    /// 3. QC must be a valid BLS aggregate signature from 2f+1 validators.
    pub fn update_header(&mut self, header: ForeignHeader) -> Result<(), XclError> {
        if header.number <= self.latest_height && self.latest_height > 0 {
            return Err(XclError::StaleHeader(header.number, self.latest_height));
        }

        // Parent continuity check (skip for genesis / first inserted header).
        if let Some(prev) = self.headers.get(&(header.number.saturating_sub(1))) {
            if prev.hash != header.parent_hash {
                return Err(XclError::ProofInvalid(format!(
                    "parent hash mismatch at height {}: expected {}, got {}",
                    header.number,
                    hex::encode(prev.hash.as_bytes()),
                    hex::encode(header.parent_hash.as_bytes()),
                )));
            }
        }

        // BLS QC verification.
        if !self.skip_qc {
            self.verify_qc(&header)?;
        } else {
            warn!(
                client = %hex::encode(self.client_id),
                height = header.number,
                "xcl: skip_qc=true — QC not verified (devnet only!)"
            );
        }

        let height = header.number;
        self.latest_height = height;
        self.headers.insert(height, header);

        // Prune oldest headers to stay within memory budget.
        while self.headers.len() > self.max_headers {
            if let Some(oldest) = self.headers.keys().next().copied() {
                self.headers.remove(&oldest);
            }
        }

        debug!(
            client = %hex::encode(self.client_id),
            height = height,
            "xcl: foreign header accepted"
        );
        Ok(())
    }

    /// Verify BLS QC on a foreign header.
    fn verify_qc(&self, header: &ForeignHeader) -> Result<(), XclError> {
        if self.validators.is_empty() {
            return Err(XclError::NoValidatorSet);
        }
        if header.quorum_cert.len() != 96 {
            return Err(XclError::InvalidQc(format!(
                "QC must be 96 bytes, got {}",
                header.quorum_cert.len()
            )));
        }

        // Build the list of signing keys from the signer bitmap.
        // verify_aggregate expects &[BlsPubKey] (owned), so clone.
        let active_keys: Vec<BlsPubKey> = if header.signer_bitmap.is_empty() {
            // No bitmap — all validators signed (common in small sets).
            self.validators.clone()
        } else {
            self.validators.iter().enumerate()
                .filter(|(i, _)| {
                    let byte = i / 8;
                    let bit  = i % 8;
                    header.signer_bitmap.get(byte).map_or(false, |b| (b >> bit) & 1 == 1)
                })
                .map(|(_, k)| k.clone())
                .collect()
        };

        // 2f+1 quorum check: signer count must exceed 2/3 of total.
        let total   = self.validators.len();
        let signers = active_keys.len();
        let quorum  = 2 * total / 3 + 1;
        if signers < quorum {
            return Err(XclError::InvalidQc(format!(
                "insufficient signers: {signers}/{total} (need {quorum})"
            )));
        }

        // Parse the BLS aggregate signature.
        let sig = BlsSignature::from_bytes(&header.quorum_cert)
            .map_err(|e| XclError::InvalidQc(format!("bad sig bytes: {e:?}")))?;

        // The signing message is chain_id || block_hash, wrapped in H256.
        let raw_msg = header.signing_msg(self.chain_id);
        let msg = zbx_types::H256(raw_msg);

        // BLS12-381 aggregate verification via real pairing.
        // verify_aggregate(sig, pubkeys, msg) — sig first.
        let ok = verify_aggregate(&sig, &active_keys, &msg);
        if !ok {
            return Err(XclError::InvalidQc("BLS pairing check failed".into()));
        }

        Ok(())
    }

    /// Get stored header at `height`.
    pub fn header(&self, height: u64) -> Option<&ForeignHeader> {
        self.headers.get(&height)
    }

    /// Get the state root of the header at `height`.
    pub fn state_root(&self, height: u64) -> Option<H256> {
        self.headers.get(&height).map(|h| h.state_root)
    }

    /// Verify a state trie inclusion proof against the foreign chain's state root.
    ///
    /// `proof_nodes` is an ordered list of RLP-encoded MPT nodes from root → leaf.
    /// `key` is the trie key (hashed storage or account key).
    /// `expected_value` is the expected leaf value.
    ///
    /// Returns `Ok(())` if the proof is valid (value exists and matches).
    pub fn verify_state_proof(
        &self,
        height:         u64,
        key:            &[u8],
        expected_value: &[u8],
        proof_nodes:    &[Vec<u8>],
    ) -> Result<(), XclError> {
        let state_root = self.state_root(height)
            .ok_or(XclError::HeaderNotFound(height))?;

        // verify_proof takes &Option<Vec<u8>>:
        //   None  = non-inclusion proof (key absent)
        //   Some  = inclusion proof (key maps to value)
        let expected_opt: Option<Vec<u8>> = if expected_value.is_empty() {
            None
        } else {
            Some(expected_value.to_vec())
        };

        let valid = verify_proof(state_root, key, &expected_opt, proof_nodes);

        if !valid {
            return Err(XclError::ProofInvalid(
                "MPT proof does not match expected value".into()
            ));
        }

        Ok(())
    }
}

/// Registry of all registered foreign clients.
#[derive(Debug, Default)]
pub struct ClientRegistry {
    clients: std::collections::HashMap<ClientId, ForeignClient>,
}

impl ClientRegistry {
    pub fn new() -> Self {
        Self { clients: std::collections::HashMap::new() }
    }

    pub fn register(&mut self, client: ForeignClient) {
        info!(
            client   = %hex::encode(client.client_id),
            chain_id = client.chain_id,
            "xcl: registered foreign client"
        );
        self.clients.insert(client.client_id, client);
    }

    pub fn get(&self, id: &ClientId) -> Option<&ForeignClient> {
        self.clients.get(id)
    }

    pub fn get_mut(&mut self, id: &ClientId) -> Option<&mut ForeignClient> {
        self.clients.get_mut(id)
    }

    pub fn require_mut(&mut self, id: &ClientId) -> Result<&mut ForeignClient, XclError> {
        self.clients.get_mut(id)
            .ok_or_else(|| XclError::ClientNotFound(hex::encode(id)))
    }
}
