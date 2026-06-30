//! Live node configuration: load, validate, hot-reload.
//!
//! Configuration is read from `node.toml` at startup and can be
//! reloaded at runtime via the `admin_reloadConfig` RPC call.
//! Only a subset of fields support hot-reload (marked with ✓ below).

use crate::error::AdminError;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// Complete node configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    pub node:      NodeSection,
    pub network:   NetworkSection,
    pub mempool:   MempoolSection,
    pub staking:   StakingSection,
    pub rpc:       RpcSection,
    pub admin:     AdminSection,
    pub storage:   StorageSection,
    pub consensus: ConsensusSection,
    pub metrics:   MetricsSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSection {
    pub name:         String,
    pub chain_id:     u64,
    pub data_dir:     PathBuf,
    pub log_level:    String,   // "trace"|"debug"|"info"|"warn"|"error"
    pub log_format:   String,   // "json"|"text"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSection {
    pub listen_addr:     SocketAddr,
    pub external_ip:     Option<String>,
    pub max_peers:       usize,     // ✓ hot-reload
    pub max_inbound:     usize,     // ✓ hot-reload
    pub max_outbound:    usize,     // ✓ hot-reload
    pub bootnodes:       Vec<String>,
    pub ban_duration_s:  u64,
    pub discovery:       bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MempoolSection {
    pub max_txs:         usize,     // ✓ hot-reload
    pub max_tx_size:     usize,
    pub min_gas_price:   u64,       // ✓ hot-reload (in wei)
    pub eviction_policy: String,    // "oldest"|"cheapest"
    pub local_addresses: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakingSection {
    pub min_self_stake:    u128,
    pub max_validators:    usize,
    pub epoch_length:      u64,
    pub commission_max_bps: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcSection {
    pub http_addr:    SocketAddr,
    pub ws_addr:      Option<SocketAddr>,
    pub max_conns:    usize,        // ✓ hot-reload
    pub cors_origins: Vec<String>,  // ✓ hot-reload
    pub rate_limit:   u32,          // ✓ hot-reload (req/s per IP)
    pub timeout_s:    u64,
    pub api_modules:  Vec<String>,  // e.g. ["eth","net","zbx","debug"]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminSection {
    pub listen_addr:  SocketAddr,
    pub secret_file:  PathBuf,
    pub allowed_ips:  Vec<String>,
    pub tls_cert:     Option<PathBuf>,
    pub tls_key:      Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageSection {
    pub path:            PathBuf,
    pub cache_mb:        usize,
    pub max_open_files:  i32,
    pub compression:     String,     // "lz4"|"zstd"|"none"
    pub prune_mode:      String,     // "full"|"fast"|"archive"
    pub prune_keep:      u64,        // blocks to retain in fast mode
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusSection {
    pub validator_key:  Option<PathBuf>,
    pub proposer_delay_ms: u64,
    pub vote_timeout_ms:   u64,
    pub sync_committee:    bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSection {
    pub enabled:   bool,
    pub addr:      SocketAddr,
    pub namespace: String,
}

// ── Default configuration ─────────────────────────────────────────────────────

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            node: NodeSection {
                name:       "zebvix-node".into(),
                chain_id:   zbx_types::CHAIN_ID_MAINNET,
                data_dir:   PathBuf::from("/var/lib/zebvix"),
                log_level:  "info".into(),
                log_format: "text".into(),
            },
            network: NetworkSection {
                listen_addr:    "0.0.0.0:30303".parse().unwrap(),
                external_ip:    None,
                max_peers:      50,
                max_inbound:    35,
                max_outbound:   15,
                bootnodes:      vec![],
                ban_duration_s: 3600,
                discovery:      true,
            },
            mempool: MempoolSection {
                max_txs:         8_192,
                max_tx_size:     128 * 1024,
                min_gas_price:   1_000_000_000,
                eviction_policy: "cheapest".into(),
                local_addresses: vec![],
            },
            staking: StakingSection {
                min_self_stake:     100_000 * 10u128.pow(18),
                max_validators:     21,
                epoch_length:       14_400,
                commission_max_bps: 2_000,
            },
            rpc: RpcSection {
                http_addr:    "127.0.0.1:8545".parse().unwrap(),
                ws_addr:      Some("127.0.0.1:8546".parse().unwrap()),
                max_conns:    256,
                cors_origins: vec!["*".into()],
                rate_limit:   100,
                timeout_s:    30,
                api_modules:  vec!["eth".into(), "net".into(), "zbx".into(), "txpool".into()],
            },
            admin: AdminSection {
                listen_addr:  "127.0.0.1:8547".parse().unwrap(),
                secret_file:  PathBuf::from("/etc/zebvix/admin.secret"),
                allowed_ips:  vec!["127.0.0.1".into()],
                tls_cert:     None,
                tls_key:      None,
            },
            storage: StorageSection {
                path:           PathBuf::from("/var/lib/zebvix/chaindata"),
                cache_mb:       512,
                max_open_files: 1024,
                compression:    "lz4".into(),
                prune_mode:     "fast".into(),
                prune_keep:     10_000,
            },
            consensus: ConsensusSection {
                validator_key:     None,
                proposer_delay_ms: 500,
                vote_timeout_ms:   2_000,
                sync_committee:    false,
            },
            metrics: MetricsSection {
                // NODE-SEC-2026: bind to loopback only. Exposing Prometheus on
                // 0.0.0.0:9090 leaks internal node metrics to the public internet.
                // Operators who need external scraping must explicitly bind to a
                // private interface and protect with firewall or auth proxy.
                enabled:   true,
                addr:      "127.0.0.1:9090".parse().unwrap(),
                namespace: "zbx".into(),
            },
        }
    }
}

/// Thread-safe config handle (shared across the node).
pub type ConfigHandle = Arc<RwLock<NodeConfig>>;

/// Load config from TOML file.
pub fn load(path: &std::path::Path) -> Result<NodeConfig, AdminError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| AdminError::Config(format!("read '{}': {}", path.display(), e)))?;
    let cfg: NodeConfig = toml::from_str(&raw)
        .map_err(|e| AdminError::Config(format!("parse '{}': {}", path.display(), e)))?;
    validate(&cfg)?;
    Ok(cfg)
}

/// Validate config values.
pub fn validate(cfg: &NodeConfig) -> Result<(), AdminError> {
    if cfg.node.chain_id == 0 {
        return Err(AdminError::Config("chain_id cannot be 0".into()));
    }
    if cfg.network.max_inbound + cfg.network.max_outbound > cfg.network.max_peers {
        return Err(AdminError::Config(
            "max_inbound + max_outbound must be <= max_peers".into()
        ));
    }
    if !["lz4", "zstd", "none"].contains(&cfg.storage.compression.as_str()) {
        return Err(AdminError::Config(format!(
            "invalid compression '{}' (use lz4|zstd|none)", cfg.storage.compression
        )));
    }
    if !["full", "fast", "archive"].contains(&cfg.storage.prune_mode.as_str()) {
        return Err(AdminError::Config(format!(
            "invalid prune_mode '{}' (use full|fast|archive)", cfg.storage.prune_mode
        )));
    }
    Ok(())
}

/// Hot-reload: apply a new config, returning a diff of changed fields.
pub fn hot_reload(
    handle: &ConfigHandle,
    new_cfg: NodeConfig,
) -> Result<Vec<String>, AdminError> {
    validate(&new_cfg)?;
    let old = handle.read().unwrap().clone();
    let mut changed = Vec::new();

    if old.network.max_peers    != new_cfg.network.max_peers    { changed.push("network.max_peers".into()); }
    if old.network.max_inbound  != new_cfg.network.max_inbound  { changed.push("network.max_inbound".into()); }
    if old.network.max_outbound != new_cfg.network.max_outbound { changed.push("network.max_outbound".into()); }
    if old.mempool.max_txs      != new_cfg.mempool.max_txs      { changed.push("mempool.max_txs".into()); }
    if old.mempool.min_gas_price!= new_cfg.mempool.min_gas_price{ changed.push("mempool.min_gas_price".into()); }
    if old.rpc.max_conns        != new_cfg.rpc.max_conns        { changed.push("rpc.max_conns".into()); }
    if old.rpc.rate_limit       != new_cfg.rpc.rate_limit       { changed.push("rpc.rate_limit".into()); }

    *handle.write().unwrap() = new_cfg;
    Ok(changed)
}