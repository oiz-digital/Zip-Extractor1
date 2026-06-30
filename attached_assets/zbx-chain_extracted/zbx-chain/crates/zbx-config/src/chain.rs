//! Chain-level constants and parameters.

use serde::{Deserialize, Serialize};

// Chain IDs are the single source of truth in `zbx-types`. Re-exported here for
// backward compatibility — new code should import directly from `zbx_types`.
pub use zbx_types::{CHAIN_ID_MAINNET, CHAIN_ID_TESTNET};

pub const ZBX_BLOCK_TIME:     u64 = 5;
pub const ZBX_GAS_LIMIT:      u64 = 30_000_000;
pub const ZBX_BASE_FEE_GWEI:  u64 = 1_000_000_000;
pub const ZBX_MAX_VALIDATORS: u32 = 100;
pub const ZBX_EPOCH_LENGTH:   u64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainConfig {
    pub chain_id:             u64,
    pub name:                 String,
    pub symbol:               String,
    pub decimals:             u8,
    pub block_time_secs:      u64,
    pub gas_limit:            u64,
    pub initial_base_fee:     u64,
    pub max_validators:       u32,
    pub epoch_length:         u64,
    pub halving_interval:     u64,
    pub initial_block_reward: u64,
}

impl Default for ChainConfig {
    fn default() -> Self {
        Self {
            chain_id: CHAIN_ID_MAINNET, name: "Zebvix Chain".into(), symbol: "ZBX".into(),
            decimals: 18, block_time_secs: ZBX_BLOCK_TIME, gas_limit: ZBX_GAS_LIMIT,
            initial_base_fee: ZBX_BASE_FEE_GWEI, max_validators: ZBX_MAX_VALIDATORS,
            epoch_length: ZBX_EPOCH_LENGTH, halving_interval: 25_000_000,
            initial_block_reward: 3_000_000_000_000_000_000,
        }
    }
}

impl ChainConfig {
    pub fn mainnet()  -> Self { Self::default() }

    /// Public testnet AND devnet share `chain_id = 8990` (locked-in 2026-05-01).
    /// Operational isolation between devnet and public testnet is via
    /// bootstrap peers + validator key set, NOT chain-ID separation.
    /// `Self::devnet()` is intentionally absent — use `testnet()` for both.
    pub fn testnet()  -> Self { Self { chain_id: CHAIN_ID_TESTNET, name: "Zebvix Testnet".into(), ..Self::default() } }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mainnet_chain_id_is_8989() {
        let cfg = ChainConfig::mainnet();
        assert_eq!(cfg.chain_id, CHAIN_ID_MAINNET);
        assert_eq!(cfg.symbol, "ZBX");
        assert_eq!(cfg.decimals, 18);
    }

    #[test]
    fn testnet_chain_id_is_8990() {
        let cfg = ChainConfig::testnet();
        assert_eq!(cfg.chain_id, CHAIN_ID_TESTNET);
        assert!(cfg.name.contains("Testnet"));
    }

    #[test]
    fn default_config_matches_constants() {
        let cfg = ChainConfig::default();
        assert_eq!(cfg.block_time_secs, ZBX_BLOCK_TIME);
        assert_eq!(cfg.gas_limit, ZBX_GAS_LIMIT);
        assert_eq!(cfg.initial_base_fee, ZBX_BASE_FEE_GWEI);
        assert_eq!(cfg.max_validators, ZBX_MAX_VALIDATORS);
        assert_eq!(cfg.epoch_length, ZBX_EPOCH_LENGTH);
    }

    #[test]
    fn halving_interval_nonzero() {
        let cfg = ChainConfig::default();
        assert!(cfg.halving_interval > 0);
        assert!(cfg.initial_block_reward > 0);
    }

    #[test]
    fn mainnet_and_testnet_differ_only_in_chain_id_and_name() {
        let mainnet = ChainConfig::mainnet();
        let testnet = ChainConfig::testnet();
        assert_ne!(mainnet.chain_id, testnet.chain_id);
        assert_ne!(mainnet.name, testnet.name);
        assert_eq!(mainnet.decimals, testnet.decimals);
        assert_eq!(mainnet.gas_limit, testnet.gas_limit);
    }
}
