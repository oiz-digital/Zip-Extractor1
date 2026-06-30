//! Transaction REST endpoints.

use axum::{extract::Path, Json};
use crate::{error::RestError, types::{BroadcastRequest, BroadcastResponse, TxSummary}};

/// GET /transactions/:hash
#[utoipa::path(
    get,
    path = "/api/v1/transactions/{hash}",
    tag = "Transactions",
    params(("hash" = String, Path, description = "Transaction hash (0x-prefixed)")),
    responses(
        (status = 200, description = "Transaction detail", body = TxSummary),
        (status = 400, description = "Invalid hash"),
        (status = 404, description = "Transaction not found"),
    )
)]
pub async fn get_transaction(
    Path(hash): Path<String>,
) -> Result<Json<TxSummary>, RestError> {
    validate_hash(&hash)?;
    Err(RestError::NotFound(format!("transaction {} not found", hash)))
}

/// POST /transactions — broadcast a signed raw transaction.
#[utoipa::path(
    post,
    path = "/api/v1/transactions",
    tag = "Transactions",
    request_body = BroadcastRequest,
    responses(
        (status = 200, description = "Transaction accepted", body = BroadcastResponse),
        (status = 400, description = "Invalid or already-known transaction"),
    )
)]
pub async fn broadcast_transaction(
    Json(req): Json<BroadcastRequest>,
) -> Result<Json<BroadcastResponse>, RestError> {
    if !req.raw_tx.starts_with("0x") {
        return Err(RestError::BadRequest(
            "raw_tx must be 0x-prefixed hex-encoded RLP".to_string()
        ));
    }
    // Production: submit to mempool via RpcState.
    Err(RestError::Internal("mempool not connected".to_string()))
}

fn validate_hash(h: &str) -> Result<(), RestError> {
    if !h.starts_with("0x") || h.len() != 66 {
        return Err(RestError::BadRequest(
            format!("invalid hash '{}': must be 0x + 32 hex bytes", h)
        ));
    }
    Ok(())
}
