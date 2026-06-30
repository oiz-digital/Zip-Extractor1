//! WebSocket endpoint — streams new blocks and pending transactions to subscribers.

/// Subscription topic a client may request.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    NewBlocks,
    PendingTxs,
}

/// An outbound push message.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum WsMessage {
    NewBlock  { number: u64, hash: String },
    PendingTx { hash: String },
}
