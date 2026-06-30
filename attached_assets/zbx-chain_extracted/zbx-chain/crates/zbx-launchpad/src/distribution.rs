//! Token distribution after a successful IDO pool.

use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use crate::vesting::VestingSchedule;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contribution {
    pub contributor: [u8; 20],
    pub amount:      u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Distribution {
    pub pool_id:       u64,
    pub token_address: [u8; 20],
    pub contributions: Vec<Contribution>,
    pub schedules:     HashMap<[u8; 20], VestingSchedule>,
}

impl Distribution {
    pub fn new(pool_id: u64, token_address: [u8; 20]) -> Self {
        Self { pool_id, token_address, contributions: Vec::new(), schedules: HashMap::new() }
    }

    pub fn record_contribution(&mut self, contributor: [u8; 20], amount: u128) {
        self.contributions.push(Contribution { contributor, amount });
    }

    pub fn total_raised(&self) -> u128 {
        self.contributions.iter().map(|c| c.amount).sum()
    }

    /// Create vesting schedules for all contributors based on their pro-rata share.
    pub fn create_schedules(
        &mut self,
        total_tokens: u128,
        cliff_time: u64,
        start_time: u64,
        duration: u64,
    ) {
        let total = self.total_raised();
        if total == 0 { return; }
        for c in &self.contributions {
            let tokens = total_tokens * c.amount / total;
            let sched = VestingSchedule::new(c.contributor, tokens, cliff_time, start_time, duration);
            self.schedules.entry(c.contributor).or_insert(sched);
        }
    }

    pub fn claimable_for(&self, addr: &[u8; 20], now: u64) -> u128 {
        self.schedules.get(addr).map(|s| s.claimable(now)).unwrap_or(0)
    }

    pub fn claim(&mut self, addr: &[u8; 20], now: u64) -> anyhow::Result<u128> {
        match self.schedules.get_mut(addr) {
            Some(s) => s.claim(now),
            None    => anyhow::bail!("No schedule for this address"),
        }
    }
}
