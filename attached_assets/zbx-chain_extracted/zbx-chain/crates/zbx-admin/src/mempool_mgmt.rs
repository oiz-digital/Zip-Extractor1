//! Mempool admin operations: inspection, clearing, and tx eviction.
//!
//! ## Pool audit findings (Session 23)
//!
//! Issues found and fixed:
//! 1. `PendingTxSummary` was missing `gas_token` — needed for multi-token
//!    gas fee filtering and block-explorer display (ZBX / ZUSD).
//! 2. `MempoolStats` had no per-token breakdown — operators couldn't see
//!    how many pending txs use each gas token.
//! 3. `EvictFilter` had no `ByGasToken` variant — couldn't surgically evict
//!    all txs of one token (e.g. during ZUSD depeg).
//! 4. `set_min_gas_price` applied one threshold across all gas tokens — added
//!    per-token floor support via `set_min_gas_price_per_token`.

use crate::error::AdminError;
use zbx_types::{address::Address, H256, U256};
use serde::{Serialize, Deserialize};
use tracing::{info, warn};

// ── Gas token ID ───────────────────────────────────────────────────────────────

/// Gas token discriminant mirroring `zbx_tx::GasToken` wire byte.
///
/// Using `u8` avoids a heavy dep from `zbx-admin` on `zbx-tx`.
/// Keep in sync with `zbx_tx::GasToken`.
///
/// | Value | Token |
/// |-------|-------|
/// | 0     | ZBX (default) |
/// | 1     | ZUSD |
pub type GasTokenId = u8;

pub const GAS_ZBX:  GasTokenId = 0;
pub const GAS_ZUSD: GasTokenId = 1;

pub fn gas_token_symbol(id: GasTokenId) -> &'static str {
    match id {
        GAS_ZBX  => "ZBX",
        GAS_ZUSD => "ZUSD",
        _        => "UNKNOWN",
    }
}

// ── Mempool stats ──────────────────────────────────────────────────────────────

/// Detailed mempool statistics (extended with per-gas-token counts).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct MempoolStats {
    pub pending_count:        usize,
    pub queued_count:         usize,
    pub total_txs:            usize,
    pub total_size_bytes:     usize,
    pub unique_senders:       usize,
    pub base_fee_wei:         u128,
    pub min_effective_tip:    u128,
    pub max_effective_tip:    u128,
    pub oldest_pending_s:     u64,
    pub evictions_total:      u64,
    pub replacements_total:   u64,
    // Multi-token breakdown (Session 23).
    /// Pending txs paying gas in ZBX (gas_token=0).
    pub pending_zbx_gas:      usize,
    /// Pending txs paying gas in ZUSD (gas_token=1).
    pub pending_zusd_gas:     usize,
    /// Minimum gas price floor for ZBX-paying txs (wei).
    pub min_gas_price_zbx:    u128,
    /// Minimum gas price floor for ZUSD-paying txs (base units).
    pub min_gas_price_zusd:   u128,
}

// ── Pending tx summary ─────────────────────────────────────────────────────────

/// Summary of a pending transaction (with gas_token, Session 23).
#[derive(Debug, Serialize, Deserialize)]
pub struct PendingTxSummary {
    pub hash:           H256,
    pub sender:         Address,
    pub nonce:          u64,
    pub to:             Option<Address>,
    pub value_wei:      U256,
    pub gas_limit:      u64,
    pub max_fee_gwei:   f64,
    pub max_tip_gwei:   f64,
    pub data_len:       usize,
    pub received_s:     u64,
    pub replacements:   u32,
    /// Gas payment token: 0=ZBX (default), 1=ZUSD.
    pub gas_token:      GasTokenId,
    /// Human-readable symbol, e.g. "ZBX".
    pub gas_token_sym:  String,
}

// ── Clear result ───────────────────────────────────────────────────────────────

/// Result of a mempool clear operation.
#[derive(Debug, Serialize, Deserialize)]
pub struct ClearResult {
    pub removed_pending: usize,
    pub removed_queued:  usize,
    pub filter:          Option<String>,
}

// ── Eviction filter ────────────────────────────────────────────────────────────

/// Eviction filter: which transactions to remove.
///
/// `ByGasToken` added in Session 23 for emergency stablecoin-gas eviction.
#[derive(Debug, Clone)]
pub enum EvictFilter {
    All,
    BySender(Address),
    BelowGasPrice(U256),
    OlderThanSecs(u64),
    MaxCount(usize),
    /// Evict all txs paying gas in a specific token (0=ZBX, 1=ZUSD).
    ByGasToken(GasTokenId),
}

// ── Operations ─────────────────────────────────────────────────────────────────

/// Clear mempool transactions matching a filter.
pub fn clear_mempool(filter: EvictFilter) -> Result<ClearResult, AdminError> {
    let desc = match &filter {
        EvictFilter::All                => "all transactions".to_string(),
        EvictFilter::BySender(a)        => format!("transactions from {:?}", a),
        EvictFilter::BelowGasPrice(p)   => format!("transactions below {} wei", p),
        EvictFilter::OlderThanSecs(s)   => format!("transactions older than {}s", s),
        EvictFilter::MaxCount(n)        => format!("keep top {} by price", n),
        EvictFilter::ByGasToken(t)      => format!(
            "transactions paying gas in {} (token_id={})", gas_token_symbol(*t), t
        ),
    };
    warn!("admin: clearing mempool — {}", desc);
    // In production: delegate to zbx-mempool eviction routines.
    Ok(ClearResult { removed_pending: 0, removed_queued: 0, filter: Some(desc) })
}

/// Look up a specific pending transaction by hash.
pub fn inspect_tx(hash: H256) -> Result<Option<PendingTxSummary>, AdminError> {
    // In production: query zbx-mempool pool by hash.
    let _ = hash;
    Ok(None)
}

// ── Gas price floors ───────────────────────────────────────────────────────────

/// Per-token minimum gas price floors.
///
/// Each gas token has its own floor because ZBX and ZUSD have
/// different market values and oracle price feeds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinGasPricePerToken {
    pub zbx_wei:  u64,
    pub zusd_wei: u64,
}

impl Default for MinGasPricePerToken {
    fn default() -> Self {
        Self {
            zbx_wei:  1_000_000_000,
            zusd_wei: 1_000_000_000,
        }
    }
}

/// Set a uniform gas price floor (applies to ZBX — legacy API).
pub fn set_min_gas_price(price_wei: u64) -> Result<u64, AdminError> {
    if price_wei == 0 {
        return Err(AdminError::InvalidParam("min_gas_price cannot be 0".into()));
    }
    info!("admin: ZBX gas floor = {} wei ({:.3} Gwei)",
          price_wei, price_wei as f64 / 1e9);
    Ok(price_wei)
}

/// Set independent gas price floors for each of the two gas tokens.
pub fn set_min_gas_price_per_token(cfg: MinGasPricePerToken) -> Result<MinGasPricePerToken, AdminError> {
    if cfg.zbx_wei == 0 || cfg.zusd_wei == 0 {
        return Err(AdminError::InvalidParam("all per-token gas floors must be > 0".into()));
    }
    info!(
        zbx_gwei  = cfg.zbx_wei  as f64 / 1e9,
        zusd_gwei = cfg.zusd_wei as f64 / 1e9,
        "admin: per-token gas price floors updated"
    );
    Ok(cfg)
}

/// Add an address to the local-priority list (not affected by gas price floor).
pub fn add_local_address(addr: Address) -> Result<(), AdminError> {
    info!("admin: address {:?} added to local-priority list", addr);
    Ok(())
}

/// Remove an address from the local-priority list.
pub fn remove_local_address(addr: Address) -> Result<(), AdminError> {
    info!("admin: address {:?} removed from local-priority list", addr);
    Ok(())
}
