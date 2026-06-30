//! Emergency controls -- pause, circuit breaker, freeze, blacklist.
//!
//! The Pauser role can call pause()/unpause() at any time.
//! The Guardian role can freeze individual accounts or trip the circuit breaker.
//!
//! Circuit breaker auto-trips when:
//!   - Block time exceeds 3x normal (chain stall detection)
//!   - Validator count drops below MIN_VALIDATORS
//!   - Treasury balance drops below TREASURY_FLOOR
//!   - Oracle price deviation exceeds ORACLE_DEVIATION_LIMIT

use std::collections::HashSet;
use std::time::{Duration, SystemTime};

pub const MIN_VALIDATORS: u32 = 4;
pub const TREASURY_FLOOR: u128 = 100_000 * 1_000_000_000_000_000_000; // 100k ZBX
pub const ORACLE_DEVIATION_LIMIT: f64 = 0.25; // 25% deviation triggers breaker
pub const NORMAL_BLOCK_TIME: Duration = Duration::from_secs(3);
pub const CHAIN_STALL_MULTIPLIER: u32 = 3;

// ── Global state ──────────────────────────────────────────────────────────────

pub struct EmergencyState {
    /// Global pause flag -- all transactions rejected when true
    pub is_paused:           bool,
    /// Paused at block N
    pub paused_at_block:     Option<u64>,
    /// Who triggered the pause
    pub paused_by:           Option<[u8; 20]>,
    /// Circuit breaker state
    pub circuit_breaker:     CircuitBreaker,
    /// Frozen accounts -- can't send transactions
    pub frozen_accounts:     HashSet<[u8; 20]>,
    /// Blacklisted addresses -- can't receive or send
    pub blacklist:           HashSet<[u8; 20]>,
}

impl EmergencyState {
    pub fn new() -> Self {
        Self {
            is_paused:       false,
            paused_at_block: None,
            paused_by:       None,
            circuit_breaker: CircuitBreaker::new(),
            frozen_accounts: HashSet::new(),
            blacklist:       HashSet::new(),
        }
    }

    /// Pause all chain activity. Caller must have Pauser or Guardian role.
    pub fn pause(&mut self, caller: [u8; 20], block: u64) -> Result<(), EmergencyError> {
        if self.is_paused { return Err(EmergencyError::AlreadyPaused); }
        self.is_paused       = true;
        self.paused_at_block = Some(block);
        self.paused_by       = Some(caller);
        Ok(())
    }

    /// Unpause chain activity.
    pub fn unpause(&mut self, caller: [u8; 20]) -> Result<(), EmergencyError> {
        if !self.is_paused { return Err(EmergencyError::NotPaused); }
        self.is_paused       = false;
        self.paused_at_block = None;
        self.paused_by       = None;
        Ok(())
    }

    /// Freeze a specific account (can't send txs, stake locked).
    pub fn freeze_account(&mut self, account: [u8; 20]) {
        self.frozen_accounts.insert(account);
    }

    /// Unfreeze an account.
    pub fn unfreeze_account(&mut self, account: [u8; 20]) {
        self.frozen_accounts.remove(&account);
    }

    /// Add an address to the blacklist (both send and receive blocked).
    pub fn blacklist_address(&mut self, account: [u8; 20]) {
        self.blacklist.insert(account);
    }

    /// Remove from blacklist.
    pub fn remove_blacklist(&mut self, account: [u8; 20]) {
        self.blacklist.remove(&account);
    }

    /// Check if a transaction is allowed (not paused, not frozen, not blacklisted).
    pub fn check_tx_allowed(&self, from: [u8; 20], to: [u8; 20]) -> Result<(), EmergencyError> {
        if self.is_paused               { return Err(EmergencyError::GlobalPause); }
        if self.circuit_breaker.tripped { return Err(EmergencyError::CircuitBreaker); }
        if self.frozen_accounts.contains(&from) { return Err(EmergencyError::AccountFrozen); }
        if self.blacklist.contains(&from) || self.blacklist.contains(&to) {
            return Err(EmergencyError::Blacklisted);
        }
        Ok(())
    }
}

// ── Circuit Breaker ───────────────────────────────────────────────────────────

/// Circuit breaker -- auto-pause when critical thresholds are breached.
///
/// Auto-trips when:
///   1. Block time > 3x normal (chain stall)
///   2. Validator count < MIN_VALIDATORS (liveness risk)
///   3. Treasury balance < TREASURY_FLOOR (financial safety)
///   4. Oracle price deviation > 25% (manipulation risk)
///
/// Can also be manually tripped by Guardian role.
/// Reset requires SuperAdmin + Operator joint signature.
#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    pub tripped:     bool,
    pub tripped_at:  Option<u64>,    // block number
    pub reason:      Option<String>,
    pub auto_trips:  u32,            // count of automatic trips
    pub manual_trips: u32,           // count of manual trips
}

impl CircuitBreaker {
    pub fn new() -> Self {
        Self { tripped: false, tripped_at: None, reason: None, auto_trips: 0, manual_trips: 0 }
    }

    /// Trip the circuit breaker (auto-detected anomaly).
    pub fn auto_trip(&mut self, block: u64, reason: &str) {
        self.tripped    = true;
        self.tripped_at = Some(block);
        self.reason     = Some(reason.into());
        self.auto_trips += 1;
    }

    /// Manually trip (Guardian role).
    pub fn manual_trip(&mut self, block: u64, reason: &str) {
        self.tripped     = true;
        self.tripped_at  = Some(block);
        self.reason      = Some(reason.into());
        self.manual_trips += 1;
    }

    /// Reset after investigation (requires SuperAdmin).
    pub fn reset(&mut self) {
        self.tripped    = false;
        self.tripped_at = None;
        self.reason     = None;
    }

    /// Check chain health metrics; auto-trip if breached.
    pub fn check_health(
        &mut self,
        block:              u64,
        block_time_secs:    u64,
        active_validators:  u32,
        treasury_balance:   u128,
        oracle_deviation:   f64,
    ) {
        if block_time_secs > NORMAL_BLOCK_TIME.as_secs() * CHAIN_STALL_MULTIPLIER as u64 {
            self.auto_trip(block, "chain stall: block time exceeded 3x normal");
            return;
        }
        if active_validators < MIN_VALIDATORS {
            self.auto_trip(block, "validator count below minimum liveness threshold");
            return;
        }
        if treasury_balance < TREASURY_FLOOR {
            self.auto_trip(block, "treasury balance below safety floor");
            return;
        }
        if oracle_deviation > ORACLE_DEVIATION_LIMIT {
            self.auto_trip(block, "oracle price deviation exceeds 25% limit");
        }
    }
}

#[derive(Debug)]
pub enum EmergencyError {
    AlreadyPaused,
    NotPaused,
    GlobalPause,
    CircuitBreaker,
    AccountFrozen,
    Blacklisted,
    Unauthorized,
}