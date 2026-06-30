//! REST API Axum server with Swagger UI and OpenAPI spec endpoint.

use axum::{
    middleware,
    routing::{get, post},
    Router,
};
use std::net::SocketAddr;
use utoipa_swagger_ui::SwaggerUi;
use crate::{accounts, blocks, middleware as mw, network, openapi::ZbxApiDoc, transactions, validators};
use utoipa::OpenApi;

pub struct RestServer {
    bind: SocketAddr,
}

impl RestServer {
    pub fn new(addr: SocketAddr) -> Self {
        Self { bind: addr }
    }

    pub async fn run(self) -> std::io::Result<()> {
        let api_routes = Router::new()
            // Blocks
            .route("/blocks/latest",                     get(blocks::get_latest_block))
            .route("/blocks/:number",                    get(blocks::get_block_by_number))
            .route("/blocks/:number/transactions",       get(blocks::get_block_transactions))
            // Transactions
            .route("/transactions",                      post(transactions::broadcast_transaction))
            .route("/transactions/:hash",                get(transactions::get_transaction))
            // Accounts
            .route("/accounts/:address",                 get(accounts::get_account))
            .route("/accounts/:address/transactions",    get(accounts::get_account_transactions))
            // Validators
            .route("/validators",                        get(validators::list_validators))
            .route("/validators/:address",               get(validators::get_validator))
            // Network
            .route("/network/info",                      get(network::get_network_info))
            .route("/network/gas",                       get(network::get_gas_info))
            .layer(middleware::from_fn(mw::log_request))
            .layer(middleware::from_fn(mw::request_id));

        let openapi_json = Router::new()
            .route("/api/v1/openapi.json", get(|| async {
                axum::Json(ZbxApiDoc::openapi())
            }));

        let app = Router::new()
            .nest("/api/v1", api_routes)
            .merge(openapi_json)
            .merge(
                SwaggerUi::new("/api/v1/docs")
                    .url("/api/v1/openapi.json", ZbxApiDoc::openapi()),
            );

        tracing::info!("REST API server listening on http://{}", self.bind);
        tracing::info!("Swagger UI: http://{}/api/v1/docs", self.bind);
        let listener = tokio::net::TcpListener::bind(self.bind).await?;
        axum::serve(listener, app).await
    }
}
