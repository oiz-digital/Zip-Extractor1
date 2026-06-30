//! GraphQL schema assembly.

use async_graphql::{EmptyMutation, Schema};
use crate::query::QueryRoot;
use crate::subscription::SubscriptionRoot;

/// The Zebvix GraphQL schema type.
pub type ZbxSchema = Schema<QueryRoot, EmptyMutation, SubscriptionRoot>;

/// Build the Zebvix GraphQL schema with introspection enabled.
pub fn build_schema() -> ZbxSchema {
    Schema::build(QueryRoot, EmptyMutation, SubscriptionRoot)
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_graphql::Request;

    #[tokio::test]
    async fn introspection_returns_schema() {
        let schema = build_schema();
        let resp = schema.execute(Request::new("{ __schema { queryType { name } } }")).await;
        assert!(resp.errors.is_empty(), "introspection failed: {:?}", resp.errors);
    }

    #[tokio::test]
    async fn chain_info_query() {
        let schema = build_schema();
        let resp = schema.execute(Request::new(
            "{ chainInfo { chainId chainName nativeToken } }"
        )).await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
    }

    #[tokio::test]
    async fn invalid_tx_hash_returns_error() {
        let schema = build_schema();
        let resp = schema.execute(Request::new(
            r#"{ transaction(hash: "not-a-valid-hash") { hash } }"#
        )).await;
        assert!(!resp.errors.is_empty(), "expected error for invalid hash");
    }

    #[tokio::test]
    async fn block_without_args_returns_error() {
        let schema = build_schema();
        let resp = schema.execute(Request::new("{ block { number } }")).await;
        assert!(!resp.errors.is_empty(), "expected error when no args provided");
    }
}
