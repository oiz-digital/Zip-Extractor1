//! Re-exported and extended types for SDK users.

pub use zbx_types::{Address, U256, H256};
use serde::{Deserialize, Serialize};

// ── Block ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    pub number:       U256,
    pub hash:         H256,
    pub parent_hash:  H256,
    pub timestamp:    u64,
    pub gas_used:     u64,
    pub gas_limit:    u64,
    pub base_fee_per_gas: Option<U256>,
    pub miner:        Address,
    pub transactions: Vec<H256>,
    pub state_root:   H256,
    pub receipts_root: H256,
    pub transactions_root: H256,
    pub extra_data:   Vec<u8>,
    pub nonce:        u64,
    pub difficulty:   U256,
    pub size:         u64,
}

// ── Transaction ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transaction {
    pub hash:                     H256,
    pub nonce:                    u64,
    pub block_hash:               Option<H256>,
    pub block_number:             Option<u64>,
    pub transaction_index:        Option<u64>,
    pub from:                     Address,
    pub to:                       Option<Address>,
    pub value:                    U256,
    pub gas_price:                Option<U256>,
    pub max_fee_per_gas:          Option<U256>,
    pub max_priority_fee_per_gas: Option<U256>,
    pub gas:                      u64,
    pub input:                    Vec<u8>,
    pub v:                        u64,
    pub r:                        H256,
    pub s:                        H256,
}

// ── Receipt ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub transaction_hash:   H256,
    pub transaction_index:  u64,
    pub block_hash:         H256,
    pub block_number:       u64,
    pub from:               Address,
    pub to:                 Option<Address>,
    pub cumulative_gas_used: u64,
    pub gas_used:           u64,
    pub contract_address:   Option<Address>,
    pub logs:               Vec<Log>,
    pub status:             u8,              // 1 = success, 0 = reverted
    pub effective_gas_price: U256,
}

impl Receipt {
    pub fn success(&self) -> bool { self.status == 1 }
}

// ── Log ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Log {
    pub address:          Address,
    pub topics:           Vec<H256>,
    pub data:             Vec<u8>,
    pub block_number:     Option<u64>,
    pub block_hash:       Option<H256>,
    pub transaction_hash: Option<H256>,
    pub transaction_index: Option<u64>,
    pub log_index:        Option<u64>,
    pub removed:          bool,
}

impl Log {
    pub fn event_signature(&self) -> Option<H256> {
        self.topics.first().copied()
    }
    pub fn indexed_topic(&self, n: usize) -> Option<H256> {
        self.topics.get(n + 1).copied()
    }
}

// ── Chain Constants ────────────────────────────────────────────────────────────
//
// Chain IDs:
//   Mainnet:        8989
//   Testnet+Devnet: 8990  (same chain ID — devnet is "private testnet")
//
// Pre-launch network IDs (7878/7879) are RETIRED and must not be re-used.
//
// The placeholder addresses below contain the digits "7878"/"7879"/"787A" inside
// 20-byte zero-padded slots solely as memorable hex literals; they do NOT encode
// chain IDs and must be replaced by real deployment addresses post-genesis.

pub mod mainnet {
    /// Zebvix Mainnet chain ID.
    pub const CHAIN_ID:    u64  = zbx_types::CHAIN_ID_MAINNET;
    pub const NETWORK:     &str = "mainnet";
    pub const BLOCK_TIME:  u64  = 5;
    pub const MAX_GAS:     u64  = 30_000_000;
    pub const SYMBOL:      &str = "ZBX";
    pub const DECIMALS:    u8   = 18;
    pub const RPC_URL:     &str = "https://rpc.zebvix.com";
    pub const WS_URL:      &str = "wss://ws.zebvix.com";
    pub const EXPLORER:    &str = "https://explorer.zebvix.com";
    pub const MULTICALL3:  &str = "0xcA11bde05977b3631167028862bE2a173976CA11";
    /// Placeholder — replace with real deployed address post-genesis.
    pub const ZRC20:       &str = "0x0000000000000000000000000000000000007878";
    /// Placeholder — replace with real deployed address post-genesis.
    pub const STAKING:     &str = "0x0000000000000000000000000000000000007879";
    /// Placeholder — replace with real deployed address post-genesis.
    pub const BRIDGE:      &str = "0x000000000000000000000000000000000000787A";
}

pub mod devnet {
    /// Devnet chain ID — shares the testnet chain ID by mandate.
    pub const CHAIN_ID: u64  = zbx_types::CHAIN_ID_TESTNET;
    pub const RPC_URL:  &str = "http://localhost:8545";
    pub const WS_URL:   &str = "ws://localhost:8546";
}

pub mod testnet {
    /// Testnet chain ID — shared with devnet.
    pub const CHAIN_ID: u64  = zbx_types::CHAIN_ID_TESTNET;
    pub const RPC_URL:  &str = "https://testnet-rpc.zebvix.com";
    pub const WS_URL:   &str = "wss://testnet-ws.zebvix.com";
    pub const EXPLORER: &str = "https://testnet-explorer.zebvix.com";
}