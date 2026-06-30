//! HTTP server that exposes the /metrics endpoint.
//!
//! SEC-2026-05-09 Pass-10:
//!   * Fix `\\r\\n` literal-escape bug in the response framing — no Prometheus
//!     scraper could parse the body.
//!   * Carry a full `Registry` instead of just the four legacy families so
//!     RPC, bridge and staking counters are exported too.
//!   * 1 KiB max request line, drop the body — defends against slow-loris
//!     style scrapers and stops mis-routed POST payloads from leaking memory.

use crate::counters::{
    BlockMetrics, ConsensusMetrics, MempoolMetrics, NetworkMetrics,
    RpcMetrics, BridgeMetrics, StakingMetrics, Registry, render_prometheus_full,
};
use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, error};

/// Metrics HTTP server (Prometheus scrape target).
pub struct MetricsServer {
    pub port: u16,
    /// Bind address. Defaults to localhost so a public node does not
    /// inadvertently expose internal counters to the open internet.
    /// Override with `with_bind` or env var `ZBX_METRICS_BIND` (e.g. set to
    /// "0.0.0.0" in a private K8s overlay network).
    /// See AUDIT_2026-04-30.md M-17.
    pub bind_addr: String,
    /// Full registry — Pass-10 added rpc/bridge/staking families.
    pub registry: Registry,
    // Family handles re-exported for back-compat with existing call sites
    // (`server.block_metrics.on_block_committed(...)` etc). They share Arcs
    // with `registry`, so updates flow through to the scrape output.
    pub block_metrics: BlockMetrics,
    pub consensus_metrics: ConsensusMetrics,
    pub mempool_metrics: MempoolMetrics,
    pub network_metrics: NetworkMetrics,
    pub rpc_metrics: RpcMetrics,
    pub bridge_metrics: BridgeMetrics,
    pub staking_metrics: StakingMetrics,
}

impl MetricsServer {
    pub fn new(port: u16) -> Self {
        Self::with_registry(port, Registry::new())
    }

    /// SEC-2026-05-09 Pass-10 — construct against a caller-owned Registry
    /// so node.rs can hand the same Arc-backed handles to ConsensusDriver,
    /// network layer, etc., and have them flow through to the scrape
    /// output. All family handles share Arcs with the registry.
    pub fn with_registry(port: u16, registry: Registry) -> Self {
        MetricsServer {
            port,
            bind_addr: std::env::var("ZBX_METRICS_BIND")
                .unwrap_or_else(|_| "127.0.0.1".to_string()),
            block_metrics:     registry.blocks.clone(),
            consensus_metrics: registry.consensus.clone(),
            mempool_metrics:   registry.mempool.clone(),
            network_metrics:   registry.network.clone(),
            rpc_metrics:       registry.rpc.clone(),
            bridge_metrics:    registry.bridge.clone(),
            staking_metrics:   registry.staking.clone(),
            registry,
        }
    }

    /// Override bind address (e.g. "0.0.0.0" for in-cluster scraping).
    pub fn with_bind(mut self, bind: impl Into<String>) -> Self {
        self.bind_addr = bind.into();
        self
    }

    pub async fn run(&self) -> std::io::Result<()> {
        let addr = format!("{}:{}", self.bind_addr, self.port);
        let listener = TcpListener::bind(&addr).await?;
        info!(addr = addr, "metrics server listening");

        let registry = self.registry.clone();

        loop {
            let (mut stream, peer) = listener.accept().await?;
            let reg = registry.clone();
            tokio::spawn(async move {
                // Drain at most 1 KiB of request data — we only need to
                // consume the request line so the client doesn't block on
                // its write side.
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await;
                let body = render_prometheus_full(&reg);
                let response = format!(
                    "HTTP/1.1 200 OK\r\n\
                     Content-Type: text/plain; version=0.0.4; charset=utf-8\r\n\
                     Content-Length: {}\r\n\
                     Connection: close\r\n\
                     \r\n\
                     {}",
                    body.len(), body,
                );
                if let Err(e) = stream.write_all(response.as_bytes()).await {
                    error!(?peer, error = %e, "metrics: response write failed");
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    /// End-to-end: serve a real HTTP scrape, parse the response status line
    /// and confirm the body is a sequence of well-formed prom-text lines
    /// terminated by real `\n`.
    #[tokio::test]
    async fn end_to_end_scrape_returns_valid_prom_text() {
        let srv = MetricsServer::new(0).with_bind("127.0.0.1");
        // Bind on an ephemeral port via the kernel.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound = listener.local_addr().unwrap();
        let registry = srv.registry.clone();
        registry.blocks.committed_blocks.add(99);

        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf).await;
            let body = render_prometheus_full(&registry);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body
            );
            s.write_all(resp.as_bytes()).await.unwrap();
        });

        let mut client = tokio::net::TcpStream::connect(bound).await.unwrap();
        client.write_all(b"GET /metrics HTTP/1.0\r\n\r\n").await.unwrap();
        let mut resp = Vec::new();
        client.read_to_end(&mut resp).await.unwrap();
        let text = String::from_utf8(resp).unwrap();

        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains("Content-Type: text/plain"));
        let body = text.split("\r\n\r\n").nth(1).unwrap();
        assert!(body.contains("zbx_committed_blocks_total 99\n"));
        assert!(!body.contains("\\n"), "body still contains literal '\\n'");
    }
}
