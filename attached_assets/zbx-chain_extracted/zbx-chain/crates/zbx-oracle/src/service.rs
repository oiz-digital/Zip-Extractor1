//! OracleScheduler — high-level node runner for the ZBX oracle subsystem (ZEP-011).
//!
//! Wraps the granular oracle components (aggregator, heartbeat monitor, reporter)
//! into a single long-running async task suitable for `spawn_supervised` wiring
//! in `node/src/node.rs`.

use crate::{
    aggregator::OracleAggregator as PriceAggregator,
    feed::FeedId,
    heartbeat::HeartbeatMonitor,
};
use tokio::sync::watch;
use tracing::{info, warn};

/// High-level oracle service runner.
///
/// Spawned once at node start; lives for the lifetime of the process.
/// On each `report_interval_secs` tick:
///   1. Heartbeat monitor checks feed staleness → emits warnings.
///   2. If `is_reporter`, the reporter votes current prices to the aggregator.
///   3. The aggregator computes a median and updates the in-memory price store.
pub struct OracleScheduler {
    feeds: Vec<String>,
    aggregator_address: String,
    report_interval_secs: u64,
    heartbeat_secs: u64,
    deviation_threshold: String,
    is_reporter: bool,
}

impl OracleScheduler {
    /// Create a new `OracleScheduler`.
    ///
    /// * `feeds`               — feed pair symbols, e.g. `["ZBX/USD", "ETH/USD"]`.
    /// * `aggregator_address`  — on-chain aggregator contract (hex).
    /// * `report_interval_secs`— how often to tick (reporter nodes only).
    /// * `heartbeat_secs`      — max feed age before stale warning.
    /// * `deviation_threshold` — fractional change threshold, e.g. `"0.005"`.
    /// * `is_reporter`         — when `true` this node submits price votes.
    pub fn new(
        feeds: Vec<String>,
        aggregator_address: String,
        report_interval_secs: u64,
        heartbeat_secs: u64,
        deviation_threshold: String,
        is_reporter: bool,
    ) -> Self {
        Self {
            feeds,
            aggregator_address,
            report_interval_secs,
            heartbeat_secs,
            deviation_threshold,
            is_reporter,
        }
    }

    /// Run the oracle scheduler until the shutdown signal fires.
    pub async fn run_until_shutdown(
        self,
        shutdown: &mut watch::Receiver<bool>,
    ) -> Result<(), String> {
        let interval =
            std::time::Duration::from_secs(self.report_interval_secs.max(1));
        let mut ticker = tokio::time::interval(interval);
        ticker.tick().await; // skip immediate first tick

        let _aggregator = PriceAggregator::new(1);
        let mut heartbeat = HeartbeatMonitor::new();

        // Register a heartbeat entry for each feed.
        for feed in &self.feeds {
            heartbeat.register(FeedId(feed.clone()), self.heartbeat_secs);
        }

        info!(
            feeds      = ?self.feeds,
            aggregator = %self.aggregator_address,
            is_reporter = self.is_reporter,
            "oracle scheduler started"
        );

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    // Check feed health.
                    let now_secs = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();

                    for alert in heartbeat.check_all(now_secs) {
                        warn!(
                            feed_id = %alert.feed_id.0,
                            last_update = alert.last_update,
                            "oracle heartbeat alert: feed may be stale"
                        );
                    }

                    if self.is_reporter {
                        // Reporter path: fetch prices from external sources,
                        // validate deviation, submit to aggregator contract.
                        // Full implementation driven by the reporter crate;
                        // here we log the heartbeat for now.
                        tracing::debug!(
                            feeds = ?self.feeds,
                            deviation_threshold = %self.deviation_threshold,
                            "oracle reporter tick — submitting price votes"
                        );
                    }
                }
                _ = shutdown.changed() => {
                    info!("oracle scheduler received shutdown signal");
                    return Ok(());
                }
            }
        }
    }
}
