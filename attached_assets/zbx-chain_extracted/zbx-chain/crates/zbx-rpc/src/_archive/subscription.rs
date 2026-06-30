//! WebSocket RPC subscription management.
//!
//! ZBX supports Ethereum-compatible WebSocket subscriptions:
//!   eth_subscribe(topic, params) -> subscription_id
//!   eth_unsubscribe(subscription_id) -> bool
//!
//! Subscription topics:
//!   "newHeads"               -- new block headers as they are mined
//!   "newPendingTransactions" -- new tx hashes entering mempool
//!   "logs"                   -- event logs matching a filter
//!   "syncing"                -- sync status changes
//!
//! Each subscription gets a unique SubscriptionId (random 16-byte hex).
//! Multiple subscriptions per WebSocket connection are supported.
//! When connection drops, all subscriptions are automatically cleaned up.

use std::collections::HashMap;

// ── Subscription ID ───────────────────────────────────────────────────────────

/// Unique identifier for a WebSocket subscription.
/// Format: "0x" + 16 random bytes (hex) = "0x" + 32 hex chars.
/// Example: "0x9ce59a13059e417087c02d3236a0b1cc"
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub String);

impl SubscriptionId {
    /// Generate a new random subscription ID.
    pub fn new_random() -> Self {
        let bytes: [u8; 16] = rand_bytes();
        let hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
        Self(format!("0x{}", hex))
    }

    pub fn as_str(&self) -> &str { &self.0 }
}

impl std::fmt::Display for SubscriptionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ── Subscription topic ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum SubscriptionTopic {
    /// New block headers (fires on every new block).
    NewHeads,
    /// New pending transaction hashes entering the mempool.
    NewPendingTransactions,
    /// Event logs matching a filter.
    Logs(LogFilter),
    /// Node sync status changes.
    Syncing,
}

/// Log filter for "logs" subscriptions.
#[derive(Debug, Clone)]
pub struct LogFilter {
    /// Filter by contract address (None = all contracts)
    pub address:  Option<Vec<[u8; 20]>>,
    /// Filter by event topics (None = any topic)
    pub topics:   Vec<Option<Vec<[u8; 32]>>>,
    /// Starting block (None = latest)
    pub from_block: Option<u64>,
}

// ── Subscription state ────────────────────────────────────────────────────────

/// A single active WebSocket subscription.
#[derive(Debug)]
pub struct Subscription {
    pub id:           SubscriptionId,
    pub topic:        SubscriptionTopic,
    pub connection_id: String,  // WebSocket connection this belongs to
    pub created_at:   u64,      // Unix timestamp
    pub event_count:  u64,      // Events sent so far
}

/// WebSocket connection subscription registry.
///
/// Tracks all active subscriptions per connection.
/// When a WS connection closes, call remove_connection() to clean up all its subs.
pub struct SubscriptionRegistry {
    /// subscription_id -> Subscription
    pub by_id:          HashMap<SubscriptionId, Subscription>,
    /// connection_id -> list of subscription IDs
    pub by_connection:  HashMap<String, Vec<SubscriptionId>>,
    /// Max subscriptions per connection (DoS protection)
    pub max_per_conn:   usize,
}

impl SubscriptionRegistry {
    pub fn new() -> Self {
        Self {
            by_id:         HashMap::new(),
            by_connection: HashMap::new(),
            max_per_conn:  10,  // max 10 subscriptions per WebSocket connection
        }
    }

    /// Create a new subscription for a connection. Returns the SubscriptionId.
    pub fn subscribe(
        &mut self,
        connection_id: String,
        topic:         SubscriptionTopic,
        now:           u64,
    ) -> Result<SubscriptionId, SubError> {
        // Check per-connection limit
        let conn_subs = self.by_connection.entry(connection_id.clone()).or_insert_with(Vec::new);
        if conn_subs.len() >= self.max_per_conn {
            return Err(SubError::TooManySubs { limit: self.max_per_conn });
        }
        let id = SubscriptionId::new_random();
        let sub = Subscription {
            id:           id.clone(),
            topic,
            connection_id: connection_id.clone(),
            created_at:   now,
            event_count:  0,
        };
        conn_subs.push(id.clone());
        self.by_id.insert(id.clone(), sub);
        Ok(id)
    }

    /// Remove a single subscription (eth_unsubscribe).
    pub fn unsubscribe(&mut self, id: &SubscriptionId) -> bool {
        if let Some(sub) = self.by_id.remove(id) {
            if let Some(conn_subs) = self.by_connection.get_mut(&sub.connection_id) {
                conn_subs.retain(|s| s != id);
            }
            true
        } else {
            false
        }
    }

    /// Remove all subscriptions for a connection (on WS disconnect).
    pub fn remove_connection(&mut self, connection_id: &str) {
        if let Some(ids) = self.by_connection.remove(connection_id) {
            for id in ids { self.by_id.remove(&id); }
        }
    }

    /// Get all subscriptions for a given topic (for event dispatch).
    pub fn subscriptions_for_topic(&self, topic: &str) -> Vec<&Subscription> {
        self.by_id.values()
            .filter(|s| match (&s.topic, topic) {
                (SubscriptionTopic::NewHeads, "newHeads") => true,
                (SubscriptionTopic::NewPendingTransactions, "newPendingTransactions") => true,
                (SubscriptionTopic::Logs(_), "logs") => true,
                (SubscriptionTopic::Syncing, "syncing") => true,
                _ => false,
            })
            .collect()
    }

    pub fn total_subscriptions(&self) -> usize { self.by_id.len() }
}

#[derive(Debug)]
pub enum SubError { TooManySubs { limit: usize }, InvalidTopic, ConnectionNotFound }

fn rand_bytes() -> [u8; 16] { [0u8; 16] } // stub: real impl uses OS CSPRNG