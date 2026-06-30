//! Core indexer: processes blocks and writes to the database.

use crate::schema::CREATE_TABLES;
use zbx_types::{Address, H256, U256};
use tokio_rusqlite::Connection;
use tracing::{info, debug, warn};

/// Indexer configuration.
#[derive(Debug, Clone)]
pub struct IndexerConfig {
    /// Path to the SQLite database file.
    pub db_path: String,
    /// How many blocks to index per batch.
    pub batch_size: usize,
    /// Whether to index internal transactions (call traces).
    pub index_traces: bool,
    /// Whether to decode ERC-20 transfer events.
    pub decode_transfers: bool,
    /// Number of concurrent writer threads.
    pub writer_threads: usize,
}

impl Default for IndexerConfig {
    fn default() -> Self {
        Self {
            db_path: "./indexer.db".into(),
            batch_size: 100,
            index_traces: true,
            decode_transfers: true,
            writer_threads: 4,
        }
    }
}

/// A block to be indexed.
#[derive(Debug, Clone)]
pub struct IndexBlock {
    pub number:     u64,
    pub hash:       H256,
    pub parent_hash: H256,
    pub timestamp:  u64,
    pub gas_used:   u64,
    pub gas_limit:  u64,
    pub base_fee:   U256,
    pub coinbase:   Address,
    pub state_root: H256,
    pub txs:        Vec<IndexTx>,
}

/// A transaction to be indexed.
#[derive(Debug, Clone)]
pub struct IndexTx {
    pub hash:          H256,
    pub tx_index:      usize,
    pub from:          Address,
    pub to:            Option<Address>,
    pub value:         U256,
    pub gas_limit:     u64,
    pub gas_used:      u64,
    pub gas_price:     U256,
    pub nonce:         u64,
    pub input:         Vec<u8>,
    pub success:       bool,
    pub contract_addr: Option<Address>,
    pub logs:          Vec<IndexLog>,
}

/// An EVM log to be indexed.
#[derive(Debug, Clone)]
pub struct IndexLog {
    pub log_index: usize,
    pub contract:  Address,
    pub topics:    Vec<H256>,
    pub data:      Vec<u8>,
}

/// The main indexer.
pub struct Indexer {
    config: IndexerConfig,
    conn:   Connection,
}

impl Indexer {
    pub async fn new(config: IndexerConfig) -> anyhow::Result<Self> {
        let conn = Connection::open(&config.db_path).await?;
        // Initialize schema.
        conn.call(|db| {
            db.execute_batch(CREATE_TABLES).map_err(|e| tokio_rusqlite::Error::Other(e.into()))
        }).await?;
        info!("indexer: database initialized at {}", config.db_path);
        Ok(Self { config, conn })
    }

    /// Index a batch of blocks.
    pub async fn index_blocks(&self, blocks: Vec<IndexBlock>) -> anyhow::Result<()> {
        info!("indexer: indexing {} blocks", blocks.len());
        for block in &blocks {
            self.index_block(block).await?;
        }
        Ok(())
    }

    async fn index_block(&self, block: &IndexBlock) -> anyhow::Result<()> {
        let b = block.clone();
        self.conn.call(move |db| {
            let tx = db.transaction().map_err(|e| tokio_rusqlite::Error::Other(e.into()))?;
            // Insert block.
            tx.execute(
                "INSERT OR IGNORE INTO blocks (number, hash, parent_hash, timestamp, \
                 gas_used, gas_limit, base_fee, tx_count, coinbase, state_root) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                rusqlite::params![
                    b.number, format!("{:?}", b.hash), format!("{:?}", b.parent_hash),
                    b.timestamp, b.gas_used, b.gas_limit,
                    b.base_fee.to_string(), b.txs.len(),
                    format!("{:?}", b.coinbase), format!("{:?}", b.state_root),
                ],
            ).map_err(|e| tokio_rusqlite::Error::Other(e.into()))?;

            // Insert transactions.
            for itx in &b.txs {
                tx.execute(
                    "INSERT OR IGNORE INTO transactions \
                     (hash, block_number, block_hash, tx_index, from_addr, to_addr, \
                      value, gas_limit, gas_used, gas_price, nonce, success) \
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                    rusqlite::params![
                        format!("{:?}", itx.hash), b.number, format!("{:?}", b.hash),
                        itx.tx_index, format!("{:?}", itx.from),
                        itx.to.map(|a| format!("{:?}", a)),
                        itx.value.to_string(), itx.gas_limit, itx.gas_used,
                        itx.gas_price.to_string(), itx.nonce, itx.success as i32,
                    ],
                ).map_err(|e| tokio_rusqlite::Error::Other(e.into()))?;

                // Insert logs.
                for log in &itx.logs {
                    tx.execute(
                        "INSERT INTO logs \
                         (block_number, block_hash, tx_hash, tx_index, log_index, contract, \
                          topic0, topic1, topic2, topic3, data) \
                         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                        rusqlite::params![
                            b.number, format!("{:?}", b.hash),
                            format!("{:?}", itx.hash), itx.tx_index, log.log_index,
                            format!("{:?}", log.contract),
                            log.topics.get(0).map(|t| format!("{:?}", t)),
                            log.topics.get(1).map(|t| format!("{:?}", t)),
                            log.topics.get(2).map(|t| format!("{:?}", t)),
                            log.topics.get(3).map(|t| format!("{:?}", t)),
                            hex::encode(&log.data),
                        ],
                    ).map_err(|e| tokio_rusqlite::Error::Other(e.into()))?;
                }
            }

            tx.commit().map_err(|e| tokio_rusqlite::Error::Other(e.into()))
        }).await?;
        debug!("indexer: indexed block {}", block.number);
        Ok(())
    }

    /// Borrow the underlying SQLite connection. Used by ancillary
    /// collectors (e.g. `tvl::snapshot_loop`) that need to share the
    /// indexer's database without re-opening the file.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Get the highest indexed block number.
    pub async fn highest_block(&self) -> anyhow::Result<Option<u64>> {
        let result = self.conn.call(|db| {
            let mut stmt = db.prepare("SELECT MAX(number) FROM blocks")
                .map_err(|e| tokio_rusqlite::Error::Other(e.into()))?;
            let n: Option<i64> = stmt.query_row([], |r| r.get(0))
                .map_err(|e| tokio_rusqlite::Error::Other(e.into()))?;
            Ok(n.map(|v| v as u64))
        }).await?;
        Ok(result)
    }
}