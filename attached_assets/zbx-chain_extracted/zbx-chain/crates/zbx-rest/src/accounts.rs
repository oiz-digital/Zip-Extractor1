//! Account REST endpoints.

use axum::{extract::Path, Json};
use crate::{error::RestError, types::{AccountInfo, Paginated, TxSummary}};

fn validate_address(addr: &str) -> Result<(), RestError> {
    if !addr.starts_with("0x") || addr.len() != 42 {
        return Err(RestError::BadRequest(
            format!("invalid address '{}': must be 0x + 20 hex bytes", addr)
        ));
    }
    Ok(())
}

/// GET /accounts/:address
#[utoipa::path(
    get,
    path = "/api/v1/accounts/{address}",
    tag = "Accounts",
    params(("address" = String, Path, description = "Wallet address (EIP-55)")),
    responses(
        (status = 200, description = "Account info", body = AccountInfo),
        (status = 400, description = "Invalid address"),
        (status = 404, description = "Account not found"),
    )
)]
pub async fn get_account(
    Path(address): Path<String>,
) -> Result<Json<AccountInfo>, RestError> {
    validate_address(&address)?;
    Err(RestError::NotFound(format!("account {} not found", address)))
}

/// GET /accounts/:address/transactions
#[utoipa::path(
    get,
    path = "/api/v1/accounts/{address}/transactions",
    tag = "Accounts",
    params(("address" = String, Path, description = "Wallet address (EIP-55)")),
    responses(
        (status = 200, description = "Transaction history", body = Paginated<TxSummary>),
        (status = 400, description = "Invalid address"),
    )
)]
pub async fn get_account_transactions(
    Path(address): Path<String>,
) -> Result<Json<Paginated<TxSummary>>, RestError> {
    validate_address(&address)?;
    Ok(Json(Paginated { items: vec![], total: 0, page: 1, limit: 20 }))
}
