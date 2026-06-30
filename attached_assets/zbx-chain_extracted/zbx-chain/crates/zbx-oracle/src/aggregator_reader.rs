//! Task #5 (Precompile 0x0C — Price oracle read): on-chain registry layout
//! re-export.
//!
//! The `0x0C` precompile reads from a well-known pseudo-system contract
//! that aggregates every Chainlink-style `ZbxAggregatorV3` feed under a
//! single `mapping(bytes32 => Feed)` slot layout. The actual reader and
//! gas schedule live in `zbx_crypto::oracle_state` so EVM and ZVM stay
//! byte-identical.
//!
//! This module exists in `zbx-oracle` so producer-side tooling
//! (genesis seeders, CLI publishers, RPC observers) can `use
//! zbx_oracle::aggregator_reader::*` without depending on `zbx-crypto`
//! directly. It is a thin re-export — adding logic here would risk
//! drifting from the consensus body in `zbx-crypto`.

pub use zbx_crypto::oracle_state::{
    do_price_oracle, encode_price_e8, encode_timestamp, slot_pair, write_feed, OraclePrecompileError,
    OracleStateReader, BASE_GAS, FEED_MAP_SLOT, ORACLE_REGISTRY_ADDRESS, PER_SLOT_GAS, TOTAL_GAS,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn re_exports_are_consensus_critical_constants() {
        // Lock the public surface — if any of these change, every node
        // that hasn't upgraded will fork at the first 0x0C call.
        assert_eq!(TOTAL_GAS, BASE_GAS + PER_SLOT_GAS * 2);
        assert_eq!(TOTAL_GAS, 1200);
        assert_eq!(ORACLE_REGISTRY_ADDRESS[19], 0xCC);
        assert_eq!(FEED_MAP_SLOT, [0u8; 32]);
    }
}
