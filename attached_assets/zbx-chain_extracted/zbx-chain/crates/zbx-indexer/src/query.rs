//! Query engine: rich queries over the indexed data.

use zbx_types::{Address, H256};
use serde::{Deserialize, Serialize};

/// Transaction query filter.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TxFilter {
    pub from:         Option<String>,
    pub to:           Option<String>,
    pub from_block:   Option<u64>,
    pub to_block:     Option<u64>,
    pub min_value:    Option<String>,
    pub page:         Option<u64>,
    pub page_size:    Option<u64>,
}

/// Log query filter.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LogFilter {
    pub contract:   Option<String>,
    pub topic0:     Option<String>,
    pub topic1:     Option<String>,
    pub from_block: Option<u64>,
    pub to_block:   Option<u64>,
    pub page:       Option<u64>,
    pub page_size:  Option<u64>,
}

/// A transaction result row.
#[derive(Debug, Clone, Serialize)]
pub struct TxRow {
    pub hash:         String,
    pub block_number: u64,
    pub from_addr:    String,
    pub to_addr:      Option<String>,
    pub value:        String,
    pub gas_used:     u64,
    pub success:      bool,
}

/// A log result row.
#[derive(Debug, Clone, Serialize)]
pub struct LogRow {
    pub id:           i64,
    pub block_number: u64,
    pub tx_hash:      String,
    pub contract:     String,
    pub topic0:       Option<String>,
    pub data:         Option<String>,
}

/// Paginated result.
#[derive(Debug, Clone, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub page:  u64,
    pub pages: u64,
}

// ─── ZEP-007 TVL query types (S17 Phase 3) ───────────────────────────────────

/// Filter for `query_tvl_history`. Time bounds are unix seconds against
/// the on-chain `block.timestamp` column (NOT the indexer's wall-clock
/// `captured_at`). Specify `oracle` (or its alias `oracle_addr`) to
/// disambiguate when the indexer is collecting from multiple oracle
/// deployments.
///
/// The query-string field name is `oracle` to match the other TVL routes
/// (`/v1/tvl`, `/v1/tvl/global`, `/v1/tvl/by-source`); the alternate
/// `oracle_addr` form is accepted for backwards compatibility.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TvlHistoryFilter {
    #[serde(rename = "oracle", alias = "oracle_addr")]
    pub oracle_addr: Option<String>,
    pub from_ts:     Option<u64>,
    pub to_ts:       Option<u64>,
    pub page:        Option<u64>,
    pub page_size:   Option<u64>,
}

/// One row of the `tvl_snapshots` table, USD values rendered as decimal
/// strings to preserve U256 precision through JSON.
#[derive(Debug, Clone, Serialize)]
pub struct TvlSnapshotRow {
    pub block_number:  u64,
    pub block_hash:    Option<String>,
    pub timestamp:     u64,
    pub captured_at:   u64,
    pub amm_usd:       String,
    pub lending_usd:   String,
    pub stability_usd: String,
    pub staking_usd:   String,
    pub reward_usd:    String,
    pub bridge_usd:    String,
    pub total_usd:     String,
    pub oracle_addr:   String,
}

/// Compact projection used by `/v1/tvl/by-source`.
#[derive(Debug, Clone, Serialize)]
pub struct TvlBySourceRow {
    pub block_number:  u64,
    pub timestamp:     u64,
    pub amm_usd:       String,
    pub lending_usd:   String,
    pub stability_usd: String,
    pub staking_usd:   String,
    pub reward_usd:    String,
    pub bridge_usd:    String,
    pub oracle_addr:   String,
}

/// Compact projection used by `/v1/tvl/global` — just the headline number.
#[derive(Debug, Clone, Serialize)]
pub struct TvlGlobalRow {
    pub block_number: u64,
    pub timestamp:    u64,
    pub total_usd:    String,
    pub oracle_addr:  String,
}

/// Query engine backed by SQLite.
pub struct QueryEngine {
    conn: tokio_rusqlite::Connection,
}

impl QueryEngine {
    pub fn new(conn: tokio_rusqlite::Connection) -> Self {
        Self { conn }
    }

    // ─── TVL queries (S17 Phase 3) ───────────────────────────────────────────

    /// Latest snapshot, optionally scoped to a specific oracle address.
    /// Returns `None` if the indexer has not yet recorded any snapshots.
    pub async fn query_latest_tvl(&self, oracle_addr: Option<String>)
        -> anyhow::Result<Option<TvlSnapshotRow>>
    {
        let oracle = oracle_addr.map(|s| s.to_lowercase());
        let result = self.conn.call(move |db| {
            let (sql, oracle_param): (&str, Option<String>) = match &oracle {
                Some(o) => (
                    "SELECT block_number, block_hash, timestamp, captured_at, \
                     amm_usd, lending_usd, stability_usd, staking_usd, \
                     reward_usd, bridge_usd, total_usd, oracle_addr \
                     FROM tvl_snapshots \
                     WHERE LOWER(oracle_addr) = ?1 \
                     ORDER BY block_number DESC LIMIT 1",
                    Some(o.clone()),
                ),
                None => (
                    "SELECT block_number, block_hash, timestamp, captured_at, \
                     amm_usd, lending_usd, stability_usd, staking_usd, \
                     reward_usd, bridge_usd, total_usd, oracle_addr \
                     FROM tvl_snapshots \
                     ORDER BY block_number DESC LIMIT 1",
                    None,
                ),
            };
            let mut stmt = db.prepare(sql)
                .map_err(|e| tokio_rusqlite::Error::Other(e.into()))?;
            let map = |r: &rusqlite::Row<'_>| -> rusqlite::Result<TvlSnapshotRow> {
                Ok(TvlSnapshotRow {
                    block_number:  r.get::<_, i64>(0)? as u64,
                    block_hash:    r.get(1)?,
                    timestamp:     r.get::<_, i64>(2)? as u64,
                    captured_at:   r.get::<_, i64>(3)? as u64,
                    amm_usd:       r.get(4)?,
                    lending_usd:   r.get(5)?,
                    stability_usd: r.get(6)?,
                    staking_usd:   r.get(7)?,
                    reward_usd:    r.get(8)?,
                    bridge_usd:    r.get(9)?,
                    total_usd:     r.get(10)?,
                    oracle_addr:   r.get(11)?,
                })
            };
            let row = match oracle_param {
                Some(o) => stmt.query_row(rusqlite::params![o], map),
                None    => stmt.query_row([], map),
            };
            match row {
                Ok(r)                                          => Ok(Some(r)),
                Err(rusqlite::Error::QueryReturnedNoRows)      => Ok(None),
                Err(e)                                         => Err(tokio_rusqlite::Error::Other(e.into())),
            }
        }).await?;
        Ok(result)
    }

    /// Latest snapshot projected down to just the headline `total_usd`.
    pub async fn query_tvl_global(&self, oracle_addr: Option<String>)
        -> anyhow::Result<Option<TvlGlobalRow>>
    {
        Ok(self.query_latest_tvl(oracle_addr).await?.map(|s| TvlGlobalRow {
            block_number: s.block_number,
            timestamp:    s.timestamp,
            total_usd:    s.total_usd,
            oracle_addr:  s.oracle_addr,
        }))
    }

    /// Latest snapshot projected down to per-source breakdowns.
    pub async fn query_tvl_by_source(&self, oracle_addr: Option<String>)
        -> anyhow::Result<Option<TvlBySourceRow>>
    {
        Ok(self.query_latest_tvl(oracle_addr).await?.map(|s| TvlBySourceRow {
            block_number:  s.block_number,
            timestamp:     s.timestamp,
            amm_usd:       s.amm_usd,
            lending_usd:   s.lending_usd,
            stability_usd: s.stability_usd,
            staking_usd:   s.staking_usd,
            reward_usd:    s.reward_usd,
            bridge_usd:    s.bridge_usd,
            oracle_addr:   s.oracle_addr,
        }))
    }

    /// Paginated time-series. `page` is 1-indexed; `page_size` is
    /// clamped to `[1, 1000]` (default 100). Time bounds are inclusive.
    pub async fn query_tvl_history(&self, filter: TvlHistoryFilter)
        -> anyhow::Result<Page<TvlSnapshotRow>>
    {
        let page      = filter.page.unwrap_or(1).max(1);
        let page_size = filter.page_size.unwrap_or(100).clamp(1, 1000);
        let offset    = (page - 1) * page_size;

        let oracle  = filter.oracle_addr.clone().map(|s| s.to_lowercase());
        let from_ts = filter.from_ts;
        let to_ts   = filter.to_ts;

        let result = self.conn.call(move |db| {
            let mut where_clauses: Vec<String> = Vec::new();
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(ref o) = oracle {
                where_clauses.push("LOWER(oracle_addr) = ?".into());
                params.push(Box::new(o.clone()));
            }
            if let Some(f) = from_ts {
                where_clauses.push("timestamp >= ?".into());
                params.push(Box::new(f as i64));
            }
            if let Some(t) = to_ts {
                where_clauses.push("timestamp <= ?".into());
                params.push(Box::new(t as i64));
            }
            let where_str = if where_clauses.is_empty() {
                "1=1".to_string()
            } else {
                where_clauses.join(" AND ")
            };

            // Total count for pagination metadata.
            let count_sql = format!(
                "SELECT COUNT(*) FROM tvl_snapshots WHERE {}", where_str
            );
            let param_refs: Vec<&dyn rusqlite::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();
            let total: i64 = db.query_row(
                &count_sql,
                param_refs.as_slice(),
                |r| r.get(0),
            ).map_err(|e| tokio_rusqlite::Error::Other(e.into()))?;

            let sql = format!(
                "SELECT block_number, block_hash, timestamp, captured_at, \
                 amm_usd, lending_usd, stability_usd, staking_usd, \
                 reward_usd, bridge_usd, total_usd, oracle_addr \
                 FROM tvl_snapshots WHERE {} \
                 ORDER BY block_number DESC LIMIT {} OFFSET {}",
                where_str, page_size, offset
            );
            let mut stmt = db.prepare(&sql)
                .map_err(|e| tokio_rusqlite::Error::Other(e.into()))?;
            let rows: Vec<TvlSnapshotRow> = stmt.query_map(
                param_refs.as_slice(),
                |r| Ok(TvlSnapshotRow {
                    block_number:  r.get::<_, i64>(0)? as u64,
                    block_hash:    r.get(1)?,
                    timestamp:     r.get::<_, i64>(2)? as u64,
                    captured_at:   r.get::<_, i64>(3)? as u64,
                    amm_usd:       r.get(4)?,
                    lending_usd:   r.get(5)?,
                    stability_usd: r.get(6)?,
                    staking_usd:   r.get(7)?,
                    reward_usd:    r.get(8)?,
                    bridge_usd:    r.get(9)?,
                    total_usd:     r.get(10)?,
                    oracle_addr:   r.get(11)?,
                })
            )
            .map_err(|e| tokio_rusqlite::Error::Other(e.into()))?
            .filter_map(|r| r.ok())
            .collect();

            let total_u = total as u64;
            let pages   = (total_u + page_size - 1).max(1) / page_size.max(1);
            Ok(Page {
                items: rows,
                total: total_u,
                page,
                pages,
            })
        }).await?;
        Ok(result)
    }

    /// H-8 fix: query log entries from the indexed log store with filtering.
    /// Previously the /v1/logs endpoint returned a static empty placeholder.
    /// This method executes the actual SQL query against the log table.
    pub async fn query_logs(&self, filter: LogFilter) -> anyhow::Result<Page<LogRow>> {
        let page      = filter.page.unwrap_or(1).max(1);
        let page_size = filter.page_size.unwrap_or(25).min(1000);
        let offset    = (page - 1) * page_size;

        let filter = filter.clone();
        let result = self.conn.call(move |db| {
            let mut where_clauses = Vec::new();
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

            if let Some(ref c) = filter.contract {
                where_clauses.push("contract = ?");
                params.push(Box::new(c.clone()));
            }
            if let Some(ref t0) = filter.topic0 {
                where_clauses.push("topic0 = ?");
                params.push(Box::new(t0.clone()));
            }
            if let Some(ref t1) = filter.topic1 {
                where_clauses.push("topic1 = ?");
                params.push(Box::new(t1.clone()));
            }
            if let Some(fb) = filter.from_block {
                where_clauses.push("block_number >= ?");
                params.push(Box::new(fb as i64));
            }
            if let Some(tb) = filter.to_block {
                where_clauses.push("block_number <= ?");
                params.push(Box::new(tb as i64));
            }

            let where_str = if where_clauses.is_empty() {
                "1=1".to_string()
            } else {
                where_clauses.join(" AND ")
            };

            let sql = format!(
                "SELECT id, block_number, tx_hash, contract, topic0, data \
                 FROM logs WHERE {} ORDER BY block_number DESC LIMIT {} OFFSET {}",
                where_str, page_size, offset
            );

            let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
            let mut stmt = db.prepare(&sql)
                .map_err(|e| tokio_rusqlite::Error::Other(e.into()))?;

            let rows: Vec<LogRow> = stmt.query_map(param_refs.as_slice(), |r| {
                Ok(LogRow {
                    id:           r.get(0)?,
                    block_number: r.get::<_, i64>(1)? as u64,
                    tx_hash:      r.get(2)?,
                    contract:     r.get(3)?,
                    topic0:       r.get(4)?,
                    data:         r.get(5)?,
                })
            })
            .map_err(|e| tokio_rusqlite::Error::Other(e.into()))?
            .filter_map(|r| r.ok())
            .collect();

            let total  = rows.len() as u64;
            let pages  = (total + page_size - 1).max(1) / page_size.max(1);
            Ok(Page { items: rows, total, page, pages })
        }).await?;
        Ok(result)
    }

    pub async fn query_transactions(&self, filter: TxFilter) -> anyhow::Result<Page<TxRow>> {
        let page = filter.page.unwrap_or(1).max(1);
        let page_size = filter.page_size.unwrap_or(25).min(1000);
        let offset = (page - 1) * page_size;

        let filter = filter.clone();
        let result = self.conn.call(move |db| {
            let mut where_clauses = Vec::new();
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

            if let Some(ref from) = filter.from {
                where_clauses.push("from_addr = ?");
                params.push(Box::new(from.clone()));
            }
            if let Some(ref to) = filter.to {
                where_clauses.push("to_addr = ?");
                params.push(Box::new(to.clone()));
            }
            if let Some(fb) = filter.from_block {
                where_clauses.push("block_number >= ?");
                params.push(Box::new(fb as i64));
            }
            if let Some(tb) = filter.to_block {
                where_clauses.push("block_number <= ?");
                params.push(Box::new(tb as i64));
            }

            let where_str = if where_clauses.is_empty() {
                "1=1".to_string()
            } else {
                where_clauses.join(" AND ")
            };

            let sql = format!(
                "SELECT hash, block_number, from_addr, to_addr, value, gas_used, success \
                 FROM transactions WHERE {} ORDER BY block_number DESC LIMIT {} OFFSET {}",
                where_str, page_size, offset
            );

            let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
            let mut stmt = db.prepare(&sql)
                .map_err(|e| tokio_rusqlite::Error::Other(e.into()))?;

            let rows: Vec<TxRow> = stmt.query_map(param_refs.as_slice(), |r| {
                Ok(TxRow {
                    hash:         r.get(0)?,
                    block_number: r.get::<_, i64>(1)? as u64,
                    from_addr:    r.get(2)?,
                    to_addr:      r.get(3)?,
                    value:        r.get(4)?,
                    gas_used:     r.get::<_, i64>(5)? as u64,
                    success:      r.get::<_, i32>(6)? != 0,
                })
            })
            .map_err(|e| tokio_rusqlite::Error::Other(e.into()))?
            .filter_map(|r| r.ok())
            .collect();

            Ok(Page {
                total: rows.len() as u64,
                pages: (rows.len() as u64 + page_size - 1) / page_size,
                page,
                items: rows,
            })
        }).await?;
        Ok(result)
    }
}