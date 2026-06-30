//! GraphQL Query root — read-only chain data queries.

use async_graphql::{Context, Object, Result as GqlResult};
use crate::types::{GqlBlockHeader, GqlTransaction, GqlAccount, GqlValidator, GqlChainInfo};
use crate::error::GraphqlError;

pub struct QueryRoot;

#[Object]
impl QueryRoot {
    /// Fetch the current chain info.
    async fn chain_info(&self, _ctx: &Context<'_>) -> GqlResult<GqlChainInfo> {
        Ok(GqlChainInfo {
            chain_id:        8990,
            chain_name:      "Zebvix Testnet".to_string(),
            network:         "testnet".to_string(),
            latest_block:    0,
            finalized_block: 0,
            native_token:    "ZBX".to_string(),
            rpc_version:     "1.0.0".to_string(),
        })
    }

    /// Fetch a block by number or hash. If both provided, number takes precedence.
    async fn block(
        &self,
        _ctx: &Context<'_>,
        number: Option<u64>,
        hash:   Option<String>,
    ) -> GqlResult<Option<GqlBlockHeader>> {
        // Validate inputs.
        if number.is_none() && hash.is_none() {
            return Err(async_graphql::Error::new(
                "provide at least one of 'number' or 'hash'"
            ));
        }
        if let Some(ref h) = hash {
            if !h.starts_with("0x") || h.len() != 66 {
                return Err(async_graphql::Error::new(
                    GraphqlError::InvalidHash(h.clone(), "must be 0x + 32 hex bytes".into())
                        .to_string()
                ));
            }
        }
        // In production: delegate to RpcState / StateDB.
        // Returns None to signal "not found" without error.
        Ok(None)
    }

    /// Fetch a transaction by hash.
    async fn transaction(
        &self,
        _ctx: &Context<'_>,
        hash: String,
    ) -> GqlResult<Option<GqlTransaction>> {
        if !hash.starts_with("0x") || hash.len() != 66 {
            return Err(async_graphql::Error::new(
                GraphqlError::InvalidHash(hash, "must be 0x + 32 hex bytes".into()).to_string()
            ));
        }
        Ok(None)
    }

    /// Fetch account state by address.
    async fn account(
        &self,
        _ctx: &Context<'_>,
        address: String,
    ) -> GqlResult<Option<GqlAccount>> {
        if !address.starts_with("0x") || address.len() != 42 {
            return Err(async_graphql::Error::new(
                GraphqlError::InvalidAddress(address, "must be 0x + 20 hex bytes".into()).to_string()
            ));
        }
        Ok(None)
    }

    /// Fetch a validator by address.
    async fn validator(
        &self,
        _ctx: &Context<'_>,
        address: String,
    ) -> GqlResult<Option<GqlValidator>> {
        if !address.starts_with("0x") || address.len() != 42 {
            return Err(async_graphql::Error::new(
                GraphqlError::InvalidAddress(address, "must be 0x + 20 hex bytes".into()).to_string()
            ));
        }
        Ok(None)
    }

    /// List validators (optionally filtered to active only).
    async fn validators(
        &self,
        _ctx: &Context<'_>,
        active: Option<bool>,
    ) -> GqlResult<Vec<GqlValidator>> {
        let _ = active;
        Ok(vec![])
    }
}
