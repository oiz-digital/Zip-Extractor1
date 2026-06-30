//! Vesting schedules for token distribution.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VestingSchedule {
    pub beneficiary:    [u8; 20],
    pub total_amount:   u128,
    pub claimed:        u128,
    pub cliff_time:     u64,  // Unix timestamp — no tokens before this
    pub start_time:     u64,  // Vesting starts after cliff
    pub duration:       u64,  // Seconds over which tokens unlock
}

impl VestingSchedule {
    pub fn new(
        beneficiary: [u8; 20],
        total_amount: u128,
        cliff_time: u64,
        start_time: u64,
        duration: u64,
    ) -> Self {
        Self { beneficiary, total_amount, claimed: 0, cliff_time, start_time, duration }
    }

    /// Tokens unlocked at `now` (before subtracting already claimed).
    pub fn vested_at(&self, now: u64) -> u128 {
        if now < self.cliff_time { return 0; }
        if self.duration == 0   { return self.total_amount; }
        let elapsed = now.saturating_sub(self.start_time);
        if elapsed >= self.duration {
            self.total_amount
        } else {
            self.total_amount * elapsed as u128 / self.duration as u128
        }
    }

    /// Tokens available to claim right now.
    pub fn claimable(&self, now: u64) -> u128 {
        self.vested_at(now).saturating_sub(self.claimed)
    }

    /// Claim vested tokens. Returns the amount claimed.
    pub fn claim(&mut self, now: u64) -> anyhow::Result<u128> {
        let amount = self.claimable(now);
        if amount == 0 { anyhow::bail!("Nothing to claim yet"); }
        self.claimed += amount;
        Ok(amount)
    }
}
