//! Validator REST endpoints.

use axum::{extract::Path, Json};
use crate::{error::RestError, types::{Paginated, ValidatorInfo}};

/// GET /validators
#[utoipa::path(
    get,
    path = "/api/v1/validators",
    tag = "Validators",
    responses(
        (status = 200, description = "Validator list", body = Paginated<ValidatorInfo>),
    )
)]
pub async fn list_validators() -> Result<Json<Paginated<ValidatorInfo>>, RestError> {
    Ok(Json(Paginated { items: vec![], total: 0, page: 1, limit: 100 }))
}

/// GET /validators/:address
#[utoipa::path(
    get,
    path = "/api/v1/validators/{address}",
    tag = "Validators",
    params(("address" = String, Path, description = "Validator address")),
    responses(
        (status = 200, description = "Validator detail", body = ValidatorInfo),
        (status = 400, description = "Invalid address"),
        (status = 404, description = "Validator not found"),
    )
)]
pub async fn get_validator(
    Path(address): Path<String>,
) -> Result<Json<ValidatorInfo>, RestError> {
    if !address.starts_with("0x") || address.len() != 42 {
        return Err(RestError::BadRequest(format!("invalid address: {}", address)));
    }
    Err(RestError::NotFound(format!("validator {} not found", address)))
}
