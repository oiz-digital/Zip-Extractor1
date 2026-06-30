//! zbx-rest — Zebvix Chain REST API (OpenAPI 3.1)
//!
//! Provides a standards-compliant REST API alongside the existing JSON-RPC
//! endpoint. All endpoints follow OpenAPI 3.1 spec and are documented with
//! Swagger UI at `/api/v1/docs`.
//!
//! ## Base path: `/api/v1`
//!
//! ### Blocks
//! - `GET /blocks/latest`
//! - `GET /blocks/{number}`
//! - `GET /blocks/{number}/transactions`
//!
//! ### Transactions
//! - `GET /transactions/{hash}`
//! - `POST /transactions` — broadcast a signed raw transaction
//!
//! ### Accounts
//! - `GET /accounts/{address}`
//! - `GET /accounts/{address}/transactions`
//! - `GET /accounts/{address}/tokens`
//!
//! ### Validators
//! - `GET /validators`
//! - `GET /validators/{address}`
//! - `GET /validators/{address}/delegators`
//!
//! ### Network
//! - `GET /network/info`
//! - `GET /network/peers`
//! - `GET /network/gas`
//!
//! ### Tokens
//! - `GET /tokens`
//! - `GET /tokens/{address}`
//!
//! ### Swagger UI
//! - `GET /api/v1/docs` — Swagger UI playground

pub mod blocks;
pub mod accounts;
pub mod error;
pub mod middleware;
pub mod network;
pub mod openapi;
pub mod server;
pub mod transactions;
pub mod types;
pub mod validators;

pub use error::RestError;
pub use server::RestServer;
