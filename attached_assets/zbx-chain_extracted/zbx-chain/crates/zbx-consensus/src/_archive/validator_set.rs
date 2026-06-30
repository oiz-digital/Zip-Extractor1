//! Validator set management.

use serde::{Deserialize, Serialize};

/// A single validator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Validator {
    pub address:      [u8; 20],
    pub public_key:   [u8; 32],   // Ed25519
    pub voting_power: u64,        // proportional to stake
    pub active:       bool,
}

impl Validator {
    pub fn address_hex(&self) -> String {
        hex::encode(self.address)
    }
}

/// Ordered set of active validators.
#[derive(Debug, Clone)]
pub struct ValidatorSet {
    validators: Vec<Validator>,
    total_power: u64,
}

impl ValidatorSet {
    pub fn new(validators: Vec<Validator>) -> Self {
        let total_power = validators.iter().map(|v| v.voting_power).sum();
        Self { validators, total_power }
    }

    pub fn get(&self, index: usize) -> &Validator {
        &self.validators[index % self.validators.len()]
    }

    pub fn len(&self) -> usize { self.validators.len() }
    pub fn is_empty(&self) -> bool { self.validators.is_empty() }
    pub fn total_power(&self) -> u64 { self.total_power }

    /// Minimum voting power for a quorum (2/3+).
    pub fn quorum_power(&self) -> u64 {
        self.total_power * 2 / 3 + 1
    }

    pub fn iter(&self) -> impl Iterator<Item = &Validator> {
        self.validators.iter()
    }
}