//! Log filter builder for eth_getLogs and event subscriptions.

use zbx_types::{Address, H256};
use serde_json::{json, Value};

/// Fluent log filter builder.
#[derive(Debug, Clone, Default)]
pub struct FilterBuilder {
    from_block: Option<BlockId>,
    to_block:   Option<BlockId>,
    addresses:  Vec<Address>,
    topics:     Vec<Option<Vec<H256>>>,
    limit:      Option<u64>,
}

#[derive(Debug, Clone)]
pub enum BlockId {
    Number(u64),
    Latest,
    Finalized,
    Pending,
}

impl BlockId {
    fn to_str(&self) -> String {
        match self {
            BlockId::Number(n)  => format!("0x{:x}", n),
            BlockId::Latest     => "latest".into(),
            BlockId::Finalized  => "finalized".into(),
            BlockId::Pending    => "pending".into(),
        }
    }
}

impl FilterBuilder {
    pub fn new() -> Self { Self::default() }

    pub fn from_block(mut self, b: u64) -> Self {
        self.from_block = Some(BlockId::Number(b)); self
    }
    pub fn to_block(mut self, b: u64) -> Self {
        self.to_block = Some(BlockId::Number(b)); self
    }
    pub fn latest(mut self) -> Self {
        self.from_block = Some(BlockId::Latest);
        self.to_block   = Some(BlockId::Latest);
        self
    }
    pub fn finalized(mut self) -> Self {
        self.to_block = Some(BlockId::Finalized); self
    }

    pub fn address(mut self, addr: Address) -> Self {
        self.addresses.push(addr); self
    }
    pub fn addresses(mut self, addrs: Vec<Address>) -> Self {
        self.addresses.extend(addrs); self
    }

    /// Filter by an event signature topic (keccak256 hash).
    pub fn event_signature(mut self, sig: H256) -> Self {
        self.topics.insert(0, Some(vec![sig])); self
    }

    /// Filter by topic at index `i`.
    pub fn topic(mut self, idx: usize, values: Vec<H256>) -> Self {
        while self.topics.len() <= idx { self.topics.push(None); }
        self.topics[idx] = Some(values);
        self
    }

    pub fn limit(mut self, n: u64) -> Self { self.limit = Some(n); self }

    pub fn build(self) -> LogFilter { LogFilter(self) }
}

/// A compiled log filter.
pub struct LogFilter(FilterBuilder);

impl LogFilter {
    pub fn to_json(&self) -> Value {
        let f = &self.0;
        let mut obj = serde_json::Map::new();
        if let Some(ref b) = f.from_block {
            obj.insert("fromBlock".into(), json!(b.to_str()));
        }
        if let Some(ref b) = f.to_block {
            obj.insert("toBlock".into(), json!(b.to_str()));
        }
        if !f.addresses.is_empty() {
            let addrs: Vec<String> = f.addresses.iter()
                .map(|a| format!("0x{}", hex::encode(a.as_bytes())))
                .collect();
            obj.insert("address".into(), json!(addrs));
        }
        if !f.topics.is_empty() {
            let topics: Vec<Value> = f.topics.iter().map(|slot| match slot {
                None => Value::Null,
                Some(hashes) => json!(hashes.iter()
                    .map(|h| format!("0x{}", hex::encode(h.as_bytes())))
                    .collect::<Vec<_>>()),
            }).collect();
            obj.insert("topics".into(), json!(topics));
        }
        Value::Object(obj)
    }
}

impl std::ops::Deref for FilterBuilder {
    type Target = FilterBuilder;
    fn deref(&self) -> &Self { self }
}