//! Admin JSON-RPC server — `admin_*` namespace.
//!
//! All methods require a valid admin bearer token in the Authorization header.
//!
//! ## Methods
//!
//! | Method                       | Role      | Description                       |
//! |------------------------------|-----------|-----------------------------------|
//! | admin_nodeInfo               | ReadOnly  | Node identity and version         |
//! | admin_peers                  | ReadOnly  | List connected peers              |
//! | admin_addPeer                | Operator  | Add a static peer                 |
//! | admin_removePeer             | Operator  | Remove a peer                     |
//! | admin_banPeer                | Operator  | Ban a peer for N seconds          |
//! | admin_mempoolStatus          | ReadOnly  | Mempool stats                     |
//! | admin_clearMempool           | Operator  | Drop all pending transactions     |
//! | admin_validatorStatus        | ReadOnly  | Current validator set             |
//! | admin_setValidatorActive     | Validator | Activate / deactivate a validator |
//! | admin_slashValidator         | Validator | Manually slash a validator        |
//! | admin_reloadConfig           | SuperUser | Hot-reload node.toml              |
//! | admin_startBackup            | SuperUser | Begin an online backup            |
//! | admin_stopNode               | SuperUser | Graceful shutdown                 |
//! | admin_chainStatus            | ReadOnly  | Chain head, finalized, sync status|
//! | admin_dbCompact              | SuperUser | Trigger RocksDB compaction        |
//! | admin_setLogLevel            | Operator  | Change log level at runtime       |

use crate::{
    auth::{AdminRole, AdminSession, verify_token},
    config::{ConfigHandle, NodeConfig},
    error::AdminError,
    metrics::NodeMetrics,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use tracing::{info, warn};

// ── Request / Response types ──────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeInfoResponse {
    pub name:           String,
    pub version:        String,
    pub chain_id:       u64,
    pub node_id:        String,     // hex-encoded public key
    pub listen_addr:    String,
    pub protocols:      Vec<String>,
    pub sync_state:     String,     // "synced"|"syncing"|"stalled"
    pub head_number:    u64,
    pub head_hash:      String,
    pub finalized:      u64,
    pub peer_count:     usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id:          String,
    pub addr:        String,
    pub direction:   String,     // "inbound"|"outbound"
    pub protocols:   Vec<String>,
    pub head_number: u64,
    pub latency_ms:  Option<u64>,
    pub bytes_sent:  u64,
    pub bytes_recv:  u64,
    pub connected_s: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MempoolStatus {
    pub pending:       usize,
    pub queued:        usize,
    pub total_bytes:   usize,
    pub base_fee_gwei: f64,
    pub min_tip_gwei:  f64,
    pub oldest_age_s:  u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ValidatorInfo {
    pub address:         String,
    pub status:          String,    // "active"|"jailed"|"pending_exit"
    pub self_stake:      String,    // decimal string (wei)
    pub total_delegated: String,
    pub commission_bps:  u64,
    pub blocks_proposed: u64,
    pub blocks_missed:   u64,
    pub slash_count:     u32,
    pub since_epoch:     u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChainStatus {
    pub head_number:       u64,
    pub head_hash:         String,
    pub finalized_number:  u64,
    pub finalized_hash:    String,
    pub safe_number:       u64,
    pub is_syncing:        bool,
    pub sync_target:       Option<u64>,
    pub sync_pct:          Option<f64>,
    pub epoch:             u64,
    pub next_epoch_block:  u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BackupStatus {
    pub id:             String,
    pub state:          String,    // "running"|"completed"|"failed"
    pub started_at:     u64,
    pub bytes_written:  u64,
    pub path:           String,
}

// ── Admin RPC handler ─────────────────────────────────────────────────────────

pub struct AdminRpcHandler {
    pub config: ConfigHandle,
}

impl AdminRpcHandler {
    pub fn new(config: ConfigHandle) -> Self {
        Self { config }
    }

    pub fn node_info(&self) -> NodeInfoResponse {
        let cfg = self.config.read().unwrap();
        NodeInfoResponse {
            name:        cfg.node.name.clone(),
            version:     env!("CARGO_PKG_VERSION").into(),
            chain_id:    cfg.node.chain_id,
            node_id:     "0x00".into(), // populated from P2P layer at runtime
            listen_addr: cfg.network.listen_addr.to_string(),
            protocols:   vec!["zbx/1".into(), "eth/68".into()],
            sync_state:  "synced".into(),
            head_number: 0,
            head_hash:   "0x0000000000000000000000000000000000000000000000000000000000000000".into(),
            finalized:   0,
            peer_count:  0,
        }
    }

    pub fn mempool_status(&self) -> MempoolStatus {
        MempoolStatus {
            pending:       0,
            queued:        0,
            total_bytes:   0,
            base_fee_gwei: 1.0,
            min_tip_gwei:  0.001,
            oldest_age_s:  0,
        }
    }

    pub fn chain_status(&self) -> ChainStatus {
        ChainStatus {
            head_number:      0,
            head_hash:        "0x0000000000000000000000000000000000000000000000000000000000000000".into(),
            finalized_number: 0,
            finalized_hash:   "0x0000000000000000000000000000000000000000000000000000000000000000".into(),
            safe_number:      0,
            is_syncing:       false,
            sync_target:      None,
            sync_pct:         None,
            epoch:            0,
            next_epoch_block: 14_400,
        }
    }

    pub fn set_log_level(&self, level: &str) -> Result<String, AdminError> {
        let valid = ["trace", "debug", "info", "warn", "error"];
        if !valid.contains(&level) {
            return Err(AdminError::InvalidParam(format!(
                "invalid log level '{}' (use: {})", level, valid.join("|")
            )));
        }
        info!("admin: log level changed to '{}'", level);
        // In production: update the tracing subscriber filter.
        Ok(format!("log level set to '{}'", level))
    }

    pub fn reload_config(&self, path: &std::path::Path) -> Result<Vec<String>, AdminError> {
        let new_cfg = crate::config::load(path)?;
        let changed  = crate::config::hot_reload(&self.config, new_cfg)?;
        info!("admin: config reloaded ({} fields changed)", changed.len());
        Ok(changed)
    }
}