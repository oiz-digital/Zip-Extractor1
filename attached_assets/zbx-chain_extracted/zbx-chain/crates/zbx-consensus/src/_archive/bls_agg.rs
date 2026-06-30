//! BLS signature aggregation for ZBX consensus.
//! Used for attestation aggregation, sync committee, and proposer signing.

use std::collections::HashMap;
use crate::types::{ValidatorIndex, Epoch};
use crate::crypto::bls::{BLSPublicKey, BLSSignature, AggregateSignature};

/// Attestation aggregator
pub struct AttestationAggregator {
    /// Aggregated signatures per committee slot
    pub aggregates: HashMap<(Epoch, u64), AggregateEntry>,
    /// Individual attestations not yet aggregated
    pub singles: Vec<SingleAttestation>,
    /// Maximum aggregate size
    pub max_aggregate_size: usize,
}

/// Aggregated entry
#[derive(Debug, Clone)]
pub struct AggregateEntry {
    pub epoch: Epoch,
    pub committee_index: u64,
    pub participants: Vec<ValidatorIndex>,
    pub pub_keys: Vec<BLSPublicKey>,
    pub aggregate_sig: Option<AggregateSignature>,
    pub bits: Vec<bool>,
    pub count: usize,
}

/// Single (non-aggregated) attestation
#[derive(Debug, Clone)]
pub struct SingleAttestation {
    pub validator_index: ValidatorIndex,
    pub epoch: Epoch,
    pub committee_index: u64,
    pub pub_key: BLSPublicKey,
    pub signature: BLSSignature,
    pub target_root: [u8; 32],
    pub source_root: [u8; 32],
}

impl AttestationAggregator {
    pub fn new(max_aggregate_size: usize) -> Self {
        Self { aggregates: HashMap::new(), singles: Vec::new(), max_aggregate_size }
    }

    /// Add an attestation to be aggregated
    pub fn add(&mut self, att: SingleAttestation) -> Result<Option<AggregateEntry>, AggError> {
        let key = (att.epoch, att.committee_index);
        let entry = self.aggregates.entry(key).or_insert_with(|| AggregateEntry {
            epoch: att.epoch,
            committee_index: att.committee_index,
            participants: Vec::new(),
            pub_keys: Vec::new(),
            aggregate_sig: None,
            bits: Vec::new(),
            count: 0,
        });

        // Check duplicate
        if entry.participants.contains(&att.validator_index) {
            return Err(AggError::Duplicate(att.validator_index));
        }

        entry.participants.push(att.validator_index);
        entry.pub_keys.push(att.pub_key.clone());
        entry.bits.push(true);
        entry.count += 1;

        // Aggregate signature
        if let Some(ref mut agg_sig) = entry.aggregate_sig {
            agg_sig.add(&att.signature)?;
        } else {
            entry.aggregate_sig = Some(AggregateSignature::from_single(&att.signature));
        }

        if entry.count >= self.max_aggregate_size {
            return Ok(Some(entry.clone()));
        }
        Ok(None)
    }

    /// Flush all complete aggregates
    pub fn flush(&mut self) -> Vec<AggregateEntry> {
        let complete: Vec<_> = self.aggregates.iter()
            .filter(|(_, e)| e.count >= 2)
            .map(|(k, e)| (*k, e.clone()))
            .collect();
        let mut result = Vec::new();
        for (k, entry) in complete {
            self.aggregates.remove(&k);
            result.push(entry);
        }
        result
    }

    /// Verify an aggregate signature
    pub fn verify_aggregate(&self, entry: &AggregateEntry, message: &[u8]) -> Result<bool, AggError> {
        let agg_sig = entry.aggregate_sig.as_ref().ok_or(AggError::NoSignature)?;
        Ok(agg_sig.fast_aggregate_verify(message, &entry.pub_keys)
            .map_err(|e| AggError::VerifyFailed(e.to_string()))?)
    }

    /// Prune old epochs
    pub fn prune(&mut self, min_epoch: Epoch) {
        self.aggregates.retain(|(epoch, _), _| *epoch >= min_epoch);
        self.singles.retain(|s| s.epoch >= min_epoch);
    }
}

/// Sync committee aggregator (512 validators per committee)
pub struct SyncCommitteeAggregator {
    pub period: u64,
    pub committee_pubkeys: Vec<BLSPublicKey>,
    pub contributions: HashMap<u64, SyncContribution>,
}

/// Sync committee contribution
#[derive(Debug, Clone)]
pub struct SyncContribution {
    pub slot: u64,
    pub subcommittee_index: u64,
    pub aggregation_bits: Vec<bool>,
    pub aggregate_sig: Option<AggregateSignature>,
    pub count: usize,
}

impl SyncCommitteeAggregator {
    pub fn new(period: u64, pubkeys: Vec<BLSPublicKey>) -> Self {
        Self { period, committee_pubkeys: pubkeys, contributions: HashMap::new() }
    }

    pub fn add_contribution(&mut self, slot: u64, subcommittee: u64, participant: usize, sig: BLSSignature) -> Result<(), AggError> {
        let contrib = self.contributions.entry(subcommittee).or_insert_with(|| SyncContribution {
            slot,
            subcommittee_index: subcommittee,
            aggregation_bits: vec![false; 64], // 512/8 sub-committees
            aggregate_sig: None,
            count: 0,
        });
        if contrib.aggregation_bits.get(participant).copied().unwrap_or(false) {
            return Err(AggError::Duplicate(participant as ValidatorIndex));
        }
        if let Some(i) = contrib.aggregation_bits.get_mut(participant) { *i = true; }
        contrib.count += 1;
        if let Some(ref mut agg) = contrib.aggregate_sig {
            agg.add(&sig)?;
        } else {
            contrib.aggregate_sig = Some(AggregateSignature::from_single(&sig));
        }
        Ok(())
    }

    pub fn get_aggregate(&self, subcommittee: u64) -> Option<&SyncContribution> {
        self.contributions.get(&subcommittee)
    }
}

/// Aggregation errors
#[derive(Debug, thiserror::Error)]
pub enum AggError {
    #[error("Duplicate validator: {0}")]
    Duplicate(ValidatorIndex),
    #[error("No signature")]
    NoSignature,
    #[error("Verify failed: {0}")]
    VerifyFailed(String),
    #[error("BLS error: {0}")]
    Bls(String),
}

impl From<crate::crypto::bls::BLSError> for AggError {
    fn from(e: crate::crypto::bls::BLSError) -> Self { AggError::Bls(e.to_string()) }
}