//! GraphQL output types — mirrors RPC types but with GraphQL derives.

use async_graphql::{SimpleObject, Enum};
use serde::{Deserialize, Serialize};

/// Chain-level metadata.
#[derive(Debug, Clone, SimpleObject, Serialize, Deserialize)]
pub struct GqlChainInfo {
    pub chain_id:       u64,
    pub chain_name:     String,
    pub network:        String,
    pub latest_block:   u64,
    pub finalized_block: u64,
    pub native_token:   String,
    pub rpc_version:    String,
}

/// Block header representation.
#[derive(Debug, Clone, SimpleObject, Serialize, Deserialize)]
pub struct GqlBlockHeader {
    pub number:       u64,
    pub hash:         String,
    pub parent_hash:  String,
    pub state_root:   String,
    pub tx_root:      String,
    pub receipts_root: String,
    pub timestamp:    u64,
    pub gas_limit:    u64,
    pub gas_used:     u64,
    pub base_fee:     String,
    pub proposer:     String,
    pub tx_count:     u32,
    pub epoch:        u64,
}

/// Transaction summary.
#[derive(Debug, Clone, SimpleObject, Serialize, Deserialize)]
pub struct GqlTransaction {
    pub hash:              String,
    pub block_number:      Option<u64>,
    pub block_hash:        Option<String>,
    pub from_addr:         String,
    pub to_addr:           Option<String>,
    pub value:             String,
    pub gas:               u64,
    pub gas_price:         String,
    pub max_fee_per_gas:   Option<String>,
    pub max_priority_fee:  Option<String>,
    pub nonce:             u64,
    pub input:             String,
    pub tx_type:           u8,
    pub status:            Option<bool>,
    pub gas_used:          Option<u64>,
}

/// Account state.
#[derive(Debug, Clone, SimpleObject, Serialize, Deserialize)]
pub struct GqlAccount {
    pub address:        String,
    pub balance:        String,
    pub nonce:          u64,
    pub code_hash:      String,
    pub is_contract:    bool,
    pub storage_root:   String,
}

/// Validator info.
#[derive(Debug, Clone, SimpleObject, Serialize, Deserialize)]
pub struct GqlValidator {
    pub address:         String,
    pub pub_key:         String,
    pub stake:           String,
    pub delegated_stake: String,
    pub commission:      f64,
    pub status:          GqlValidatorStatus,
    pub uptime_pct:      f64,
    pub blocks_produced: u64,
    pub epoch_joined:    u64,
}

/// Validator status enum.
#[derive(Debug, Clone, Copy, Enum, PartialEq, Eq, Serialize, Deserialize)]
pub enum GqlValidatorStatus {
    Active,
    Jailed,
    Unbonding,
    Inactive,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gql_chain_info_serializes() {
        let info = GqlChainInfo {
            chain_id:       8990,
            chain_name:     "Zebvix Testnet".into(),
            network:        "testnet".into(),
            latest_block:   42,
            finalized_block: 40,
            native_token:   "ZBX".into(),
            rpc_version:    "1.0.0".into(),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("8990"));
        assert!(json.contains("Zebvix Testnet"));
    }

    #[test]
    fn gql_block_header_serializes() {
        let hdr = GqlBlockHeader {
            number: 1, hash: "0xabc".into(), parent_hash: "0x000".into(),
            state_root: "0xsr".into(), tx_root: "0xtr".into(),
            receipts_root: "0xrr".into(), timestamp: 1_000_000,
            gas_limit: 30_000_000, gas_used: 21_000,
            base_fee: "1000000000".into(), proposer: "0xprop".into(),
            tx_count: 1, epoch: 0,
        };
        let json = serde_json::to_string(&hdr).unwrap();
        assert!(json.contains("30000000"));
        assert!(json.contains("0xprop"));
    }

    #[test]
    fn gql_validator_status_roundtrip() {
        let statuses = [
            GqlValidatorStatus::Active,
            GqlValidatorStatus::Jailed,
            GqlValidatorStatus::Unbonding,
            GqlValidatorStatus::Inactive,
        ];
        for s in &statuses {
            let json = serde_json::to_string(s).unwrap();
            let back: GqlValidatorStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(*s, back);
        }
    }

    #[test]
    fn gql_transaction_optional_fields_none() {
        let tx = GqlTransaction {
            hash: "0xtx".into(), block_number: None, block_hash: None,
            from_addr: "0xfrom".into(), to_addr: None, value: "0".into(),
            gas: 21_000, gas_price: "1".into(), max_fee_per_gas: None,
            max_priority_fee: None, nonce: 0, input: "0x".into(),
            tx_type: 0, status: None, gas_used: None,
        };
        let json = serde_json::to_string(&tx).unwrap();
        assert!(json.contains("null"));
    }
}
