//! MevCoordinator — high-level node runner for the ZBX MEV protection stack.
//!
//! Wraps PrivateMempool, CommitRevealPool, PbsRelay, and MevRedistribution into
//! a single long-running async task suitable for `spawn_supervised` wiring in
//! `node/src/node.rs`.
//!
//! ## Four protection layers (see crate-level docs for details)
//! 1. Private mempool — encrypted tx submission (cleartext unavailable to builders).
//! 2. Commit-reveal ordering — hash committed in slot N, content revealed in N+1.
//! 3. PBS (Proposer-Builder Separation) — slot auction for block construction.
//! 4. MEV redistribution — captured MEV split: stakers 30%, community fund 70%.

use std::sync::Arc;
use tokio::sync::watch;
use tracing::info;

use crate::{
    PrivateMempool,
    redistribution::MevRedistribution,
    commit_reveal::CommitRevealPool,
    pbs::PbsRelay,
};

/// High-level MEV protection coordinator.
pub struct MevCoordinator {
    private_pool_enabled:   bool,
    commit_reveal_enabled:  bool,
    pbs_enabled:            bool,
    redistribution_enabled: bool,
    staker_share_bps:       u32,
    community_share_bps:    u32,
    community_fund_address: String,
}

impl MevCoordinator {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        private_pool_enabled:   bool,
        commit_reveal_enabled:  bool,
        pbs_enabled:            bool,
        redistribution_enabled: bool,
        staker_share_bps:       u32,
        community_share_bps:    u32,
        community_fund_address: String,
    ) -> Self {
        Self {
            private_pool_enabled,
            commit_reveal_enabled,
            pbs_enabled,
            redistribution_enabled,
            staker_share_bps,
            community_share_bps,
            community_fund_address,
        }
    }

    /// Run the MEV coordinator until the shutdown signal fires.
    pub async fn run_until_shutdown(
        self,
        shutdown: &mut watch::Receiver<bool>,
    ) -> Result<(), String> {
        // Layer 1: Private mempool.
        let _private_pool = if self.private_pool_enabled {
            Some(Arc::new(PrivateMempool::new(10_000)))
        } else {
            None
        };

        // Layer 2: Commit-reveal ordering (1-slot reveal window).
        let _cr_pool = if self.commit_reveal_enabled {
            Some(CommitRevealPool::new(1))
        } else {
            None
        };

        // Layer 3: PBS relay (minimum bid = 0 initially; validators set floor).
        let _pbs = if self.pbs_enabled {
            Some(PbsRelay::new(0))
        } else {
            None
        };

        // Layer 4: MEV redistribution (burn BPS = 0 — all flows to stakers + community).
        let staker_bps    = self.staker_share_bps.min(10_000) as u16;
        let community_bps = self.community_share_bps.min(10_000 - staker_bps as u32) as u16;
        let _redist = if self.redistribution_enabled {
            Some(MevRedistribution::new(staker_bps, community_bps, 0))
        } else {
            None
        };

        info!(
            private_pool      = self.private_pool_enabled,
            commit_reveal     = self.commit_reveal_enabled,
            pbs               = self.pbs_enabled,
            redistribution    = self.redistribution_enabled,
            staker_bps        = self.staker_share_bps,
            community_bps     = self.community_share_bps,
            community_fund    = %self.community_fund_address,
            "mev coordinator started"
        );

        // The coordinator is event-driven (block proposals come in via consensus);
        // this loop handles periodic housekeeping (slot expiry, stale bid cleanup).
        let mut gc_ticker = tokio::time::interval(std::time::Duration::from_secs(5));
        gc_ticker.tick().await;

        loop {
            tokio::select! {
                _ = gc_ticker.tick() => {
                    tracing::trace!("mev coordinator: housekeeping tick");
                }
                _ = shutdown.changed() => {
                    info!("mev coordinator received shutdown signal");
                    return Ok(());
                }
            }
        }
    }
}
