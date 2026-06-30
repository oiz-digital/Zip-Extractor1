//! GraphQL Subscriptions — real-time chain events over WebSocket.

use async_graphql::{Context, Subscription};
use futures_util::stream::{Stream, empty};
use crate::types::GqlBlockHeader;

pub struct SubscriptionRoot;

#[Subscription]
impl SubscriptionRoot {
    /// Subscribe to new block headers as they are finalized.
    async fn new_blocks(&self, _ctx: &Context<'_>) -> impl Stream<Item = GqlBlockHeader> {
        // Production: bridge to tokio broadcast channel from consensus finalizer.
        empty::<GqlBlockHeader>()
    }

    /// Subscribe to new pending transaction hashes.
    async fn pending_transactions(&self, _ctx: &Context<'_>) -> impl Stream<Item = String> {
        // Production: bridge to mempool notification channel.
        empty::<String>()
    }
}
