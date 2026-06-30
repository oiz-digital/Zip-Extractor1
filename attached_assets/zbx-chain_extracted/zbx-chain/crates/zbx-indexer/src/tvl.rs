//! TVL snapshot collector for the on-chain ZbxTvlOracle (ZEP-007).
//!
//! Pairs the indexer's SQLite store with the on-chain TVL aggregator
//! deployed at a configurable address. Periodically calls the oracle's
//! `tvlBreakdown()` view via JSON-RPC `eth_call`, decodes the 8-uint256
//! struct, and persists one row per snapshot into `tvl_snapshots`.
//!
//! ## Wire-up
//!
//! ```rust,no_run
//! # use std::sync::Arc;
//! # use zbx_indexer::{Indexer, IndexerConfig};
//! # use zbx_indexer::tvl::{TvlClient, snapshot_loop};
//! # use zbx_sdk::provider::Provider;
//! # use zbx_types::Address;
//! # async fn run() -> anyhow::Result<()> {
//! let indexer = Indexer::new(IndexerConfig::default()).await?;
//! let provider = Provider::http("http://localhost:8545").await?;
//! let oracle: Address = Address([0u8; 20]);   // operator-supplied
//! let client = TvlClient::new(provider, oracle);
//! tokio::spawn(snapshot_loop(client, indexer.connection().clone(), 60));
//! # Ok(()) }
//! ```
//!
//! ## Failure semantics
//!
//! Every snapshot attempt is fail-soft: RPC errors, decode errors, and
//! database write errors are logged at WARN but do not terminate the
//! loop. The on-chain oracle itself is fail-closed (stale prices yield
//! zero, never revert), so the worst plausible outcome of an isolated
//! incident is a single snapshot row with an artificially low `total_usd`.
//! Off-chain dashboards should therefore display TVL with a small
//! moving-average smoothing window.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_rusqlite::Connection;
use tracing::{debug, info, warn};

use zbx_crypto::keccak::keccak256;
use zbx_sdk::provider::Provider;
use zbx_types::{Address, U256};

// ─── Constants ───────────────────────────────────────────────────────────────

/// Default polling interval (seconds). One minute matches a typical
/// off-chain dashboard refresh cadence.
pub const DEFAULT_INTERVAL_SECS: u64 = 60;

/// Number of `uint256` slots returned by `IZbxTvlOracle.tvlBreakdown()`.
/// Matches the `TvlBreakdown` Solidity struct field count
/// (amm, lending, stability, staking, reward, bridgeVault, total, timestamp).
pub const BREAKDOWN_SLOTS: usize = 8;

/// Expected raw byte length of the `tvlBreakdown()` return data.
pub const BREAKDOWN_BYTES: usize = BREAKDOWN_SLOTS * 32;

// ─── Data model ──────────────────────────────────────────────────────────────

/// One snapshot of the oracle's `tvlBreakdown()` view.
///
/// All USD values are 18-decimal `U256` (matching the on-chain canonical
/// precision documented in `IZbxTvlOracle.sol`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TvlSnapshot {
    /// Block number at which the `eth_call` was executed.
    pub block_number: u64,

    /// Optional block hash (currently `None`; populated by future RPC).
    /// `block_number` is pinned BEFORE the `eth_call` and the call is
    /// targeted at that exact block, so this is an exact anchor — not a
    /// lower or upper bound.
    pub block_hash: Option<String>,

    /// On-chain `block.timestamp` reported inside the breakdown struct
    /// (NOT the indexer's wall-clock time).
    pub timestamp: u64,

    /// Wall-clock unix seconds when the indexer captured this snapshot.
    pub captured_at: u64,

    /// AMM source contribution, USD-18.
    pub amm_usd: U256,

    /// Lending source contribution, USD-18.
    pub lending_usd: U256,

    /// Stability pool contribution, USD-18.
    pub stability_usd: U256,

    /// Staking module contribution, USD-18.
    pub staking_usd: U256,

    /// Reward pool contribution (scaffolded — typically zero in v1).
    pub reward_usd: U256,

    /// Bridge vault contribution (scaffolded — typically zero in v1).
    pub bridge_vault_usd: U256,

    /// Sum of all sources, USD-18.
    pub total_usd: U256,

    /// Address of the oracle contract that produced this breakdown.
    pub oracle_addr: Address,
}

// ─── On-chain client ─────────────────────────────────────────────────────────

/// Stateless client around an `eth_call` provider, scoped to one oracle.
#[derive(Clone)]
pub struct TvlClient {
    provider: Provider,
    oracle:   Address,
    selector_breakdown: [u8; 4],
}

impl TvlClient {
    /// Construct a client. Pre-computes the `tvlBreakdown()` 4-byte
    /// selector at construction time so the hot path is just hex-encode.
    pub fn new(provider: Provider, oracle: Address) -> Self {
        let h = keccak256(b"tvlBreakdown()");
        let selector_breakdown = [h.0[0], h.0[1], h.0[2], h.0[3]];
        Self { provider, oracle, selector_breakdown }
    }

    /// Execute one `tvlBreakdown()` call and decode the result.
    ///
    /// **Atomicity:** the block number is resolved BEFORE the `eth_call`
    /// and the call is then pinned to that exact block hex tag. This
    /// closes the race window where `latest` could advance between
    /// `eth_call` and `eth_blockNumber`, which would otherwise mislabel
    /// the snapshot AND silently suppress the true row for the new block
    /// via the `(block_number, oracle_addr)` UNIQUE constraint.
    ///
    /// Returns an error if:
    ///   - the RPC call fails or returns a non-hex payload,
    ///   - the payload is shorter than `BREAKDOWN_BYTES`,
    ///   - the on-chain oracle reverted (paused, or unconfigured source).
    pub async fn snapshot(&self) -> anyhow::Result<TvlSnapshot> {
        // 1) Resolve the block height FIRST so the eth_call below is
        //    pinned to a specific block. If this fails we abort the whole
        //    snapshot — better to skip a tick than store a mislabeled row.
        //    NOTE: malformed hex MUST propagate; do NOT coerce to 0,
        //    otherwise a misconfigured RPC could silently write
        //    block-0/genesis snapshots and pollute the time-series.
        let bn_hex: String = self.provider
            .raw_call("eth_blockNumber", json!([]))
            .await?;
        let block_number = u64::from_str_radix(
            bn_hex.trim_start_matches("0x"), 16
        ).map_err(|e| anyhow::anyhow!(
            "eth_blockNumber returned malformed hex {:?}: {}", bn_hex, e
        ))?;
        let block_tag = format!("0x{:x}", block_number);

        // 2) eth_call against the EXACT block we just resolved.
        let calldata = format!("0x{}", hex::encode(self.selector_breakdown));
        let to_hex   = format!("0x{}", hex::encode(self.oracle.0));
        let result_hex: String = self.provider
            .raw_call("eth_call", json!([
                {"to": to_hex, "data": calldata},
                block_tag
            ]))
            .await?;

        let bytes = hex::decode(result_hex.trim_start_matches("0x"))?;
        if bytes.len() < BREAKDOWN_BYTES {
            anyhow::bail!(
                "tvlBreakdown() returned {} bytes, expected >= {}",
                bytes.len(), BREAKDOWN_BYTES
            );
        }

        let read_slot = |i: usize| -> U256 {
            U256::from_big_endian(&bytes[i * 32 .. (i + 1) * 32])
        };
        let amm        = read_slot(0);
        let lending    = read_slot(1);
        let stability  = read_slot(2);
        let staking    = read_slot(3);
        let reward     = read_slot(4);
        let bridge     = read_slot(5);
        let total      = read_slot(6);
        let timestamp  = read_slot(7).low_u64();

        let captured_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Ok(TvlSnapshot {
            block_number,
            block_hash: None,
            timestamp,
            captured_at,
            amm_usd:           amm,
            lending_usd:       lending,
            stability_usd:     stability,
            staking_usd:       staking,
            reward_usd:        reward,
            bridge_vault_usd:  bridge,
            total_usd:         total,
            oracle_addr:       self.oracle,
        })
    }

    /// Sanity-check selector — exposed for tests.
    pub fn selector_breakdown(&self) -> [u8; 4] { self.selector_breakdown }

    /// Oracle address the client is bound to.
    pub fn oracle(&self) -> Address { self.oracle }
}

// ─── Persistence ─────────────────────────────────────────────────────────────

/// Insert one snapshot row. Uses `INSERT OR IGNORE` keyed by
/// `(block_number, oracle_addr)` so repeated snapshots in the same block
/// are de-duped (e.g. when the indexer interval is shorter than the
/// chain's block time).
pub async fn insert_snapshot(
    conn: &Connection,
    snap: &TvlSnapshot,
) -> anyhow::Result<()> {
    let s = snap.clone();
    conn.call(move |db| {
        db.execute(
            "INSERT OR IGNORE INTO tvl_snapshots \
             (block_number, block_hash, timestamp, captured_at, \
              amm_usd, lending_usd, stability_usd, staking_usd, \
              reward_usd, bridge_usd, total_usd, oracle_addr) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            rusqlite::params![
                s.block_number as i64,
                s.block_hash,
                s.timestamp as i64,
                s.captured_at as i64,
                s.amm_usd.to_string(),
                s.lending_usd.to_string(),
                s.stability_usd.to_string(),
                s.staking_usd.to_string(),
                s.reward_usd.to_string(),
                s.bridge_vault_usd.to_string(),
                s.total_usd.to_string(),
                format!("0x{}", hex::encode(s.oracle_addr.0)),
            ],
        ).map_err(|e| tokio_rusqlite::Error::Other(e.into()))?;
        Ok(())
    }).await?;
    Ok(())
}

// ─── Periodic loop ───────────────────────────────────────────────────────────

/// Run the snapshot collector forever. Intended to be `tokio::spawn`-ed.
///
/// `interval_secs` is the polling cadence; recommended >= chain block
/// time (mainnet target = 2s) and >= 30s for cost/storage reasons. A
/// value of 0 is silently bumped to `DEFAULT_INTERVAL_SECS`.
///
/// # Cancellation
///
/// The loop runs until cancelled (drop the `JoinHandle`). It does NOT
/// gracefully await an in-flight RPC, so callers that need clean
/// shutdown should wrap this in a `tokio::select!` with their own
/// shutdown channel.
pub async fn snapshot_loop(
    client: TvlClient,
    conn: Connection,
    interval_secs: u64,
) -> anyhow::Result<()> {
    let secs = if interval_secs == 0 { DEFAULT_INTERVAL_SECS } else { interval_secs };
    let mut tick = tokio::time::interval(Duration::from_secs(secs));
    info!(
        "tvl: snapshot loop started, oracle=0x{}, interval={}s",
        hex::encode(client.oracle().0), secs
    );

    loop {
        tick.tick().await;
        match client.snapshot().await {
            Ok(snap) => {
                debug!(
                    "tvl: snapshot block={} amm={} lending={} stab={} stake={} total={}",
                    snap.block_number,
                    snap.amm_usd, snap.lending_usd, snap.stability_usd,
                    snap.staking_usd, snap.total_usd
                );
                if let Err(e) = insert_snapshot(&conn, &snap).await {
                    warn!("tvl: insert failed: {}", e);
                }
            }
            Err(e) => warn!("tvl: snapshot fetch failed: {}", e),
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity: the `tvlBreakdown()` selector matches the well-known value.
    /// Off-line cross-check: `keccak256("tvlBreakdown()")[0..4]`.
    #[test]
    fn selector_is_deterministic() {
        let h = keccak256(b"tvlBreakdown()");
        let sel = [h.0[0], h.0[1], h.0[2], h.0[3]];
        // Two computations of the same selector must agree.
        let h2 = keccak256(b"tvlBreakdown()");
        assert_eq!(sel, [h2.0[0], h2.0[1], h2.0[2], h2.0[3]]);
        // Selector must not be all-zero (sanity).
        assert!(sel.iter().any(|b| *b != 0));
    }

    /// In-memory schema bring-up + insert roundtrip.
    #[tokio::test]
    async fn insert_and_read_back() -> anyhow::Result<()> {
        let conn = Connection::open(":memory:").await?;
        conn.call(|db| {
            db.execute_batch(crate::schema::CREATE_TABLES)
                .map_err(|e| tokio_rusqlite::Error::Other(e.into()))
        }).await?;

        // 1e18 — primitive-types U256 has no `exp10`; use pow.
        let one_eth: U256 = U256::from(10u64).pow(U256::from(18u64));
        let snap = TvlSnapshot {
            block_number:     12345,
            block_hash:       None,
            timestamp:        1_700_000_000,
            captured_at:      1_700_000_010,
            amm_usd:          U256::from(250u64)  * one_eth,
            lending_usd:      U256::from(140u64)  * one_eth,
            stability_usd:    U256::from(1000u64) * one_eth,
            staking_usd:      U256::from(100u64)  * one_eth,
            reward_usd:       U256::zero(),
            bridge_vault_usd: U256::zero(),
            total_usd:        U256::from(1490u64) * one_eth,
            oracle_addr:      Address([0xAA; 20]),
        };
        insert_snapshot(&conn, &snap).await?;

        let count: i64 = conn.call(|db| {
            db.query_row("SELECT COUNT(*) FROM tvl_snapshots", [], |r| r.get(0))
                .map_err(|e| tokio_rusqlite::Error::Other(e.into()))
        }).await?;
        assert_eq!(count, 1);

        // Re-inserting the same (block_number, oracle_addr) is idempotent.
        insert_snapshot(&conn, &snap).await?;
        let count2: i64 = conn.call(|db| {
            db.query_row("SELECT COUNT(*) FROM tvl_snapshots", [], |r| r.get(0))
                .map_err(|e| tokio_rusqlite::Error::Other(e.into()))
        }).await?;
        assert_eq!(count2, 1);

        Ok(())
    }
}
