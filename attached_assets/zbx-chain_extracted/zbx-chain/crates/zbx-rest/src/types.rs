//! Shared REST API response types (OpenAPI schema annotations via utoipa).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Paginated response wrapper.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct Paginated<T: Serialize> {
    pub items:  Vec<T>,
    pub total:  u64,
    pub page:   u64,
    pub limit:  u64,
}

/// Block summary for list endpoints.
#[derive(Serialize, Deserialize, ToSchema, Clone)]
pub struct BlockSummary {
    pub number:     u64,
    pub hash:       String,
    pub parent_hash: String,
    pub timestamp:  u64,
    pub tx_count:   u32,
    pub gas_used:   u64,
    pub gas_limit:  u64,
    pub base_fee:   String,
    pub proposer:   String,
    pub size_bytes: u64,
}

/// Full block with transaction hashes.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct BlockDetail {
    #[serde(flatten)]
    pub summary:      BlockSummary,
    pub state_root:   String,
    pub tx_root:      String,
    pub receipts_root: String,
    pub epoch:        u64,
    pub transactions: Vec<String>,
}

/// Transaction summary.
#[derive(Serialize, Deserialize, ToSchema, Clone)]
pub struct TxSummary {
    pub hash:        String,
    pub from_addr:   String,
    pub to_addr:     Option<String>,
    pub value:       String,
    pub gas:         u64,
    pub nonce:       u64,
    pub tx_type:     u8,
    pub block_number: Option<u64>,
    pub status:      Option<bool>,
}

/// Account state.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct AccountInfo {
    pub address:      String,
    pub balance:      String,
    pub nonce:        u64,
    pub is_contract:  bool,
    pub code_hash:    String,
    pub storage_root: String,
}

/// Validator info.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct ValidatorInfo {
    pub address:         String,
    pub pub_key_hex:     String,
    pub stake:           String,
    pub delegated_stake: String,
    pub commission_pct:  f64,
    pub status:          String,
    pub uptime_pct:      f64,
    pub blocks_produced: u64,
    pub epoch_joined:    u64,
}

/// Network info.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct NetworkInfo {
    pub chain_id:        u64,
    pub chain_name:      String,
    pub network:         String,
    pub latest_block:    u64,
    pub finalized_block: u64,
    pub peer_count:      u32,
    pub sync_status:     String,
    pub node_version:    String,
}

/// Gas price info.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct GasInfo {
    pub base_fee:       String,
    pub safe_gas_price: String,
    pub fast_gas_price: String,
    pub rapid_gas_price: String,
}

/// Raw transaction broadcast request.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct BroadcastRequest {
    /// RLP-encoded signed transaction (0x-prefixed hex).
    pub raw_tx: String,
}

/// Raw transaction broadcast response.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct BroadcastResponse {
    pub tx_hash: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_summary_serializes() {
        let b = BlockSummary {
            number: 100, hash: "0xhash".into(), parent_hash: "0xparent".into(),
            timestamp: 1_700_000_000, tx_count: 5, gas_used: 105_000,
            gas_limit: 30_000_000, base_fee: "1000000000".into(),
            proposer: "0xval".into(), size_bytes: 4096,
        };
        let json = serde_json::to_string(&b).unwrap();
        assert!(json.contains("100"));
        assert!(json.contains("0xhash"));
    }

    #[test]
    fn paginated_wraps_items() {
        let p: Paginated<u64> = Paginated { items: vec![1, 2, 3], total: 3, page: 0, limit: 10 };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"total\":3"));
        assert!(json.contains("[1,2,3]"));
    }

    #[test]
    fn gas_info_fields_present() {
        let g = GasInfo {
            base_fee: "1000000000".into(),
            safe_gas_price: "1100000000".into(),
            fast_gas_price: "1500000000".into(),
            rapid_gas_price: "2000000000".into(),
        };
        let json = serde_json::to_string(&g).unwrap();
        assert!(json.contains("base_fee"));
        assert!(json.contains("rapid_gas_price"));
    }

    #[test]
    fn broadcast_request_and_response_serialize() {
        let req = BroadcastRequest { raw_tx: "0xdeadbeef".into() };
        let res = BroadcastResponse { tx_hash: "0xtxhash".into() };
        let req_json = serde_json::to_string(&req).unwrap();
        let res_json = serde_json::to_string(&res).unwrap();
        assert!(req_json.contains("0xdeadbeef"));
        assert!(res_json.contains("0xtxhash"));
    }

    #[test]
    fn tx_summary_optional_fields() {
        let tx = TxSummary {
            hash: "0xtx".into(), from_addr: "0xfrom".into(), to_addr: None,
            value: "0".into(), gas: 21_000, nonce: 0, tx_type: 2,
            block_number: None, status: None,
        };
        let json = serde_json::to_string(&tx).unwrap();
        assert!(json.contains("null"));
        assert!(json.contains("21000"));
    }
}
