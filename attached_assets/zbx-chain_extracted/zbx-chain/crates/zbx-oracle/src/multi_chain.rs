//! Multi-chain oracle support — ZBX oracle data on 8 EVM networks.
//!
//! ZBX Chain oracle publishes price feeds natively, but the same price data
//! can be relayed to other EVM networks via the ZBX-XCM cross-chain messaging
//! protocol (ZEP-026). Any DeFi protocol on Ethereum, BSC, Polygon, Arbitrum,
//! Optimism, or Avalanche can consume ZBX oracle data without a separate oracle.
//!
//! ## Supported networks
//!
//! | # | Network | Chain ID | Oracle contract | Update method |
//! |---|---------|----------|-----------------|---------------|
//! | 1 | **ZBX Mainnet** | 8989 | Native | Direct (ZEP-011) |
//! | 2 | **ZBX Testnet** | 8990 | Native | Direct (ZEP-011) |
//! | 3 | **Ethereum** | 1 | `ZbxAggregatorETH.sol` | XCM relay |
//! | 4 | **BSC** | 56 | `ZbxAggregatorBSC.sol` | XCM relay |
//! | 5 | **Polygon** | 137 | `ZbxAggregatorPoly.sol` | XCM relay |
//! | 6 | **Arbitrum One** | 42161 | `ZbxAggregatorArb.sol` | XCM relay |
//! | 7 | **Optimism** | 10 | `ZbxAggregatorOP.sol` | XCM relay |
//! | 8 | **Avalanche C-Chain** | 43114 | `ZbxAggregatorAvax.sol` | XCM relay |
//!
//! ## Relay flow
//!
//! ```text
//! ZBX Oracle (ZEP-011)
//!     └─ Aggregates price (median of N reporters)
//!         └─ Signs with oracle BLS key
//!             └─ ZBX-XCM message → target chain
//!                 └─ ZbxAggregator.sol verifies BLS sig + stores price
//!                     └─ latestRoundData() → DeFi protocol
//! ```
//!
//! ## Chainlink compatibility
//!
//! All relay contracts implement `AggregatorV3Interface`:
//! ```solidity
//! function latestRoundData() external view returns (
//!     uint80 roundId,
//!     int256 answer,
//!     uint256 startedAt,
//!     uint256 updatedAt,
//!     uint80 answeredInRound
//! );
//! ```
//! This means any Chainlink-compatible contract on Ethereum/BSC/Polygon
//! can use ZBX oracle data without code changes.

use crate::feed::{FeedId, Price};
use serde::{Serialize, Deserialize};
use serde_big_array::BigArray;
use std::collections::HashMap;

// ── Network identifiers ───────────────────────────────────────────────────────

/// EVM network chain ID.
pub type ChainId = u64;

/// ZBX Chain Mainnet (Chain ID 8989).
pub const CHAIN_ZBX_MAINNET:   ChainId = 8_989;
/// ZBX Chain Testnet (Chain ID 8990).
pub const CHAIN_ZBX_TESTNET:   ChainId = 8_990;
/// Ethereum Mainnet.
pub const CHAIN_ETHEREUM:      ChainId = 1;
/// BNB Smart Chain (BSC) Mainnet.
pub const CHAIN_BSC:           ChainId = 56;
/// Polygon (Matic) Mainnet.
pub const CHAIN_POLYGON:       ChainId = 137;
/// Arbitrum One.
pub const CHAIN_ARBITRUM:      ChainId = 42_161;
/// Optimism Mainnet.
pub const CHAIN_OPTIMISM:      ChainId = 10;
/// Avalanche C-Chain.
pub const CHAIN_AVALANCHE:     ChainId = 43_114;

/// All supported networks.
pub const ALL_CHAIN_IDS: [ChainId; 8] = [
    CHAIN_ZBX_MAINNET,
    CHAIN_ZBX_TESTNET,
    CHAIN_ETHEREUM,
    CHAIN_BSC,
    CHAIN_POLYGON,
    CHAIN_ARBITRUM,
    CHAIN_OPTIMISM,
    CHAIN_AVALANCHE,
];

// ── Network descriptor ────────────────────────────────────────────────────────

/// Finality model for a network — affects how many confirmations the relay waits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum FinalityModel {
    /// Immediate finality (no re-orgs possible after block N).
    Instant,
    /// Probabilistic finality — wait N blocks.
    Probabilistic { safe_blocks: u32 },
    /// Optimistic rollup — wait for fraud proof window.
    OptimisticRollup { challenge_window_secs: u64 },
    /// ZK rollup — finality when proof is verified on L1.
    ZkRollup,
}

/// Descriptor for one supported EVM network.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkDescriptor {
    pub chain_id:          ChainId,
    pub name:              &'static str,
    pub short_name:        &'static str,
    /// RPC endpoint (production; can be overridden via env).
    pub rpc_endpoint:      &'static str,
    /// Block time in milliseconds (for scheduling relay updates).
    pub block_time_ms:     u64,
    /// Finality model for this network.
    pub finality:          FinalityModel,
    /// Gas token symbol (e.g. "ETH", "BNB", "MATIC").
    pub gas_token:         &'static str,
    /// Whether ZBX oracle relaying is active on this network.
    pub relay_active:      bool,
    /// Number of confirmations required before treating a relay TX as final.
    pub confirm_blocks:    u32,
    /// Native ZBX oracle (no relay needed) — true only for ZBX mainnet/testnet.
    pub is_native:         bool,
    /// Explorer base URL for transaction links.
    pub explorer:          &'static str,
}

impl NetworkDescriptor {
    pub fn zbx_mainnet() -> Self {
        Self {
            chain_id:       CHAIN_ZBX_MAINNET,
            name:           "ZBX Chain Mainnet",
            short_name:     "zbx",
            rpc_endpoint:   "https://rpc.zbx.network",
            block_time_ms:  5_000,
            finality:       FinalityModel::Instant,
            gas_token:      "ZBX",
            relay_active:   true,
            confirm_blocks: 1,
            is_native:      true,
            explorer:       "https://explorer.zbx.network",
        }
    }

    pub fn zbx_testnet() -> Self {
        Self {
            chain_id:       CHAIN_ZBX_TESTNET,
            name:           "ZBX Chain Testnet",
            short_name:     "zbx-test",
            rpc_endpoint:   "https://rpc-testnet.zbx.network",
            block_time_ms:  5_000,
            finality:       FinalityModel::Instant,
            gas_token:      "tZBX",
            relay_active:   true,
            confirm_blocks: 1,
            is_native:      true,
            explorer:       "https://explorer-testnet.zbx.network",
        }
    }

    pub fn ethereum() -> Self {
        Self {
            chain_id:       CHAIN_ETHEREUM,
            name:           "Ethereum Mainnet",
            short_name:     "eth",
            rpc_endpoint:   "https://eth.llamarpc.com",
            block_time_ms:  12_000,
            finality:       FinalityModel::Probabilistic { safe_blocks: 12 },
            gas_token:      "ETH",
            relay_active:   true,
            confirm_blocks: 12,
            is_native:      false,
            explorer:       "https://etherscan.io",
        }
    }

    pub fn bsc() -> Self {
        Self {
            chain_id:       CHAIN_BSC,
            name:           "BNB Smart Chain",
            short_name:     "bsc",
            rpc_endpoint:   "https://bsc-dataseed.binance.org",
            block_time_ms:  3_000,
            finality:       FinalityModel::Probabilistic { safe_blocks: 15 },
            gas_token:      "BNB",
            relay_active:   true,
            confirm_blocks: 15,
            is_native:      false,
            explorer:       "https://bscscan.com",
        }
    }

    pub fn polygon() -> Self {
        Self {
            chain_id:       CHAIN_POLYGON,
            name:           "Polygon Mainnet",
            short_name:     "poly",
            rpc_endpoint:   "https://polygon-rpc.com",
            block_time_ms:  2_000,
            finality:       FinalityModel::Probabilistic { safe_blocks: 128 },
            gas_token:      "MATIC",
            relay_active:   true,
            confirm_blocks: 128,
            is_native:      false,
            explorer:       "https://polygonscan.com",
        }
    }

    pub fn arbitrum() -> Self {
        Self {
            chain_id:       CHAIN_ARBITRUM,
            name:           "Arbitrum One",
            short_name:     "arb",
            rpc_endpoint:   "https://arb1.arbitrum.io/rpc",
            block_time_ms:  250,
            finality:       FinalityModel::OptimisticRollup { challenge_window_secs: 604_800 }, // 7 days
            gas_token:      "ETH",
            relay_active:   true,
            confirm_blocks: 1,
            is_native:      false,
            explorer:       "https://arbiscan.io",
        }
    }

    pub fn optimism() -> Self {
        Self {
            chain_id:       CHAIN_OPTIMISM,
            name:           "Optimism Mainnet",
            short_name:     "op",
            rpc_endpoint:   "https://mainnet.optimism.io",
            block_time_ms:  2_000,
            finality:       FinalityModel::OptimisticRollup { challenge_window_secs: 604_800 }, // 7 days
            gas_token:      "ETH",
            relay_active:   true,
            confirm_blocks: 1,
            is_native:      false,
            explorer:       "https://optimistic.etherscan.io",
        }
    }

    pub fn avalanche() -> Self {
        Self {
            chain_id:       CHAIN_AVALANCHE,
            name:           "Avalanche C-Chain",
            short_name:     "avax",
            rpc_endpoint:   "https://api.avax.network/ext/bc/C/rpc",
            block_time_ms:  2_000,
            finality:       FinalityModel::Instant,
            gas_token:      "AVAX",
            relay_active:   true,
            confirm_blocks: 1,
            is_native:      false,
            explorer:       "https://snowtrace.io",
        }
    }

    /// All supported networks.
    pub fn all() -> Vec<Self> {
        vec![
            Self::zbx_mainnet(),
            Self::zbx_testnet(),
            Self::ethereum(),
            Self::bsc(),
            Self::polygon(),
            Self::arbitrum(),
            Self::optimism(),
            Self::avalanche(),
        ]
    }
}

// ── Per-network price record ──────────────────────────────────────────────────

/// A price record as published on a specific network.
///
/// Different networks may have slightly different prices at any instant
/// due to relay lag and gas economics.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkPrice {
    pub chain_id:       ChainId,
    pub feed_id:        FeedId,
    pub price:          Price,
    /// Round ID on the target network (Chainlink AggregatorV3 compatible).
    pub round_id:       u64,
    /// Unix timestamp when this price was accepted on the target chain.
    pub updated_at:     u64,
    /// Gas cost of the relay transaction (in target chain native token, wei).
    pub relay_gas_used: u64,
    /// Whether this price came from a direct report (native) or relay.
    pub is_relayed:     bool,
}

// ── Relay message ─────────────────────────────────────────────────────────────

/// A signed price update message sent via ZBX-XCM to a target chain.
///
/// The target chain's `ZbxAggregator.sol` verifies the BLS signature
/// against the ZBX oracle committee public key before storing the price.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RelayMessage {
    /// Source chain (always ZBX mainnet).
    pub source_chain:   ChainId,
    /// Target chain.
    pub target_chain:   ChainId,
    /// Which feed this updates.
    pub feed_id:        FeedId,
    /// The new price.
    pub price:          Price,
    /// ZBX block number at which this price was finalized.
    pub zbx_block:      u64,
    /// Oracle round ID (ZBX-native).
    pub round_id:       u64,
    /// Unix timestamp.
    pub timestamp:      u64,
    /// BLS aggregate signature over `(target_chain, feed_id, price, round_id, timestamp)`.
    /// Verified on-chain by `ZbxAggregator.sol::updatePrice()`.
    #[serde(with = "BigArray")]
    pub bls_signature:  [u8; 96],
    /// Bitmap of which oracle committee members signed.
    pub signer_bitmap:  u128,
}

impl RelayMessage {
    /// The canonical signing payload for this relay message.
    /// All oracle signers sign this exact bytes before BLS aggregation.
    pub fn signing_payload(&self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(128);
        payload.extend_from_slice(&self.target_chain.to_be_bytes());
        payload.extend_from_slice(self.feed_id.0.as_bytes());
        payload.extend_from_slice(&self.price.0.to_be_bytes());
        payload.extend_from_slice(&self.round_id.to_be_bytes());
        payload.extend_from_slice(&self.timestamp.to_be_bytes());
        payload
    }
}

// ── Multi-chain registry ──────────────────────────────────────────────────────

/// Registry of all supported networks and their latest published prices.
pub struct MultiChainRegistry {
    networks: HashMap<ChainId, NetworkDescriptor>,
    prices:   HashMap<(ChainId, FeedId), NetworkPrice>,
}

impl MultiChainRegistry {
    pub fn new() -> Self {
        let mut networks = HashMap::new();
        for net in NetworkDescriptor::all() {
            networks.insert(net.chain_id, net);
        }
        Self { networks, prices: HashMap::new() }
    }

    /// All registered networks.
    pub fn networks(&self) -> Vec<&NetworkDescriptor> {
        self.networks.values().collect()
    }

    /// Number of supported networks.
    pub fn network_count(&self) -> usize { self.networks.len() }

    /// Number of active relay networks (non-native).
    pub fn relay_network_count(&self) -> usize {
        self.networks.values().filter(|n| !n.is_native && n.relay_active).count()
    }

    /// Get a network descriptor by chain ID.
    pub fn network(&self, chain_id: ChainId) -> Option<&NetworkDescriptor> {
        self.networks.get(&chain_id)
    }

    /// Record a price update received from (or confirmed on) a network.
    pub fn record_price(&mut self, price: NetworkPrice) {
        let key = (price.chain_id, price.feed_id.clone());
        self.prices.insert(key, price);
    }

    /// Latest price for a feed on a specific network.
    pub fn latest_price(&self, chain_id: ChainId, feed_id: &FeedId)
        -> Option<&NetworkPrice>
    {
        self.prices.get(&(chain_id, feed_id.clone()))
    }

    /// All prices for a feed across all networks.
    pub fn cross_chain_prices(&self, feed_id: &FeedId) -> Vec<&NetworkPrice> {
        self.prices.iter()
            .filter(|((_, fid), _)| fid == feed_id)
            .map(|(_, p)| p)
            .collect()
    }

    /// Networks that need a relay update for a feed (price older than threshold).
    pub fn stale_relays(&self, feed_id: &FeedId, now: u64, max_age_secs: u64)
        -> Vec<ChainId>
    {
        self.networks.values()
            .filter(|n| !n.is_native && n.relay_active)
            .filter(|n| {
                match self.prices.get(&(n.chain_id, feed_id.clone())) {
                    None    => true,  // never published → stale
                    Some(p) => now.saturating_sub(p.updated_at) > max_age_secs,
                }
            })
            .map(|n| n.chain_id)
            .collect()
    }

    /// Whether a chain ID is a native ZBX chain (mainnet or testnet).
    pub fn is_native(&self, chain_id: ChainId) -> bool {
        matches!(chain_id, CHAIN_ZBX_MAINNET | CHAIN_ZBX_TESTNET)
    }
}

impl Default for MultiChainRegistry {
    fn default() -> Self { Self::new() }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eight_networks_registered() {
        let reg = MultiChainRegistry::new();
        assert_eq!(reg.network_count(), 8,
            "must have exactly 8 supported networks");
    }

    #[test]
    fn native_networks_identified() {
        let reg = MultiChainRegistry::new();
        assert!(reg.is_native(CHAIN_ZBX_MAINNET));
        assert!(reg.is_native(CHAIN_ZBX_TESTNET));
        assert!(!reg.is_native(CHAIN_ETHEREUM));
        assert!(!reg.is_native(CHAIN_AVALANCHE));
    }

    #[test]
    fn six_relay_networks() {
        let reg = MultiChainRegistry::new();
        // 8 total - 2 native = 6 relay networks
        assert_eq!(reg.relay_network_count(), 6,
            "ETH, BSC, Polygon, Arbitrum, Optimism, Avalanche = 6 relay chains");
    }

    #[test]
    fn chain_ids_correct() {
        let reg = MultiChainRegistry::new();
        assert_eq!(reg.network(CHAIN_ZBX_MAINNET).unwrap().chain_id, 8989);
        assert_eq!(reg.network(CHAIN_ZBX_TESTNET).unwrap().chain_id, 8990);
        assert_eq!(reg.network(CHAIN_ETHEREUM).unwrap().chain_id, 1);
        assert_eq!(reg.network(CHAIN_BSC).unwrap().chain_id, 56);
        assert_eq!(reg.network(CHAIN_POLYGON).unwrap().chain_id, 137);
        assert_eq!(reg.network(CHAIN_ARBITRUM).unwrap().chain_id, 42_161);
        assert_eq!(reg.network(CHAIN_OPTIMISM).unwrap().chain_id, 10);
        assert_eq!(reg.network(CHAIN_AVALANCHE).unwrap().chain_id, 43_114);
    }

    #[test]
    fn record_and_query_price() {
        let mut reg = MultiChainRegistry::new();
        let price = NetworkPrice {
            chain_id:       CHAIN_ETHEREUM,
            feed_id:        FeedId::zbx_usd(),
            price:          Price::from_f64(2.50),
            round_id:       42,
            updated_at:     10_000,
            relay_gas_used: 150_000,
            is_relayed:     true,
        };
        reg.record_price(price);
        let p = reg.latest_price(CHAIN_ETHEREUM, &FeedId::zbx_usd()).unwrap();
        assert_eq!(p.round_id, 42);
        assert!((p.price.to_f64() - 2.50).abs() < 0.001);
    }

    #[test]
    fn stale_relay_detection() {
        let mut reg = MultiChainRegistry::new();
        // Record fresh price on Ethereum (now = 20_000)
        reg.record_price(NetworkPrice {
            chain_id: CHAIN_ETHEREUM, feed_id: FeedId::zbx_usd(),
            price: Price::from_f64(2.50), round_id: 1,
            updated_at: 19_000, relay_gas_used: 0, is_relayed: true,
        });
        let stale = reg.stale_relays(&FeedId::zbx_usd(), 20_000, 3_600);
        // Ethereum: updated 1000s ago, max 3600 → NOT stale
        assert!(!stale.contains(&CHAIN_ETHEREUM));
        // BSC, Polygon, etc.: never published → stale
        assert!(stale.contains(&CHAIN_BSC));
        assert!(stale.contains(&CHAIN_POLYGON));
    }

    #[test]
    fn relay_signing_payload_is_deterministic() {
        let msg = RelayMessage {
            source_chain:  CHAIN_ZBX_MAINNET,
            target_chain:  CHAIN_ETHEREUM,
            feed_id:       FeedId::zbx_usd(),
            price:         Price::from_f64(2.50),
            zbx_block:     1_000,
            round_id:      42,
            timestamp:     9_999,
            bls_signature: [0u8; 96],
            signer_bitmap: 0b11111,
        };
        let p1 = msg.signing_payload();
        let p2 = msg.signing_payload();
        assert_eq!(p1, p2, "signing payload must be deterministic");
        assert!(!p1.is_empty());
    }
}
