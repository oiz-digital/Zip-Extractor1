//! SQLite schema for the indexer database.

/// SQL DDL for the indexer schema.
pub const CREATE_TABLES: &str = r#"
CREATE TABLE IF NOT EXISTS blocks (
    number       INTEGER PRIMARY KEY,
    hash         TEXT    NOT NULL UNIQUE,
    parent_hash  TEXT    NOT NULL,
    timestamp    INTEGER NOT NULL,
    gas_used     INTEGER NOT NULL,
    gas_limit    INTEGER NOT NULL,
    base_fee     TEXT    NOT NULL,
    tx_count     INTEGER NOT NULL,
    coinbase     TEXT    NOT NULL,
    state_root   TEXT    NOT NULL
);

CREATE TABLE IF NOT EXISTS transactions (
    hash         TEXT    PRIMARY KEY,
    block_number INTEGER NOT NULL REFERENCES blocks(number),
    block_hash   TEXT    NOT NULL,
    tx_index     INTEGER NOT NULL,
    from_addr    TEXT    NOT NULL,
    to_addr      TEXT,
    value        TEXT    NOT NULL,
    gas_limit    INTEGER NOT NULL,
    gas_used     INTEGER NOT NULL,
    gas_price    TEXT    NOT NULL,
    nonce        INTEGER NOT NULL,
    input        TEXT,
    success      INTEGER NOT NULL,
    contract_addr TEXT
);

CREATE TABLE IF NOT EXISTS logs (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    block_number INTEGER NOT NULL REFERENCES blocks(number),
    block_hash   TEXT    NOT NULL,
    tx_hash      TEXT    NOT NULL REFERENCES transactions(hash),
    tx_index     INTEGER NOT NULL,
    log_index    INTEGER NOT NULL,
    contract     TEXT    NOT NULL,
    topic0       TEXT,
    topic1       TEXT,
    topic2       TEXT,
    topic3       TEXT,
    data         TEXT
);

CREATE TABLE IF NOT EXISTS token_transfers (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    tx_hash      TEXT    NOT NULL,
    log_index    INTEGER NOT NULL,
    block_number INTEGER NOT NULL,
    token        TEXT    NOT NULL,
    from_addr    TEXT    NOT NULL,
    to_addr      TEXT    NOT NULL,
    amount       TEXT    NOT NULL
);

CREATE TABLE IF NOT EXISTS contracts (
    address      TEXT    PRIMARY KEY,
    creator      TEXT    NOT NULL,
    tx_hash      TEXT    NOT NULL,
    block_number INTEGER NOT NULL,
    code_hash    TEXT    NOT NULL,
    created_at   INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_tx_from       ON transactions(from_addr);
CREATE INDEX IF NOT EXISTS idx_tx_to         ON transactions(to_addr);
CREATE INDEX IF NOT EXISTS idx_tx_block      ON transactions(block_number);
CREATE INDEX IF NOT EXISTS idx_log_contract  ON logs(contract);
CREATE INDEX IF NOT EXISTS idx_log_topic0    ON logs(topic0);
CREATE INDEX IF NOT EXISTS idx_log_block     ON logs(block_number);
CREATE INDEX IF NOT EXISTS idx_transfer_from ON token_transfers(from_addr);
CREATE INDEX IF NOT EXISTS idx_transfer_to   ON token_transfers(to_addr);
CREATE INDEX IF NOT EXISTS idx_transfer_token ON token_transfers(token);

-- ─── ZEP-007 TVL aggregator time-series (S17 Phase 2) ──────────────────────
-- One row per snapshot of the ZbxTvlOracle.tvlBreakdown() call. U256 USD
-- values are stored as decimal-string TEXT for lossless round-trip.
CREATE TABLE IF NOT EXISTS tvl_snapshots (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    block_number  INTEGER NOT NULL,
    block_hash    TEXT,
    timestamp     INTEGER NOT NULL,    -- on-chain block.timestamp from breakdown
    captured_at   INTEGER NOT NULL,    -- wall-clock unix seconds (indexer write time)
    amm_usd       TEXT    NOT NULL,    -- U256 decimal string, USD-18
    lending_usd   TEXT    NOT NULL,
    stability_usd TEXT    NOT NULL,
    staking_usd   TEXT    NOT NULL,
    reward_usd    TEXT    NOT NULL,
    bridge_usd    TEXT    NOT NULL,
    total_usd     TEXT    NOT NULL,
    oracle_addr   TEXT    NOT NULL,
    UNIQUE (block_number, oracle_addr)
);

CREATE INDEX IF NOT EXISTS idx_tvl_block        ON tvl_snapshots(block_number);
CREATE INDEX IF NOT EXISTS idx_tvl_ts           ON tvl_snapshots(timestamp);
CREATE INDEX IF NOT EXISTS idx_tvl_oracle       ON tvl_snapshots(oracle_addr);
CREATE INDEX IF NOT EXISTS idx_tvl_captured_at  ON tvl_snapshots(captured_at);
"#;

/// ERC-20 Transfer event topic (keccak256("Transfer(address,address,uint256)")).
pub const ERC20_TRANSFER_TOPIC: &str =
    "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

/// ERC-721 Transfer event topic.
pub const ERC721_TRANSFER_TOPIC: &str =
    "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";