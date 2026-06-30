//! Bridge validator set -- MPC threshold signing for cross-chain messages.
//!
//! ZBX bridge uses a threshold multi-signature scheme (t-of-n MPC):
//!   - N bridge validators co-manage the bridge
//!   - Threshold T (e.g. 5-of-9) must sign to authorize unlock
//!   - Validators are selected from ZBX active validators + bridge council
//!   - Validator set rotates every BRIDGE_EPOCH epochs
//!
//! ## Trusted relayer whitelist
//!   Relayers are entities that submit cross-chain proofs to the bridge.
//!   Only whitelisted relayers can call unlock_tokens().
//!   Relayers are not the same as bridge validators:
//!     Validators: sign the bridge message (off-chain MPC)
//!     Relayers:   submit the signed message on-chain
//!   Initially centralized (Zebvix Foundation relayers).
//!   Decentralized relayer set in v2.
//!
//! ## Signature aggregation
//!   Each validator submits an ECDSA signature over:
//!     hash(chain_id ++ nonce ++ token ++ amount ++ recipient ++ dest_chain)
//!   Signatures are collected off-chain and submitted together.
//!   On-chain verifies T signatures >= threshold before unlocking.

use std::collections::{HashMap, HashSet};

// ── Bridge validator set ──────────────────────────────────────────────────────

/// A bridge validator (one of N co-signers for cross-chain messages).
#[derive(Debug, Clone)]
pub struct BridgeValidator {
    pub address:   [u8; 20],
    pub bls_pubkey: [u8; 48],   // BLS pubkey for aggregate signing
    pub power:     u64,          // Voting power (stake-weighted)
    pub active:    bool,
}

/// Bridge validator set (changes at BRIDGE_EPOCH boundary).
pub struct BridgeValidatorSet {
    pub validators:      Vec<BridgeValidator>,
    pub threshold:       u32,   // Minimum signatures required (e.g. 5 of 9)
    pub total_power:     u64,   // Sum of all validator powers
    pub epoch:           u64,   // Bridge epoch this set was activated
    pub set_hash:        [u8; 32], // Hash of this validator set (for slashing)
}

impl BridgeValidatorSet {
    pub fn new(validators: Vec<BridgeValidator>, threshold: u32, epoch: u64) -> Self {
        let total_power: u64 = validators.iter().map(|v| v.power).sum();
        let set_hash = compute_validator_set_hash(&validators);
        Self { validators, threshold, total_power, epoch, set_hash }
    }

    /// Check if enough validators signed (by index + power).
    pub fn has_threshold(&self, signer_indices: &[usize]) -> bool {
        let power: u64 = signer_indices.iter()
            .filter_map(|&i| self.validators.get(i))
            .filter(|v| v.active)
            .map(|v| v.power)
            .sum();
        // Threshold: 2/3+ of total power (OR fixed N-of-M count)
        power * 3 >= self.total_power * 2 || signer_indices.len() >= self.threshold as usize
    }

    pub fn validator_count(&self) -> usize { self.validators.len() }

    pub fn get_validator(&self, addr: &[u8; 20]) -> Option<&BridgeValidator> {
        self.validators.iter().find(|v| &v.address == addr)
    }

    /// Add a new validator (bridge admin action, takes effect next epoch).
    pub fn add_validator(&mut self, v: BridgeValidator) {
        if !self.validators.iter().any(|x| x.address == v.address) {
            self.total_power += v.power;
            self.validators.push(v);
        }
    }

    /// Remove a validator (slashed or resigned).
    pub fn remove_validator(&mut self, addr: &[u8; 20]) {
        if let Some(pos) = self.validators.iter().position(|v| &v.address == addr) {
            self.total_power -= self.validators[pos].power;
            self.validators.remove(pos);
        }
    }
}

// ── Relayer whitelist ─────────────────────────────────────────────────────────

/// Trusted relayer whitelist (allowed_relayer).
/// Only whitelisted relayers can submit cross-chain messages to bridge.
pub struct RelayerWhitelist {
    /// Set of trusted relayer addresses
    pub trusted_relayers: HashSet<[u8; 20]>,
    /// Relayer metadata: address -> RelayerInfo
    pub relayer_info:     HashMap<[u8; 20], RelayerInfo>,
}

#[derive(Debug, Clone)]
pub struct RelayerInfo {
    pub address:     [u8; 20],
    pub name:        String,     // e.g. "Zebvix Foundation Relayer 1"
    pub added_at:    u64,        // block number when added
    pub tx_count:    u64,        // total relay tx submitted
    pub slash_count: u32,        // times slashed for malicious relay
    pub is_active:   bool,
}

impl RelayerWhitelist {
    pub fn new() -> Self {
        Self { trusted_relayers: HashSet::new(), relayer_info: HashMap::new() }
    }

    /// Add a trusted relayer (bridge admin only).
    pub fn add_relayer(&mut self, info: RelayerInfo) {
        self.trusted_relayers.insert(info.address);
        self.relayer_info.insert(info.address, info);
    }

    /// Remove a relayer from whitelist (slash or voluntary exit).
    pub fn remove_relayer(&mut self, addr: &[u8; 20]) {
        self.trusted_relayers.remove(addr);
        if let Some(info) = self.relayer_info.get_mut(addr) {
            info.is_active = false;
        }
    }

    pub fn is_trusted(&self, addr: &[u8; 20]) -> bool {
        self.trusted_relayers.contains(addr)
    }

    pub fn relayer_count(&self) -> usize { self.trusted_relayers.len() }
}

fn compute_validator_set_hash(validators: &[BridgeValidator]) -> [u8; 32] {
    let _ = validators;
    [0u8; 32] // stub: keccak256 of sorted validator addresses + powers
}