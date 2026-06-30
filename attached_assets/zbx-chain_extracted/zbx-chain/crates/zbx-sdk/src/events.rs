//! WebSocket event subscriptions: newHeads, pendingTransactions, logs.

#[cfg(feature = "ws")]
use tokio_tungstenite::{connect_async, tungstenite::Message};
use crate::error::SdkError;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use futures::StreamExt;

/// Subscribe to a JSON-RPC push stream over WebSocket.
///
/// ```rust,no_run
/// use zbx_sdk::events::subscribe;
/// use zbx_sdk::filter::LogFilter;
///
/// let (mut rx, handle) = subscribe("wss://ws.zebvix.com", "newHeads", vec![]).await?;
/// while let Some(event) = rx.recv().await {
///     println!("New block: {:?}", event);
/// }
/// ```
#[cfg(feature = "ws")]
pub async fn subscribe(
    ws_url:       &str,
    subscription: &str,
    params:       Vec<Value>,
) -> Result<(mpsc::Receiver<Value>, SubscriptionHandle), SdkError> {
    let (ws_stream, _) = connect_async(ws_url).await
        .map_err(|e| SdkError::WebSocket(e.to_string()))?;
    let (mut write, mut read) = ws_stream.split();

    // Send eth_subscribe request.
    let req = serde_json::to_string(&json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "eth_subscribe",
        "params": [subscription].iter().chain(params.iter()).collect::<Vec<_>>()
    })).unwrap();
    write.send(Message::Text(req)).await
        .map_err(|e| SdkError::WebSocket(e.to_string()))?;

    let (tx, rx) = mpsc::channel::<Value>(256);
    let cancel   = tokio_util::sync::CancellationToken::new();
    let cancel_c = cancel.clone();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel_c.cancelled() => break,
                msg = read.next() => {
                    match msg {
                        Some(Ok(Message::Text(t))) => {
                            if let Ok(v) = serde_json::from_str::<Value>(&t) {
                                if v.get("params").is_some() {
                                    let _ = tx.send(v["params"]["result"].clone()).await;
                                }
                            }
                        }
                        _ => break,
                    }
                }
            }
        }
    });

    Ok((rx, SubscriptionHandle { cancel }))
}

/// Subscription handle — drop or call `.cancel()` to unsubscribe.
pub struct SubscriptionHandle {
    #[cfg(feature = "ws")]
    cancel: tokio_util::sync::CancellationToken,
}

impl SubscriptionHandle {
    pub fn cancel(self) {
        #[cfg(feature = "ws")]
        self.cancel.cancel();
    }
}

/// Subscribe to new block headers.
#[cfg(feature = "ws")]
pub async fn new_heads(ws_url: &str)
    -> Result<(mpsc::Receiver<Value>, SubscriptionHandle), SdkError>
{
    subscribe(ws_url, "newHeads", vec![]).await
}

/// Subscribe to pending transactions.
#[cfg(feature = "ws")]
pub async fn pending_transactions(ws_url: &str)
    -> Result<(mpsc::Receiver<Value>, SubscriptionHandle), SdkError>
{
    subscribe(ws_url, "newPendingTransactions", vec![]).await
}

/// Subscribe to contract event logs.
#[cfg(feature = "ws")]
pub async fn logs(ws_url: &str, filter: &crate::filter::LogFilter)
    -> Result<(mpsc::Receiver<Value>, SubscriptionHandle), SdkError>
{
    subscribe(ws_url, "logs", vec![filter.to_json()]).await
}