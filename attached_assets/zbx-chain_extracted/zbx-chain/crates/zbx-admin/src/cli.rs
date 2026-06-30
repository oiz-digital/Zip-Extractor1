//! Admin CLI — `zbxadmin` command-line tool.
//!
//! Usage:
//!   zbxadmin [--node <url>] [--secret <path>] <command> [args…]
//!
//! Commands:
//!   node info              — print node identity and sync status
//!   node stop              — graceful shutdown
//!   peers list             — list connected peers
//!   peers add <enode>      — add a static peer
//!   peers remove <enode>   — disconnect and remove a peer
//!   peers ban <ip> <secs>  — ban an IP
//!   mempool status         — show mempool statistics
//!   mempool clear          — drop all pending transactions
//!   mempool inspect <hash> — inspect a pending transaction
//!   validator list         — list all validators
//!   validator slash <addr> <reason> — manually slash a validator
//!   validator jail <addr>  — jail a validator
//!   validator unjail <addr>— unjail a validator
//!   config show            — print current config
//!   config reload          — hot-reload node.toml
//!   config set <key> <val> — set a hot-reloadable config value
//!   db compact             — trigger RocksDB compaction
//!   db inspect <col> <key> — raw database key lookup
//!   db stats               — print RocksDB statistics
//!   backup start <path>    — start an online backup
//!   backup status          — check backup progress
//!   logs level <level>     — change log level at runtime
//!   metrics snapshot       — print current Prometheus metrics

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "zbxadmin", version, about = "Zebvix Chain node administration CLI")]
pub struct Cli {
    /// Admin RPC endpoint
    #[arg(long, default_value = "http://127.0.0.1:8547")]
    pub node: String,

    /// Path to admin secret file
    #[arg(long, default_value = "/etc/zebvix/admin.secret")]
    pub secret: std::path::PathBuf,

    /// Output format
    #[arg(long, default_value = "table", value_parser = ["table", "json"])]
    pub format: String,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Node identity and status
    Node {
        #[command(subcommand)]
        action: NodeAction,
    },
    /// Peer management
    Peers {
        #[command(subcommand)]
        action: PeersAction,
    },
    /// Mempool management
    Mempool {
        #[command(subcommand)]
        action: MempoolAction,
    },
    /// Validator management
    Validator {
        #[command(subcommand)]
        action: ValidatorAction,
    },
    /// Config management
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Database tools
    Db {
        #[command(subcommand)]
        action: DbAction,
    },
    /// Backup management
    Backup {
        #[command(subcommand)]
        action: BackupAction,
    },
    /// Log level management
    Logs {
        #[command(subcommand)]
        action: LogsAction,
    },
    /// Metrics snapshot
    Metrics,
}

#[derive(Subcommand, Debug)]
pub enum NodeAction {
    /// Print node identity and sync status
    Info,
    /// Graceful node shutdown
    Stop,
    /// Print chain head and finality
    Status,
}

#[derive(Subcommand, Debug)]
pub enum PeersAction {
    /// List all connected peers
    List,
    /// Add a static peer
    Add { enode: String },
    /// Disconnect and remove a peer
    Remove { enode: String },
    /// Ban a peer by IP
    Ban {
        ip:   String,
        #[arg(default_value = "3600")]
        secs: u64,
    },
}

#[derive(Subcommand, Debug)]
pub enum MempoolAction {
    /// Show mempool statistics
    Status,
    /// Drop all pending transactions
    Clear,
    /// Inspect a specific transaction
    Inspect { hash: String },
}

#[derive(Subcommand, Debug)]
pub enum ValidatorAction {
    /// List all validators and their status
    List,
    /// Manually slash a validator
    Slash { address: String, reason: String },
    /// Jail a validator (stop block proposals)
    Jail { address: String },
    /// Unjail a jailed validator
    Unjail { address: String },
    /// Show validator's full history
    History { address: String },
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Print current active configuration
    Show,
    /// Hot-reload node.toml from disk
    Reload,
    /// Set a hot-reloadable config key
    Set { key: String, value: String },
}

#[derive(Subcommand, Debug)]
pub enum DbAction {
    /// Trigger RocksDB compaction
    Compact,
    /// Raw key lookup in a column family
    Inspect {
        column: String,
        key:    String,
    },
    /// Print column-family statistics
    Stats,
    /// Estimate live data size
    Size,
}

#[derive(Subcommand, Debug)]
pub enum BackupAction {
    /// Start an online backup
    Start { path: String },
    /// Check backup progress
    Status,
    /// Cancel an in-progress backup
    Cancel,
}

#[derive(Subcommand, Debug)]
pub enum LogsAction {
    /// Change the active log level
    Level { level: String },
}

/// Run the CLI (entry point).
pub fn run() -> Result<(), crate::error::AdminError> {
    let cli = Cli::parse();
    // In production: create an HTTP client, sign requests, dispatch subcommands.
    println!("zbxadmin: connecting to {}", cli.node);
    Ok(())
}