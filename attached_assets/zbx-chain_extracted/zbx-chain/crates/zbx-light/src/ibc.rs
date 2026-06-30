//! IBC (Inter-Blockchain Communication) light client compatibility (ZEP-024).
//!
//! Implements ICS-002 client semantics for ZBX Chain, allowing any IBC-enabled
//! chain (Cosmos Hub, Osmosis, etc.) to maintain a trusted view of ZBX Chain
//! state using BLS aggregate signatures (from ZEP-016) and Verkle proofs
//! (from ZEP-021).

use serde::{Deserialize, Serialize};
use zbx_types::{address::Address, H256};
use std::collections::HashMap;
use thiserror::Error;

// ── Error types ───────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum IbcClientError {
    #[error("client is frozen at height {0}")]
    ClientFrozen(u64),

    #[error("header timestamp {0} is in the past (trusting period expired)")]
    TrustingPeriodExpired(u64),

    #[error("header height {got} is not greater than current trusted height {trusted}")]
    HeaderNotNewer { got: u64, trusted: u64 },

    #[error("insufficient validators signed: {signed} < {required}")]
    InsufficientSigners { signed: usize, required: usize },

    #[error("BLS aggregate signature verification failed")]
    BLSVerificationFailed,

    #[error("misbehaviour detected: conflicting headers at height {0}")]
    MisbehaviourDetected(u64),

    #[error("client not found: {0}")]
    ClientNotFound(String),

    #[error("channel not found: {0}")]
    ChannelNotFound(String),

    #[error("packet commitment not found for sequence {0}")]
    PacketCommitmentNotFound(u64),
}

// ── IBC Height ────────────────────────────────────────────────────────────────

/// IBC height: (revision_number, revision_height).
/// For ZBX mainnet: revision_number = 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct IbcHeight {
    pub revision_number: u64,
    pub revision_height: u64,
}

impl IbcHeight {
    pub fn new(height: u64) -> Self {
        IbcHeight { revision_number: 0, revision_height: height }
    }

    pub fn zero() -> Self {
        IbcHeight { revision_number: 0, revision_height: 0 }
    }

    pub fn is_zero(&self) -> bool {
        self.revision_height == 0
    }
}

impl std::fmt::Display for IbcHeight {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}", self.revision_number, self.revision_height)
    }
}

// ── Fraction (trust level) ────────────────────────────────────────────────────

/// Trust level as a fraction (e.g. 1/3 minimum for BFT).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Fraction {
    pub numerator:   u64,
    pub denominator: u64,
}

impl Fraction {
    pub const ONE_THIRD: Fraction  = Fraction { numerator: 1, denominator: 3 };
    pub const TWO_THIRDS: Fraction = Fraction { numerator: 2, denominator: 3 };

    pub fn is_satisfied_by(&self, signed: usize, total: usize) -> bool {
        if total == 0 { return false; }
        signed as u64 * self.denominator >= self.numerator * total as u64
    }
}

// ── IBC Client State ──────────────────────────────────────────────────────────

/// ZBX Chain client state stored on a counterparty IBC chain.
/// Implements ICS-002 ClientState.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZbxClientState {
    /// Chain ID (e.g. "zbx-mainnet-1")
    pub chain_id: String,

    /// Minimum fraction of validators that must sign a header (typically 1/3)
    pub trust_level: Fraction,

    /// How long to trust a header after its timestamp (seconds).
    /// Must be < unbonding_period.
    pub trusting_period_secs: u64,

    /// Validator unbonding period (seconds). Must be > trusting_period.
    pub unbonding_period_secs: u64,

    /// Maximum allowed clock drift between chains (seconds).
    pub max_clock_drift_secs: u64,

    /// The latest verified height.
    pub latest_height: IbcHeight,

    /// If set, the client is frozen (misbehaviour detected).
    /// No new headers can be submitted.
    pub frozen_height: Option<IbcHeight>,

    /// Upgrade path for chain upgrades.
    pub upgrade_path: Vec<String>,
}

impl ZbxClientState {
    pub const CHAIN_ID_MAINNET: &'static str = "zbx-mainnet-1";
    pub const CHAIN_ID_TESTNET: &'static str = "zbx-testnet-1";

    /// Default mainnet client state.
    pub fn mainnet_default() -> Self {
        ZbxClientState {
            chain_id:               Self::CHAIN_ID_MAINNET.to_string(),
            trust_level:            Fraction::ONE_THIRD,
            trusting_period_secs:   14 * 24 * 3600, // 14 days (ICS-002: must be < unbonding)
            unbonding_period_secs:  21 * 24 * 3600, // 21 days (H-05 fix: trusting < unbonding)
            max_clock_drift_secs:   10,
            latest_height:          IbcHeight::zero(),
            frozen_height:          None,
            upgrade_path:           vec!["upgrade".to_string(), "upgradedClient".to_string()],
        }
    }

    pub fn is_frozen(&self) -> bool {
        self.frozen_height.is_some()
    }
}

// ── IBC Consensus State ───────────────────────────────────────────────────────

/// ZBX Chain consensus state at a specific height.
/// Implements ICS-002 ConsensusState.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZbxConsensusState {
    /// Unix timestamp of the block (nanoseconds).
    pub timestamp_ns: u64,

    /// Verkle state root at this height (from ZEP-021).
    pub root: H256,

    /// Commitment to the next validator set (for validator set changes).
    pub next_validators_hash: H256,
}

// ── IBC Header (Update) ───────────────────────────────────────────────────────

/// A ZBX Chain header submitted to update an IBC client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZbxHeader {
    pub height:           IbcHeight,
    pub timestamp_ns:     u64,
    pub state_root:       H256,
    pub tx_root:          H256,
    pub proposer:         Address,
    pub next_validators:  Vec<IbcValidatorInfo>,
    pub next_validators_hash: H256,
    /// BLS aggregate signature from 2f+1 validators (from ZEP-016)
    pub agg_signature:    Vec<u8>,
    /// Bitmap of which validators signed
    pub signer_bitmap:    Vec<u8>,
    /// QC round for two-phase commit verification
    pub qc_round:         u64,
}

impl ZbxHeader {
    pub fn block_hash(&self) -> H256 {
        use sha3::{Digest, Sha3_256};
        let mut h = Sha3_256::new();
        h.update(self.height.revision_height.to_le_bytes());
        h.update(&self.state_root.0);
        h.update(&self.tx_root.0);
        let hash = h.finalize();
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&hash);
        H256(arr)
    }
}

/// Validator info for IBC purposes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IbcValidatorInfo {
    pub address:      Address,
    pub bls_pub_key:  Vec<u8>,  // 48-byte BLS12-381 G1 point
    pub voting_power: u64,
}

// ── Misbehaviour ──────────────────────────────────────────────────────────────

/// Misbehaviour evidence: two conflicting headers at the same height.
/// Submitted to freeze a client that has been attacked.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZbxMisbehaviour {
    pub client_id:  String,
    pub header_1:   ZbxHeader,
    pub header_2:   ZbxHeader,
}

// ── Client Registry ───────────────────────────────────────────────────────────

/// Registry of IBC clients on ZBX Chain (one per counterparty chain).
pub struct IbcClientRegistry {
    clients:   HashMap<String, ZbxClientState>,
    consensus: HashMap<(String, IbcHeight), ZbxConsensusState>,
}

impl IbcClientRegistry {
    pub fn new() -> Self {
        IbcClientRegistry {
            clients:   HashMap::new(),
            consensus: HashMap::new(),
        }
    }

    /// Create a new IBC client for a counterparty chain.
    pub fn create_client(
        &mut self,
        client_id: String,
        client_state: ZbxClientState,
        consensus_state: ZbxConsensusState,
    ) -> Result<(), IbcClientError> {
        let height = client_state.latest_height;
        self.clients.insert(client_id.clone(), client_state);
        self.consensus.insert((client_id, height), consensus_state);
        Ok(())
    }

    /// Update an existing IBC client with a new header.
    pub fn update_client(
        &mut self,
        client_id: &str,
        header: ZbxHeader,
        current_time_ns: u64,
        validator_set: &[IbcValidatorInfo],
    ) -> Result<(ZbxClientState, ZbxConsensusState), IbcClientError> {
        let client = self.clients.get(client_id)
            .ok_or_else(|| IbcClientError::ClientNotFound(client_id.to_string()))?
            .clone();

        // Check not frozen
        if client.is_frozen() {
            return Err(IbcClientError::ClientFrozen(
                client.frozen_height.unwrap().revision_height,
            ));
        }

        // Check header is newer
        if header.height <= client.latest_height {
            return Err(IbcClientError::HeaderNotNewer {
                got:     header.height.revision_height,
                trusted: client.latest_height.revision_height,
            });
        }

        // Check trusting period has not expired
        let header_time_secs = header.timestamp_ns / 1_000_000_000;
        let current_time_secs = current_time_ns / 1_000_000_000;
        let age_secs = current_time_secs.saturating_sub(header_time_secs);
        if age_secs > client.trusting_period_secs {
            return Err(IbcClientError::TrustingPeriodExpired(header_time_secs));
        }

        // Verify BLS aggregate signature
        let signed_count = count_signers(&header.signer_bitmap);
        let required = client.trust_level;
        if !required.is_satisfied_by(signed_count, validator_set.len()) {
            return Err(IbcClientError::InsufficientSigners {
                signed:   signed_count,
                required: (validator_set.len() as u64 * required.numerator / required.denominator) as usize + 1,
            });
        }

        // Verify BLS aggregate signature — real pairing check over G2 (H-01 fix).
        // Collect public keys for all signers indicated in the bitmap.
        // H-01 fix: real BLS12-381 aggregate pairing check.
        // Collect G1 public keys for every validator whose bit is set in signer_bitmap.
        let signer_pubkeys: Vec<zbx_crypto::bls::BlsPubKey> = validator_set
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                let byte_idx = i / 8;
                let bit_idx  = i % 8;
                header.signer_bitmap.get(byte_idx).map(|b| (b >> bit_idx) & 1 == 1).unwrap_or(false)
            })
            .filter_map(|(_, v)| zbx_crypto::bls::BlsPubKey::from_bytes(&v.bls_pub_key).ok())
            .collect();

        if signer_pubkeys.is_empty() || header.agg_signature.is_empty() {
            return Err(IbcClientError::BLSVerificationFailed);
        }

        // Reconstruct the signing message: keccak256(state_root ‖ revision_height ‖ timestamp_s)
        let mut msg_input = [0u8; 48];
        msg_input[..32].copy_from_slice(header.state_root.as_bytes());
        msg_input[32..40].copy_from_slice(&header.height.revision_height.to_be_bytes());
        msg_input[40..48].copy_from_slice(&(header.timestamp_ns / 1_000_000_000).to_be_bytes());
        let msg_hash: zbx_types::H256 = zbx_crypto::keccak::keccak256(&msg_input);

        let agg_sig = match zbx_crypto::bls::BlsSignature::from_bytes(&header.agg_signature) {
            Ok(s)  => s,
            Err(_) => return Err(IbcClientError::BLSVerificationFailed),
        };

        if !zbx_crypto::bls::verify_aggregate(&agg_sig, &signer_pubkeys, &msg_hash) {
            return Err(IbcClientError::BLSVerificationFailed);
        }

        // Build new client state
        let new_client = ZbxClientState {
            latest_height: header.height,
            ..client.clone()
        };
        let new_consensus = ZbxConsensusState {
            timestamp_ns:         header.timestamp_ns,
            root:                 header.state_root,
            next_validators_hash: header.next_validators_hash,
        };

        self.clients.insert(client_id.to_string(), new_client.clone());
        self.consensus.insert(
            (client_id.to_string(), header.height),
            new_consensus.clone(),
        );

        Ok((new_client, new_consensus))
    }

    /// Submit misbehaviour evidence to freeze the client.
    pub fn submit_misbehaviour(
        &mut self,
        misbehaviour: ZbxMisbehaviour,
    ) -> Result<(), IbcClientError> {
        let client = self.clients.get_mut(&misbehaviour.client_id)
            .ok_or_else(|| IbcClientError::ClientNotFound(misbehaviour.client_id.clone()))?;

        if misbehaviour.header_1.height == misbehaviour.header_2.height
            && misbehaviour.header_1.block_hash() != misbehaviour.header_2.block_hash()
        {
            // Conflicting headers at same height → equivocation detected
            let freeze_height = misbehaviour.header_1.height;
            client.frozen_height = Some(freeze_height);
            Ok(())
        } else {
            Err(IbcClientError::MisbehaviourDetected(
                misbehaviour.header_1.height.revision_height,
            ))
        }
    }

    pub fn get_client(&self, id: &str) -> Option<&ZbxClientState> {
        self.clients.get(id)
    }

    pub fn get_consensus(&self, id: &str, height: IbcHeight) -> Option<&ZbxConsensusState> {
        self.consensus.get(&(id.to_string(), height))
    }
}

impl Default for IbcClientRegistry {
    fn default() -> Self { Self::new() }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn count_signers(bitmap: &[u8]) -> usize {
    bitmap.iter().map(|b| b.count_ones() as usize).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_header(height: u64) -> ZbxHeader {
        ZbxHeader {
            height:              IbcHeight::new(height),
            timestamp_ns:        1_000_000_000 * height,
            state_root:          H256([0u8; 32]),
            tx_root:             H256([0u8; 32]),
            proposer:            Address([0u8; 20]),
            next_validators:     vec![],
            next_validators_hash: H256([0u8; 32]),
            agg_signature:       vec![1u8; 96], // non-empty = "valid"
            signer_bitmap:       vec![0xFF, 0xFF], // all validators signed
            qc_round:            height,
        }
    }

    fn make_validators(n: usize) -> Vec<IbcValidatorInfo> {
        (0..n).map(|i| IbcValidatorInfo {
            address:      Address([i as u8; 20]),
            bls_pub_key:  vec![0u8; 48],
            voting_power: 1000,
        }).collect()
    }

    #[test]
    fn create_and_update_client() {
        let mut reg = IbcClientRegistry::new();
        let client_state = ZbxClientState::mainnet_default();
        let consensus = ZbxConsensusState {
            timestamp_ns: 1_000_000_000,
            root: H256([0u8; 32]),
            next_validators_hash: H256([0u8; 32]),
        };
        reg.create_client("cosmos-hub".to_string(), client_state, consensus).unwrap();

        let validators = make_validators(5);
        let header = make_header(1);
        let result = reg.update_client("cosmos-hub", header, 10_000_000_000, &validators);
        assert!(result.is_ok());
    }

    #[test]
    fn frozen_client_rejects_updates() {
        let mut reg = IbcClientRegistry::new();
        let mut client_state = ZbxClientState::mainnet_default();
        client_state.frozen_height = Some(IbcHeight::new(5));
        let consensus = ZbxConsensusState {
            timestamp_ns: 1_000_000_000,
            root: H256([0u8; 32]),
            next_validators_hash: H256([0u8; 32]),
        };
        reg.create_client("frozen-client".to_string(), client_state, consensus).unwrap();

        let validators = make_validators(5);
        let result = reg.update_client("frozen-client", make_header(10), 10_000_000_000, &validators);
        assert!(matches!(result, Err(IbcClientError::ClientFrozen(_))));
    }

    #[test]
    fn misbehaviour_freezes_client() {
        let mut reg = IbcClientRegistry::new();
        let client_state = ZbxClientState::mainnet_default();
        let consensus = ZbxConsensusState {
            timestamp_ns: 1_000_000_000,
            root: H256([0u8; 32]),
            next_validators_hash: H256([0u8; 32]),
        };
        reg.create_client("test".to_string(), client_state, consensus).unwrap();

        let mut h1 = make_header(5);
        let mut h2 = make_header(5);
        h2.state_root = H256([1u8; 32]); // conflicting state root

        let mb = ZbxMisbehaviour {
            client_id: "test".to_string(),
            header_1: h1,
            header_2: h2,
        };
        reg.submit_misbehaviour(mb).unwrap();
        assert!(reg.get_client("test").unwrap().is_frozen());
    }

    #[test]
    fn ibc_height_ordering() {
        assert!(IbcHeight::new(10) > IbcHeight::new(5));
        assert_eq!(IbcHeight::new(5), IbcHeight::new(5));
    }

    #[test]
    fn fraction_trust_level() {
        let one_third = Fraction::ONE_THIRD;
        assert!(one_third.is_satisfied_by(34, 100));  // 34% > 33.3%
        assert!(!one_third.is_satisfied_by(33, 100)); // 33% < 33.3%
    }
}
