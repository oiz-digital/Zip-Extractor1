//! IDO pool — manages a single token launch pool.
//!
//! ## Security fix LAUNCH-02
//! `contribute()` previously checked only `status == Active`, which allowed
//! contributions to an expired pool (past `end_time`) as long as the status
//! field had not yet been flipped.  The guard now uses `is_active(now)` which
//! additionally checks `start_time <= now < end_time`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PoolStatus {
    Pending,
    Active,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pool {
    pub id:              u64,
    pub token_address:   [u8; 20],
    pub soft_cap:        u128,
    pub hard_cap:        u128,
    pub price_per_token: u128,
    pub start_time:      u64,
    pub end_time:        u64,
    pub total_raised:    u128,
    pub status:          PoolStatus,
}

impl Pool {
    pub fn new(
        id: u64,
        token_address: [u8; 20],
        soft_cap: u128,
        hard_cap: u128,
        price_per_token: u128,
        start_time: u64,
        end_time: u64,
    ) -> Self {
        Self {
            id,
            token_address,
            soft_cap,
            hard_cap,
            price_per_token,
            start_time,
            end_time,
            total_raised: 0,
            status: PoolStatus::Pending,
        }
    }

    pub fn is_active(&self, now: u64) -> bool {
        self.status == PoolStatus::Active
            && now >= self.start_time
            && now < self.end_time
    }

    pub fn is_successful(&self) -> bool {
        self.total_raised >= self.soft_cap
    }

    /// Accept a contribution of `amount` at block timestamp `now`.
    ///
    /// ## LAUNCH-02 fix
    /// Now uses `is_active(now)` which checks **both** `status == Active` AND
    /// `start_time <= now < end_time`.  Previously only the status field was
    /// checked, so an expired pool (past `end_time`) that hadn't been finalised
    /// yet could still receive contributions.
    pub fn contribute(&mut self, amount: u128, now: u64) -> anyhow::Result<()> {
        if !self.is_active(now) {
            anyhow::bail!("Pool is not open for contributions");
        }
        if self.total_raised + amount > self.hard_cap {
            anyhow::bail!("Contribution exceeds hard cap");
        }
        self.total_raised += amount;
        Ok(())
    }

    pub fn finalize(&mut self) {
        if self.is_successful() {
            self.status = PoolStatus::Completed;
        } else {
            self.status = PoolStatus::Cancelled;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_pool(start: u64, end: u64) -> Pool {
        let mut p = Pool::new(1, [0u8; 20], 1_000, 10_000, 1, start, end);
        p.status = PoolStatus::Active;
        p
    }

    #[test]
    fn contribute_within_window_succeeds() {
        let mut p = active_pool(100, 200);
        p.contribute(500, 150).unwrap();
        assert_eq!(p.total_raised, 500);
    }

    /// LAUNCH-02: contribution before start_time must be rejected.
    #[test]
    fn contribute_before_start_rejected() {
        let mut p = active_pool(100, 200);
        let err = p.contribute(500, 50).unwrap_err();
        assert!(err.to_string().contains("not open"));
    }

    /// LAUNCH-02: contribution at or after end_time must be rejected.
    #[test]
    fn contribute_after_end_rejected() {
        let mut p = active_pool(100, 200);
        let err = p.contribute(500, 200).unwrap_err();
        assert!(err.to_string().contains("not open"));
    }

    /// LAUNCH-02: expired-but-still-Active pool must reject contributions.
    #[test]
    fn contribute_expired_active_pool_rejected() {
        let mut p = active_pool(100, 200);
        // Pool is still Active but end_time has passed.
        let err = p.contribute(500, 999).unwrap_err();
        assert!(err.to_string().contains("not open"));
    }

    #[test]
    fn contribute_pending_pool_rejected() {
        let mut p = Pool::new(1, [0u8; 20], 1_000, 10_000, 1, 100, 200);
        // status = Pending (default)
        let err = p.contribute(500, 150).unwrap_err();
        assert!(err.to_string().contains("not open"));
    }

    #[test]
    fn contribute_hard_cap_enforced() {
        let mut p = active_pool(100, 200);
        let err = p.contribute(10_001, 150).unwrap_err();
        assert!(err.to_string().contains("hard cap"));
    }

    #[test]
    fn finalize_completed_when_soft_cap_met() {
        let mut p = active_pool(100, 200);
        p.contribute(1_000, 150).unwrap();
        p.finalize();
        assert_eq!(p.status, PoolStatus::Completed);
    }

    #[test]
    fn finalize_cancelled_when_soft_cap_not_met() {
        let mut p = active_pool(100, 200);
        p.finalize();
        assert_eq!(p.status, PoolStatus::Cancelled);
    }
}
