//! GaugeController — vote-weighted reward allocation across LP farms.
//!
//! Inspired by Curve's gauge controller (veToken model).
//! Users lock ZBX to receive voting power (ve-ZBX, proportional to lock duration).
//! Each epoch (1 week), gauge weights are computed from votes → rewards split.
//!
//! Security:
//! * Votes are bounded by the voter's ve-ZBX balance (no over-voting)
//! * Gauge weight is recomputed each epoch — stale votes don't persist forever
//! * 10,000 bps total across all gauges — no rounding escape
//! * Max lock 4 years; min lock 1 week

use std::collections::HashMap;
use zbx_types::address::Address;

/// Epoch length in seconds (1 week).
pub const EPOCH_SECS: u64 = 7 * 24 * 3600;
/// Maximum lock duration in seconds (4 years).
pub const MAX_LOCK_SECS: u64 = 4 * 365 * 24 * 3600;
/// Minimum lock duration (1 week).
pub const MIN_LOCK_SECS: u64 = EPOCH_SECS;

/// Gauge controller error.
#[derive(Debug, thiserror::Error)]
pub enum GaugeError {
    #[error("gauge {0} not found")]
    GaugeNotFound(u32),
    #[error("lock duration {got}s below minimum {min}s")]
    LockTooShort { got: u64, min: u64 },
    #[error("lock duration {got}s exceeds maximum {max}s")]
    LockTooLong { got: u64, max: u64 },
    #[error("user has no ve-ZBX lock")]
    NoLock,
    #[error("vote allocation {total_bps} bps exceeds 10 000")]
    AllocationOverflow { total_bps: u32 },
    #[error("lock expired — re-lock to vote")]
    LockExpired,
}

/// A registered gauge (one LP pool / farm).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Gauge {
    pub id:   u32,
    pub name: String,
    /// Weight in bps this epoch (set by vote tally).
    pub weight_bps: u32,
}

/// A user's ve-ZBX lock.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VeLock {
    /// Locked ZBX amount (base units).
    pub amount:     u128,
    /// Lock expiry (Unix timestamp).
    pub unlock_at:  u64,
}

impl VeLock {
    /// Voting power = amount × remaining_secs / MAX_LOCK_SECS (linear decay).
    pub fn voting_power(&self, now: u64) -> u128 {
        if now >= self.unlock_at { return 0; }
        let remaining = self.unlock_at - now;
        self.amount * (remaining as u128) / (MAX_LOCK_SECS as u128)
    }
}

/// Per-epoch vote allocation from one user: gauge_id → bps.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct VoteAllocation {
    pub allocations: HashMap<u32, u32>,
}

/// GaugeController — manages gauges, ve-locks, and epoch weight computation.
#[derive(Debug, Default)]
pub struct GaugeController {
    gauges:      HashMap<u32, Gauge>,
    locks:       HashMap<Address, VeLock>,
    /// Current epoch's vote allocations per user.
    votes:       HashMap<Address, VoteAllocation>,
    pub current_epoch: u64,
}

impl GaugeController {
    pub fn new() -> Self { Self::default() }

    // ── Gauge management ───────────────────────────────────────────────────

    pub fn add_gauge(&mut self, id: u32, name: String) {
        self.gauges.insert(id, Gauge { id, name, weight_bps: 0 });
    }

    pub fn remove_gauge(&mut self, id: u32) {
        self.gauges.remove(&id);
    }

    // ── ve-ZBX locking ────────────────────────────────────────────────────

    /// Lock `amount` ZBX for `duration_secs`. Returns the voting power granted.
    pub fn lock(
        &mut self,
        user:         Address,
        amount:       u128,
        duration_secs: u64,
        now:          u64,
    ) -> Result<u128, GaugeError> {
        if duration_secs < MIN_LOCK_SECS {
            return Err(GaugeError::LockTooShort { got: duration_secs, min: MIN_LOCK_SECS });
        }
        if duration_secs > MAX_LOCK_SECS {
            return Err(GaugeError::LockTooLong { got: duration_secs, max: MAX_LOCK_SECS });
        }
        let lock = VeLock { amount, unlock_at: now + duration_secs };
        let power = lock.voting_power(now);
        self.locks.insert(user, lock);
        Ok(power)
    }

    /// Increase lock duration (extends unlock_at, not amount).
    pub fn extend_lock(
        &mut self,
        user:         Address,
        extra_secs:   u64,
        now:          u64,
    ) -> Result<u128, GaugeError> {
        let lock = self.locks.get_mut(&user).ok_or(GaugeError::NoLock)?;
        let new_unlock = lock.unlock_at.max(now) + extra_secs;
        if new_unlock - now > MAX_LOCK_SECS {
            return Err(GaugeError::LockTooLong { got: new_unlock - now, max: MAX_LOCK_SECS });
        }
        lock.unlock_at = new_unlock;
        Ok(lock.voting_power(now))
    }

    /// Withdraw unlocked ZBX after lock expires.
    pub fn unlock(&mut self, user: Address, now: u64) -> Result<u128, GaugeError> {
        let lock = self.locks.get(&user).ok_or(GaugeError::NoLock)?;
        if now < lock.unlock_at {
            return Err(GaugeError::LockExpired); // "lock still active" semantically
        }
        let amount = lock.amount;
        self.locks.remove(&user);
        self.votes.remove(&user);
        Ok(amount)
    }

    // ── Voting ─────────────────────────────────────────────────────────────

    /// Cast votes: `allocations` maps gauge_id → bps (must sum to ≤ 10 000).
    pub fn vote(
        &mut self,
        user:        Address,
        allocations: HashMap<u32, u32>,
        now:         u64,
    ) -> Result<(), GaugeError> {
        let lock = self.locks.get(&user).ok_or(GaugeError::NoLock)?;
        if now >= lock.unlock_at { return Err(GaugeError::LockExpired); }

        let total_bps: u32 = allocations.values().sum();
        if total_bps > 10_000 {
            return Err(GaugeError::AllocationOverflow { total_bps });
        }
        for gid in allocations.keys() {
            if !self.gauges.contains_key(gid) {
                return Err(GaugeError::GaugeNotFound(*gid));
            }
        }
        self.votes.insert(user, VoteAllocation { allocations });
        Ok(())
    }

    // ── Epoch settlement ───────────────────────────────────────────────────

    /// Advance to the next epoch and compute new gauge weights from current votes.
    /// Returns the new weight_bps for each gauge.
    pub fn advance_epoch(&mut self, now: u64) -> HashMap<u32, u32> {
        self.current_epoch += 1;

        // Tally weighted votes: gauge_id → total ve-ZBX-weighted bps.
        let mut tally: HashMap<u32, u128> = HashMap::new();
        let mut total_power: u128 = 0;

        for (user, alloc) in &self.votes {
            let power = self.locks.get(user)
                .map(|l| l.voting_power(now))
                .unwrap_or(0);
            if power == 0 { continue; }
            total_power += power;
            for (&gauge_id, &bps) in &alloc.allocations {
                *tally.entry(gauge_id).or_default() +=
                    power * (bps as u128) / 10_000;
            }
        }

        // Convert tallied power to bps weight.
        let mut result = HashMap::new();
        for (_, gauge) in &mut self.gauges {
            let raw = tally.get(&gauge.id).copied().unwrap_or(0);
            let weight_bps = if total_power == 0 {
                0
            } else {
                ((raw * 10_000 / total_power) as u32).min(10_000)
            };
            gauge.weight_bps = weight_bps;
            result.insert(gauge.id, weight_bps);
        }
        result
    }

    pub fn voting_power(&self, user: &Address, now: u64) -> u128 {
        self.locks.get(user).map(|l| l.voting_power(now)).unwrap_or(0)
    }

    pub fn gauge(&self, id: u32) -> Option<&Gauge> { self.gauges.get(&id) }
    pub fn lock_info(&self, user: &Address) -> Option<&VeLock> { self.locks.get(user) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(v: u8) -> Address { Address([v; 20]) }

    #[test]
    fn lock_gives_voting_power() {
        let mut gc = GaugeController::new();
        let power = gc.lock(addr(1), 1_000_000, MAX_LOCK_SECS, 0).unwrap();
        assert_eq!(power, 1_000_000); // max lock = full amount
    }

    #[test]
    fn voting_power_decays_with_time() {
        let mut gc = GaugeController::new();
        gc.lock(addr(1), 1_000_000, MAX_LOCK_SECS, 0).unwrap();
        let p_now  = gc.voting_power(&addr(1), 0);
        let p_half = gc.voting_power(&addr(1), MAX_LOCK_SECS / 2);
        assert!(p_half < p_now);
        assert!(p_half > 0);
    }

    #[test]
    fn vote_and_epoch_weights() {
        let mut gc = GaugeController::new();
        gc.add_gauge(0, "ZBX/ZUSD".into());
        gc.add_gauge(1, "ZBX/ETH".into());
        gc.lock(addr(1), 1_000_000, MAX_LOCK_SECS, 0).unwrap();
        gc.lock(addr(2), 1_000_000, MAX_LOCK_SECS, 0).unwrap();
        let mut alloc1 = HashMap::new();
        alloc1.insert(0, 6_000); // 60% to gauge 0
        alloc1.insert(1, 4_000); // 40% to gauge 1
        gc.vote(addr(1), alloc1, 0).unwrap();
        let mut alloc2 = HashMap::new();
        alloc2.insert(0, 4_000);
        alloc2.insert(1, 6_000);
        gc.vote(addr(2), alloc2, 0).unwrap();
        let weights = gc.advance_epoch(0);
        // Both users equal power → 50/50 split expected
        assert_eq!(weights[&0], weights[&1]);
    }

    #[test]
    fn over_allocation_rejected() {
        let mut gc = GaugeController::new();
        gc.add_gauge(0, "LP".into());
        gc.lock(addr(1), 1_000_000, MAX_LOCK_SECS, 0).unwrap();
        let mut alloc = HashMap::new();
        alloc.insert(0, 10_001u32);
        assert!(matches!(gc.vote(addr(1), alloc, 0), Err(GaugeError::AllocationOverflow { .. })));
    }

    #[test]
    fn cannot_vote_without_lock() {
        let mut gc = GaugeController::new();
        gc.add_gauge(0, "LP".into());
        let alloc = HashMap::from([(0, 10_000u32)]);
        assert!(matches!(gc.vote(addr(1), alloc, 0), Err(GaugeError::NoLock)));
    }

    #[test]
    fn cannot_vote_on_unknown_gauge() {
        let mut gc = GaugeController::new();
        gc.lock(addr(1), 1_000_000, MAX_LOCK_SECS, 0).unwrap();
        let alloc = HashMap::from([(99, 5_000u32)]);
        assert!(matches!(gc.vote(addr(1), alloc, 0), Err(GaugeError::GaugeNotFound(99))));
    }

    #[test]
    fn lock_duration_bounds_enforced() {
        let mut gc = GaugeController::new();
        assert!(matches!(
            gc.lock(addr(1), 1000, MIN_LOCK_SECS - 1, 0),
            Err(GaugeError::LockTooShort { .. })
        ));
        assert!(matches!(
            gc.lock(addr(1), 1000, MAX_LOCK_SECS + 1, 0),
            Err(GaugeError::LockTooLong { .. })
        ));
    }

    #[test]
    fn unlock_after_expiry() {
        let mut gc = GaugeController::new();
        gc.lock(addr(1), 5_000, MIN_LOCK_SECS, 0).unwrap();
        let returned = gc.unlock(addr(1), MIN_LOCK_SECS + 1).unwrap();
        assert_eq!(returned, 5_000);
    }
}
