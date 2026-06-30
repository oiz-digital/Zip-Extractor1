//! Node runtime configuration.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    pub data_dir:     PathBuf,
    pub keystore_dir: PathBuf,
    pub log_level:    String,
    pub p2p:          P2pConfig,
    pub rpc:          RpcConfig,
    pub metrics:      MetricsConfig,
    pub sync:         SyncConfig,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            data_dir: "./data".into(), keystore_dir: "./keystore".into(),
            log_level: "info".into(), p2p: P2pConfig::default(),
            rpc: RpcConfig::default(), metrics: MetricsConfig::default(),
            sync: SyncConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2pConfig {
    pub listen: String, pub port: u16, pub max_peers: u32,
    pub bootnodes: Vec<String>, pub discovery: bool,
}
impl Default for P2pConfig {
    fn default() -> Self {
        Self { listen: "0.0.0.0".into(), port: 30303, max_peers: 50,
               bootnodes: vec!["seed1.zebvix.com:30303".into()], discovery: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcConfig {
    pub http_enabled: bool, pub http_addr: String, pub http_port: u16,
    pub ws_enabled:   bool, pub ws_addr:   String, pub ws_port:   u16,
    pub modules: Vec<String>,
}
impl Default for RpcConfig {
    fn default() -> Self {
        Self {
            http_enabled: true, http_addr: "127.0.0.1".into(), http_port: 8545,
            ws_enabled:   true, ws_addr:   "127.0.0.1".into(), ws_port:   8546,
            modules: vec!["eth".into(),"net".into(),"web3".into(),"zbx".into()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsConfig { pub enabled: bool, pub addr: String, pub port: u16 }
impl Default for MetricsConfig { fn default() -> Self { Self { enabled: true, addr: "127.0.0.1".into(), port: 9090 } } }
// NODE-SEC-2026: MetricsConfig.addr defaults to 127.0.0.1 (loopback only).
// Previously the zbx-admin NodeConfig had 0.0.0.0:9090 which exposed Prometheus
// metrics on all network interfaces.  Operators who need external scraping should
// bind to a specific private IP and protect with a firewall rule or nginx auth proxy.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig { pub mode: SyncMode, pub snap_min_peers: u32 }
impl Default for SyncConfig { fn default() -> Self { Self { mode: SyncMode::Snap, snap_min_peers: 5 } } }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncMode { Full, Snap, Light }