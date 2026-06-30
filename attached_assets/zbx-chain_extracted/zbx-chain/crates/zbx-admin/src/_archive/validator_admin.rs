//! Validator administration -- override set, slots, whitelist, jail.
//!
//! Operator role actions:
//!   override_validator_set() -- emergency replace full validator set
//!   set_validator_slots()    -- change number of active slots (default 21)
//!   whitelist_validator()    -- add validator to trusted IP whitelist
//!
//! These are powerful ops, all gated behind Operator role + timelock.

use std::collections::{HashSet, HashMap};

// ── Validator admin ───────────────────────────────────────────────────────────

pub struct ValidatorAdmin {
    /// Active validator set (addresses)
    pub active_set:       Vec<[u8; 20]>,
    /// Number of active validator slots
    pub validator_slots:  u32,
    /// Whitelisted validator IPs (for trusted validator infrastructure)
    pub ip_whitelist:     HashMap<[u8; 20], Vec<std::net::IpAddr>>,
    /// Jailed validators (cannot produce blocks, lose rewards)
    pub jailed:           HashSet<[u8; 20]>,
    /// Override history (for audit)
    pub override_history: Vec<ValidatorSetOverride>,
}

#[derive(Debug, Clone)]
pub struct ValidatorSetOverride {
    pub block:    u64,
    pub old_set:  Vec<[u8; 20]>,
    pub new_set:  Vec<[u8; 20]>,
    pub reason:   String,
    pub by:       [u8; 20],
}

impl ValidatorAdmin {
    pub fn new(initial_set: Vec<[u8; 20]>) -> Self {
        let slots = initial_set.len() as u32;
        Self {
            active_set:       initial_set,
            validator_slots:  slots.min(21),
            ip_whitelist:     HashMap::new(),
            jailed:           HashSet::new(),
            override_history: Vec::new(),
        }
    }

    /// EMERGENCY: Override the entire validator set.
    /// Used only in extreme circumstances (mass validator failure, attack).
    /// Requires Operator role + 48h timelock + multi-sig.
    /// Emits AdminEvent::ValidatorSetOverridden.
    pub fn override_validator_set(
        &mut self,
        caller:    [u8; 20],
        new_set:   Vec<[u8; 20]>,
        reason:    &str,
        block:     u64,
    ) -> Result<ValidatorSetOverride, ValidatorAdminError> {
        if new_set.len() < 4 {
            return Err(ValidatorAdminError::TooFewValidators);
        }
        let rec = ValidatorSetOverride {
            block,
            old_set: self.active_set.clone(),
            new_set: new_set.clone(),
            reason:  reason.into(),
            by:      caller,
        };
        self.active_set = new_set;
        self.override_history.push(rec.clone());
        Ok(rec)
    }

    /// Set the number of active validator slots.
    /// Controls how many validators can produce blocks per epoch.
    /// Emits AdminEvent::ValidatorSlotsChanged.
    pub fn set_validator_slots(&mut self, caller: [u8; 20], new_slots: u32) -> Result<u32, ValidatorAdminError> {
        if new_slots < 4 { return Err(ValidatorAdminError::TooFewSlots); }
        if new_slots > 200 { return Err(ValidatorAdminError::TooManySlots); }
        let old = self.validator_slots;
        self.validator_slots = new_slots;
        Ok(old)
    }

    /// Whitelist a validator's IP addresses.
    /// Whitelisted validators bypass IP-based rate limiting.
    /// Emits AdminEvent::ValidatorWhitelisted.
    pub fn whitelist_validator(
        &mut self,
        validator: [u8; 20],
        ips:       Vec<std::net::IpAddr>,
    ) {
        self.ip_whitelist.insert(validator, ips);
    }

    /// Remove a validator from the IP whitelist.
    pub fn remove_whitelist(&mut self, validator: [u8; 20]) {
        self.ip_whitelist.remove(&validator);
    }

    /// Check if a validator's IP is whitelisted.
    pub fn is_whitelisted(&self, validator: [u8; 20], ip: std::net::IpAddr) -> bool {
        self.ip_whitelist.get(&validator).map(|ips| ips.contains(&ip)).unwrap_or(false)
    }

    /// Jail a validator (double-sign or downtime).
    pub fn jail_validator(&mut self, validator: [u8; 20]) {
        self.jailed.insert(validator);
        self.active_set.retain(|v| *v != validator);
    }

    /// Unjail a validator after serving jail time.
    pub fn unjail_validator(&mut self, validator: [u8; 20]) {
        self.jailed.remove(&validator);
    }

    /// Force-add a validator to the active set (Operator role).
    pub fn add_validator(&mut self, validator: [u8; 20]) -> Result<(), ValidatorAdminError> {
        if self.active_set.len() as u32 >= self.validator_slots {
            return Err(ValidatorAdminError::SlotsFull);
        }
        if !self.active_set.contains(&validator) {
            self.active_set.push(validator);
        }
        Ok(())
    }

    /// Force-remove a validator from the active set.
    pub fn remove_validator(&mut self, validator: [u8; 20]) {
        self.active_set.retain(|v| *v != validator);
    }
}

#[derive(Debug)]
pub enum ValidatorAdminError {
    TooFewValidators,
    TooFewSlots,
    TooManySlots,
    SlotsFull,
    AlreadyJailed,
    NotJailed,
}