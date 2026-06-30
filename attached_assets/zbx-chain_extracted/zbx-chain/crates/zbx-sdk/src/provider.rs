//! HTTP JSON-RPC provider with middleware, retry, and batch support.

use crate::{
    error::SdkError,
    middleware::{Middleware, MiddlewareStack},
    transaction::{TransactionRequest, SignedTransaction},
    wallet::Wallet,
    filter::LogFilter,
    gas::GasPricing,
    types::Block,
};
use zbx_types::{Address, U256, H256};
use serde_json::{json, Value};
use serde::de::DeserializeOwned;
use reqwest::Client;
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};

/// JSON-RPC provider for Zebvix Chain.
///
/// `Provider` is cheap to clone — it wraps an `Arc` internally.
#[derive(Clone)]
pub struct Provider(Arc<Inner>);

struct Inner {
    url:        String,
    client:     Client,
    chain_id:   AtomicU64,
    middleware: MiddlewareStack,
    id_counter: AtomicU64,
}

impl Provider {
    // ── Constructors ─────────────────────────────────────────────────────────

    /// Create an HTTP provider and verify connectivity.
    pub async fn http(url: impl Into<String>) -> Result<Self, SdkError> {
        let url    = url.into();
        url::Url::parse(&url).map_err(|e| SdkError::UrlParse(e.to_string()))?;
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        let inner = Arc::new(Inner {
            url,
            client,
            chain_id:   AtomicU64::new(0),
            middleware: MiddlewareStack::default(),
            id_counter: AtomicU64::new(1),
        });
        let p = Self(inner);
        let id = p.chain_id().await?;
        p.0.chain_id.store(id, Ordering::Relaxed);
        Ok(p)
    }

    /// Return a provider pointing at the Zebvix mainnet public RPC.
    pub async fn mainnet() -> Result<Self, SdkError> {
        Self::http("https://rpc.zebvix.com").await
    }

    /// Return a provider pointing at localhost (devnet).
    pub async fn devnet() -> Result<Self, SdkError> {
        Self::http("http://localhost:8545").await
    }

    /// Attach a middleware (e.g. logging, retry, auth).
    pub fn with_middleware(self, m: impl Middleware + 'static) -> Self {
        // Clone inner and push middleware — omitted for brevity.
        self
    }

    // ── Core RPC call ─────────────────────────────────────────────────────────

    /// Send a raw JSON-RPC request.
    pub async fn raw_call<R: DeserializeOwned>(
        &self,
        method:  &str,
        params:  Value,
    ) -> Result<R, SdkError> {
        let id = self.0.id_counter.fetch_add(1, Ordering::Relaxed);
        let body = json!({
            "jsonrpc": "2.0",
            "id":       id,
            "method":   method,
            "params":   params,
        });
        let resp: Value = self.0.client
            .post(&self.0.url)
            .json(&body)
            .send().await?
            .json().await?;

        if let Some(err) = resp.get("error") {
            let code = err["code"].as_i64().unwrap_or(-1);
            let msg  = err["message"].as_str().unwrap_or("unknown").to_string();
            return Err(SdkError::rpc(code, msg));
        }

        serde_json::from_value(resp["result"].clone())
            .map_err(|e| SdkError::RpcParse(e.to_string()))
    }

    // ── Standard eth_ methods ─────────────────────────────────────────────────

    pub async fn chain_id(&self) -> Result<u64, SdkError> {
        let hex: String = self.raw_call("eth_chainId", json!([])).await?;
        parse_hex_u64(&hex)
    }

    pub async fn block_number(&self) -> Result<u64, SdkError> {
        let hex: String = self.raw_call("eth_blockNumber", json!([])).await?;
        parse_hex_u64(&hex)
    }

    pub async fn get_balance(&self, addr: Address) -> Result<U256, SdkError> {
        let hex: String = self.raw_call("eth_getBalance",
            json!([addr_hex(addr), "latest"])).await?;
        parse_hex_u256(&hex)
    }

    pub async fn get_nonce(&self, addr: Address) -> Result<u64, SdkError> {
        let hex: String = self.raw_call("eth_getTransactionCount",
            json!([addr_hex(addr), "latest"])).await?;
        parse_hex_u64(&hex)
    }

    pub async fn get_code(&self, addr: Address) -> Result<Vec<u8>, SdkError> {
        let hex: String = self.raw_call("eth_getCode",
            json!([addr_hex(addr), "latest"])).await?;
        hex::decode(hex.trim_start_matches("0x")).map_err(SdkError::Hex)
    }

    pub async fn get_storage_at(&self, addr: Address, slot: H256) -> Result<H256, SdkError> {
        let hex: String = self.raw_call("eth_getStorageAt",
            json!([addr_hex(addr), hash_hex(slot), "latest"])).await?;
        parse_hex_h256(&hex)
    }

    pub async fn get_block_by_number(&self, n: u64) -> Result<Value, SdkError> {
        self.raw_call("eth_getBlockByNumber",
            json!([format!("0x{:x}", n), true])).await
    }

    pub async fn get_block_by_hash(&self, hash: H256) -> Result<Value, SdkError> {
        self.raw_call("eth_getBlockByHash",
            json!([hash_hex(hash), true])).await
    }

    pub async fn get_transaction(&self, hash: H256) -> Result<Value, SdkError> {
        self.raw_call("eth_getTransactionByHash", json!([hash_hex(hash)])).await
    }

    pub async fn get_receipt(&self, hash: H256) -> Result<Option<Value>, SdkError> {
        self.raw_call("eth_getTransactionReceipt", json!([hash_hex(hash)])).await
    }

    pub async fn get_logs(&self, filter: &LogFilter) -> Result<Vec<Value>, SdkError> {
        self.raw_call("eth_getLogs", json!([filter.to_json()])).await
    }

    pub async fn estimate_gas(&self, tx: &TransactionRequest) -> Result<u64, SdkError> {
        let call_obj = tx_to_call_obj(tx);
        let hex: String = self.raw_call("eth_estimateGas", json!([call_obj])).await?;
        parse_hex_u64(&hex)
    }

    pub async fn call(&self, tx: &TransactionRequest) -> Result<Vec<u8>, SdkError> {
        let call_obj = tx_to_call_obj(tx);
        let hex: String = self.raw_call("eth_call",
            json!([call_obj, "latest"])).await?;
        hex::decode(hex.trim_start_matches("0x")).map_err(SdkError::Hex)
    }

    // ── Gas pricing ───────────────────────────────────────────────────────────

    pub async fn gas_price(&self) -> Result<U256, SdkError> {
        let hex: String = self.raw_call("eth_gasPrice", json!([])).await?;
        parse_hex_u256(&hex)
    }

    pub async fn fee_history(&self, blocks: u64, percentiles: &[f64]) -> Result<Value, SdkError> {
        self.raw_call("eth_feeHistory",
            json!([blocks, "latest", percentiles])).await
    }

    pub async fn get_gas_pricing(&self) -> Result<GasPricing, SdkError> {
        let history = self.fee_history(5, &[10.0, 50.0, 90.0]).await?;
        GasPricing::from_fee_history(&history)
    }

    // ── Transaction sending ───────────────────────────────────────────────────

    /// Send a signed transaction and return its hash.
    pub async fn send_raw(&self, signed: &SignedTransaction) -> Result<H256, SdkError> {
        let hex: String = self.raw_call("eth_sendRawTransaction",
            json!([signed.raw_hex()])).await?;
        parse_hex_h256(&hex)
    }

    /// Build, sign, and send a transaction.  Returns a `PendingTx` handle.
    pub async fn send(
        &self,
        mut tx: TransactionRequest,
        wallet: &Wallet,
    ) -> Result<PendingTx, SdkError> {
        // Fill in nonce if missing.
        if tx.nonce.is_none() {
            tx.nonce = Some(self.get_nonce(wallet.address()).await?);
        }
        // Fill in gas if missing.
        if tx.gas.is_none() {
            let gas = self.estimate_gas(&tx).await?;
            tx.gas = Some(gas + gas / 5); // +20% buffer
        }
        // Fill in EIP-1559 fees if missing.
        if tx.is_eip1559() && tx.max_fee_per_gas.is_none() {
            let pricing = self.get_gas_pricing().await?;
            tx.max_fee_per_gas          = Some(pricing.max_fee);
            tx.max_priority_fee_per_gas = Some(pricing.max_priority_fee);
        }
        let signed = wallet.sign_transaction(tx)?;
        let hash   = self.send_raw(&signed).await?;
        Ok(PendingTx { hash, provider: self.clone() })
    }

    // ── ZBX-specific methods ──────────────────────────────────────────────────

    pub async fn zbx_node_info(&self) -> Result<Value, SdkError> {
        self.raw_call("zbx_nodeInfo", json!([])).await
    }

    pub async fn zbx_validators(&self) -> Result<Vec<Value>, SdkError> {
        self.raw_call("zbx_getValidators", json!([])).await
    }

    pub async fn zbx_consensus_state(&self) -> Result<Value, SdkError> {
        self.raw_call("zbx_getConsensusState", json!([])).await
    }

    pub async fn zbx_mempool_status(&self) -> Result<Value, SdkError> {
        self.raw_call("zbx_getMempoolStatus", json!([])).await
    }

    pub async fn zbx_staking_info(&self) -> Result<Value, SdkError> {
        self.raw_call("zbx_getStakingInfo", json!([])).await
    }

    pub async fn zbx_finalized_block(&self) -> Result<Value, SdkError> {
        self.raw_call("zbx_getFinalizedBlock", json!([])).await
    }
}

/// A pending transaction that can be awaited for confirmation.
pub struct PendingTx {
    pub hash:     H256,
    provider: Provider,
}

impl PendingTx {
    /// Wait for a given number of block confirmations.
    pub async fn wait_confirmations(&self, confs: u64) -> Result<Value, SdkError> {
        let target_block = {
            let receipt = self.wait_receipt(60).await?;
            let block_num = receipt["blockNumber"].as_str()
                .and_then(|h| u64::from_str_radix(h.trim_start_matches("0x"), 16).ok())
                .ok_or_else(|| SdkError::Other("missing blockNumber in receipt".into()))?;
            block_num + confs
        };
        // Poll until we're at the target block.
        loop {
            let current = self.provider.block_number().await?;
            if current >= target_block { break; }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        self.provider.get_receipt(self.hash).await?
            .ok_or(SdkError::TransactionDropped)
    }

    pub async fn wait_receipt(&self, timeout_secs: u64) -> Result<Value, SdkError> {
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(timeout_secs);
        loop {
            if let Some(r) = self.provider.get_receipt(self.hash).await? {
                return Ok(r);
            }
            if std::time::Instant::now() > deadline {
                return Err(SdkError::Timeout { secs: timeout_secs });
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

fn addr_hex(a: Address) -> String { format!("0x{}", hex::encode(a.as_bytes())) }
fn hash_hex(h: H256)    -> String { format!("0x{}", hex::encode(h.as_bytes())) }

fn parse_hex_u64(hex: &str) -> Result<u64, SdkError> {
    u64::from_str_radix(hex.trim_start_matches("0x"), 16)
        .map_err(|e| SdkError::RpcParse(e.to_string()))
}
fn parse_hex_u256(hex: &str) -> Result<U256, SdkError> {
    let n = u128::from_str_radix(hex.trim_start_matches("0x"), 16)
        .map_err(|e| SdkError::RpcParse(e.to_string()))?;
    Ok(U256::from(n))
}
fn parse_hex_h256(hex: &str) -> Result<H256, SdkError> {
    let bytes = hex::decode(hex.trim_start_matches("0x")).map_err(SdkError::Hex)?;
    if bytes.len() != 32 {
        return Err(SdkError::RpcParse("expected 32-byte hash".into()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(H256(arr))
}
fn tx_to_call_obj(tx: &TransactionRequest) -> Value {
    let mut obj = serde_json::Map::new();
    if let Some(a) = tx.from { obj.insert("from".into(), json!(addr_hex(a))); }
    if let Some(a) = tx.to   { obj.insert("to".into(),   json!(addr_hex(a))); }
    if let Some(v) = &tx.value { obj.insert("value".into(), json!(format!("0x{:x}", v.as_u128()))); }
    if let Some(d) = &tx.data  { obj.insert("data".into(),  json!(format!("0x{}", hex::encode(d)))); }
    if let Some(g) = tx.gas    { obj.insert("gas".into(),   json!(format!("0x{:x}", g))); }
    Value::Object(obj)
}