//! Block REST endpoints.

use axum::{
    extract::{Path, Query},
    Json,
};
use serde::Deserialize;
use utoipa::IntoParams;
use crate::{error::RestError, types::{BlockDetail, BlockSummary, Paginated, TxSummary}};

#[derive(Deserialize, IntoParams)]
pub struct PaginationParams {
    #[serde(default = "default_page")]
    pub page:  u64,
    #[serde(default = "default_limit")]
    pub limit: u64,
}
fn default_page()  -> u64 { 1 }
fn default_limit() -> u64 { 20 }

/// GET /blocks/latest
#[utoipa::path(
    get,
    path = "/api/v1/blocks/latest",
    tag = "Blocks",
    responses(
        (status = 200, description = "Latest block", body = BlockDetail),
        (status = 404, description = "No blocks yet"),
    )
)]
pub async fn get_latest_block() -> Result<Json<BlockDetail>, RestError> {
    // Production: read from StateDB / RpcState.
    Err(RestError::NotFound("no blocks yet".to_string()))
}

/// GET /blocks/:number
#[utoipa::path(
    get,
    path = "/api/v1/blocks/{number}",
    tag = "Blocks",
    params(("number" = u64, Path, description = "Block number")),
    responses(
        (status = 200, description = "Block detail", body = BlockDetail),
        (status = 404, description = "Block not found"),
        (status = 400, description = "Invalid block number"),
    )
)]
pub async fn get_block_by_number(
    Path(number): Path<u64>,
) -> Result<Json<BlockDetail>, RestError> {
    let _ = number;
    Err(RestError::NotFound(format!("block {} not found", number)))
}

/// GET /blocks/:number/transactions
#[utoipa::path(
    get,
    path = "/api/v1/blocks/{number}/transactions",
    tag = "Blocks",
    params(
        ("number" = u64, Path, description = "Block number"),
        PaginationParams,
    ),
    responses(
        (status = 200, description = "Transaction list", body = Paginated<TxSummary>),
        (status = 404, description = "Block not found"),
    )
)]
pub async fn get_block_transactions(
    Path(number): Path<u64>,
    Query(pagination): Query<PaginationParams>,
) -> Result<Json<Paginated<TxSummary>>, RestError> {
    let _ = (number, pagination);
    Err(RestError::NotFound(format!("block {} not found", number)))
}
