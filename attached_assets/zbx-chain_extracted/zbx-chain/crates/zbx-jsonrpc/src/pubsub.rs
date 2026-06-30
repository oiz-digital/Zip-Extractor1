//! Pub/sub subscription manager for eth_subscribe / eth_unsubscribe.

use crate::response::JsonRpcNotification;
use dashmap::DashMap;
use tokio::sync::mpsc;
use tracing::{debug, warn};
use uuid::Uuid;

/// Subscription types.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SubscriptionKind {
    /// New blocks.
    NewHeads,
    /// New pending transactions.
    NewPendingTransactions,
    /// Log filter subscriptions.
    Logs,
    /// Sync state changes.
    Syncing,
}

impl SubscriptionKind {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "newHeads"             => Some(Self::NewHeads),
            "newPendingTransactions" => Some(Self::NewPendingTransactions),
            "logs"                 => Some(Self::Logs),
            "syncing"              => Some(Self::Syncing),
            _                      => None,
        }
    }
}

/// A single subscription entry.
pub struct Subscription {
    pub id:   String,
    pub kind: SubscriptionKind,
    pub tx:   mpsc::UnboundedSender<JsonRpcNotification>,
}

/// Manages all active WebSocket subscriptions.
pub struct PubSubManager {
    subs: DashMap<String, Subscription>,
}

impl PubSubManager {
    pub fn new() -> Self {
        Self { subs: DashMap::new() }
    }

    /// Create a new subscription, returning its ID and a receiver for notifications.
    pub fn subscribe(
        &self,
        kind: SubscriptionKind,
    ) -> (String, mpsc::UnboundedReceiver<JsonRpcNotification>) {
        let id = format!("0x{}", Uuid::new_v4().as_simple());
        let (tx, rx) = mpsc::unbounded_channel();
        self.subs.insert(id.clone(), Subscription { id: id.clone(), kind, tx });
        debug!("pubsub: new subscription {}", id);
        (id, rx)
    }

    /// Remove a subscription.
    pub fn unsubscribe(&self, id: &str) -> bool {
        self.subs.remove(id).is_some()
    }

    /// Broadcast a notification to all subscriptions of the given kind.
    pub fn broadcast(&self, kind: &SubscriptionKind, result: serde_json::Value) {
        let method = match kind {
            SubscriptionKind::NewHeads                => "eth_subscription",
            SubscriptionKind::NewPendingTransactions  => "eth_subscription",
            SubscriptionKind::Logs                    => "eth_subscription",
            SubscriptionKind::Syncing                 => "eth_subscription",
        };
        self.subs.retain(|_, sub| {
            if &sub.kind != kind { return true; }
            let notif = JsonRpcNotification::new(method, &sub.id, result.clone());
            if sub.tx.send(notif).is_err() {
                warn!("pubsub: dropped subscription {}", sub.id);
                return false;
            }
            true
        });
    }

    pub fn subscription_count(&self) -> usize {
        self.subs.len()
    }
}

impl Default for PubSubManager {
    fn default() -> Self { Self::new() }
}