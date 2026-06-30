//! Light client sync: download and verify headers from full nodes.

use crate::header_chain::{HeaderChain, LightHeader, Checkpoint};
use tracing::{info, debug, warn};
use std::time::Duration;

/// Light sync configuration.
#[derive(Debug, Clone)]
pub struct LightSyncConfig {
    /// RPC endpoints of full nodes to sync from.
    pub rpc_endpoints: Vec<String>,
    /// Polling interval for new headers.
    pub poll_interval: Duration,
    /// Maximum headers to request per batch.
    pub batch_size: u64,
    /// Maximum headers to store in memory.
    pub max_headers: usize,
}

impl Default for LightSyncConfig {
    fn default() -> Self {
        Self {
            rpc_endpoints: vec!["http://localhost:8545".into()],
            poll_interval: Duration::from_secs(5),
            batch_size: 100,
            max_headers: 1000,
        }
    }
}

/// Light client sync manager.
pub struct LightSync {
    chain:  HeaderChain,
    config: LightSyncConfig,
    client: reqwest::Client,
    /// Index of the currently active RPC endpoint.
    active_rpc: usize,
}

impl LightSync {
    pub fn new(config: LightSyncConfig, checkpoint: Checkpoint) -> Self {
        Self {
            chain: HeaderChain::new(checkpoint, config.max_headers),
            client: reqwest::Client::new(),
            active_rpc: 0,
            config,
        }
    }

    /// Run the sync loop (blocks indefinitely).
    pub async fn run(&mut self) {
        info!("light-sync: starting from block #{}", self.chain.tip_number());
        loop {
            if let Err(e) = self.sync_once().await {
                warn!("light-sync: error: {}", e);
                self.rotate_rpc();
            }
            tokio::time::sleep(self.config.poll_interval).await;
        }
    }

    async fn sync_once(&mut self) -> anyhow::Result<()> {
        let from = self.chain.tip_number() + 1;
        let to   = from + self.config.batch_size - 1;
        debug!("light-sync: fetching headers #{}-{}", from, to);

        let rpc_url = &self.config.rpc_endpoints[self.active_rpc];
        let headers = self.fetch_headers(rpc_url, from, to).await?;

        for header in headers {
            if let Err(e) = self.chain.insert(header) {
                warn!("light-sync: invalid header: {}", e);
            }
        }

        info!("light-sync: tip is now #{}", self.chain.tip_number());
        Ok(())
    }

    async fn fetch_headers(
        &self,
        rpc_url: &str,
        from: u64,
        to: u64,
    ) -> anyhow::Result<Vec<LightHeader>> {
        // In production: call zbx_getLightHeaders RPC method.
        Ok(Vec::new())
    }

    fn rotate_rpc(&mut self) {
        self.active_rpc = (self.active_rpc + 1) % self.config.rpc_endpoints.len();
        warn!("light-sync: rotating to RPC endpoint #{}", self.active_rpc);
    }

    pub fn header_chain(&self) -> &HeaderChain { &self.chain }
}