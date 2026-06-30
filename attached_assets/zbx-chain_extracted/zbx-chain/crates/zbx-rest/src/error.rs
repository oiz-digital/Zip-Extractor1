//! REST API error types and HTTP error response formatting.

use axum::{http::StatusCode, response::{IntoResponse, Response}, Json};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RestError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("internal error: {0}")]
    Internal(String),
    #[error("rate limited: retry after {0}s")]
    RateLimited(u64),
}

#[derive(Serialize)]
struct ErrorBody {
    code:    u16,
    error:   String,
    message: String,
}

impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            RestError::NotFound(m)    => (StatusCode::NOT_FOUND,            404, m.clone()),
            RestError::BadRequest(m)  => (StatusCode::BAD_REQUEST,          400, m.clone()),
            RestError::Internal(m)    => (StatusCode::INTERNAL_SERVER_ERROR, 500, m.clone()),
            RestError::RateLimited(s) => (StatusCode::TOO_MANY_REQUESTS,    429,
                format!("Rate limited. Retry after {}s.", s)),
        };
        let body = ErrorBody {
            code,
            error: status.canonical_reason().unwrap_or("error").to_string(),
            message,
        };
        (status, Json(body)).into_response()
    }
}
