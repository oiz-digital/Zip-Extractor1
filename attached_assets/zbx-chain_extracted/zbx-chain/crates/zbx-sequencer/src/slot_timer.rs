//! Slot timer — tracks block production slots and signals when to propose.
//!
//! ZBX Chain has a fixed block time of 5 seconds.
//! The slot timer fires every 5 seconds and signals the proposer.

use std::time::{Duration, Instant};

/// Block slot timing configuration.
#[derive(Debug, Clone)]
pub struct SlotConfig {
    /// Block time in milliseconds (5000ms = 5s).
    pub block_time_ms:     u64,
    /// Time reserved for consensus voting (ms). Proposer must seal by deadline.
    pub seal_deadline_ms:  u64,
    /// Time reserved for network propagation (ms).
    pub propagation_ms:    u64,
}

impl Default for SlotConfig {
    fn default() -> Self {
        Self {
            block_time_ms:    5_000,
            seal_deadline_ms: 4_000, // 4s to build, 1s for consensus
            propagation_ms:     500,
        }
    }
}

/// Slot timer: tracks the current slot and deadline.
pub struct SlotTimer {
    config:    SlotConfig,
    genesis:   Instant,
    slot_zero: u64,   // block number at genesis
}

impl SlotTimer {
    pub fn new(config: SlotConfig, genesis: Instant, genesis_block: u64) -> Self {
        Self { config, genesis, slot_zero: genesis_block }
    }

    /// Current slot (block number).
    pub fn current_slot(&self) -> u64 {
        let elapsed_ms = self.genesis.elapsed().as_millis() as u64;
        self.slot_zero + elapsed_ms / self.config.block_time_ms
    }

    /// Milliseconds remaining in the current slot.
    pub fn ms_remaining(&self) -> u64 {
        let elapsed_ms = self.genesis.elapsed().as_millis() as u64;
        let slot_start = (elapsed_ms / self.config.block_time_ms) * self.config.block_time_ms;
        let ms_in_slot = elapsed_ms - slot_start;
        self.config.block_time_ms.saturating_sub(ms_in_slot)
    }

    /// Whether we are within the build window (before seal deadline).
    pub fn within_build_window(&self) -> bool {
        let elapsed_ms = self.genesis.elapsed().as_millis() as u64;
        let ms_in_slot = elapsed_ms % self.config.block_time_ms;
        ms_in_slot <= self.config.seal_deadline_ms
    }

    /// Wait until the next slot begins.
    pub async fn wait_for_next_slot(&self) {
        let ms = self.ms_remaining();
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn make_timer() -> SlotTimer {
        SlotTimer::new(SlotConfig::default(), Instant::now(), 100)
    }

    #[test]
    fn slot_config_default_sane() {
        let c = SlotConfig::default();
        assert_eq!(c.block_time_ms, 5_000);
        assert_eq!(c.seal_deadline_ms, 4_000);
        assert_eq!(c.propagation_ms, 500);
    }

    #[test]
    fn current_slot_at_least_genesis_block() {
        let t = make_timer();
        assert!(t.current_slot() >= 100);
    }

    #[test]
    fn ms_remaining_within_block_time() {
        let t = make_timer();
        assert!(t.ms_remaining() <= 5_000);
    }

    #[test]
    fn within_build_window_at_genesis() {
        let t = SlotTimer::new(SlotConfig::default(), Instant::now(), 0);
        assert!(t.within_build_window());
    }

    #[test]
    fn custom_block_time_used() {
        let cfg = SlotConfig {
            block_time_ms: 2_000,
            seal_deadline_ms: 1_500,
            propagation_ms: 200,
        };
        let t = SlotTimer::new(cfg.clone(), Instant::now(), 0);
        assert!(t.ms_remaining() <= cfg.block_time_ms);
    }
}
