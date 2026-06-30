//! Rent scheduler — determines which accounts owe rent and schedules collection.

use std::collections::HashMap;
use crate::rent::{RentConfig, RentLedger, RentState};

/// Rent collection schedule for a block.
#[derive(Debug, Clone, Default)]
pub struct RentSchedule {
    pub block_number:   u64,
    pub accounts_due:   Vec<[u8; 20]>,
    pub total_rent_due: u128,
}

/// Rent scheduler — tracks which accounts need rent collected.
pub struct RentScheduler {
    pub config: RentConfig,
}

impl RentScheduler {
    pub fn new(config: RentConfig) -> Self {
        Self { config }
    }

    /// Compute the rent schedule for a given block, given a map of account rent states.
    pub fn schedule(
        &self,
        block_number: u64,
        accounts: &HashMap<[u8; 20], RentState>,
    ) -> RentSchedule {
        let mut schedule = RentSchedule { block_number, ..Default::default() };
        for (address, state) in accounts {
            let ledger = RentLedger::compute(state, &self.config, block_number);
            if ledger.due_wei > 0 {
                schedule.accounts_due.push(*address);
                schedule.total_rent_due = schedule.total_rent_due.saturating_add(ledger.due_wei);
            }
        }
        schedule
    }
}
