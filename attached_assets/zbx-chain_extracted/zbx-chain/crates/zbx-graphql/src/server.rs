//! GraphQL HTTP + WebSocket server using Axum.

use crate::schema::{build_schema, ZbxSchema};
use async_graphql::http::{playground_source, GraphQLPlaygroundConfig};
use async_graphql_axum::{GraphQLRequest, GraphQLResponse, GraphQLSubscription};
use axum::{
    extract::State,
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use std::net::SocketAddr;

/// Axum-based GraphQL server.
pub struct GraphqlServer {
    schema: ZbxSchema,
    bind:   SocketAddr,
}

impl GraphqlServer {
    /// Create a new GraphQL server bound to `addr`.
    pub fn new(addr: SocketAddr) -> Self {
        Self { schema: build_schema(), bind: addr }
    }

    /// Start the server (blocks until shutdown).
    pub async fn run(self) -> std::io::Result<()> {
        let schema = self.schema;
        let app = Router::new()
            .route("/graphql",    get(graphql_playground).post(graphql_handler))
            .route("/graphql/ws", get(graphql_ws_handler))
            .with_state(schema);

        tracing::info!("GraphQL server listening on {}", self.bind);
        let listener = tokio::net::TcpListener::bind(self.bind).await?;
        axum::serve(listener, app).await
    }
}

/// HTTP handler for GraphQL queries/mutations.
async fn graphql_handler(
    State(schema): State<ZbxSchema>,
    req: GraphQLRequest,
) -> GraphQLResponse {
    schema.execute(req.into_inner()).await.into()
}

/// WebSocket handler for GraphQL subscriptions.
async fn graphql_ws_handler(
    State(schema): State<ZbxSchema>,
    ws: axum::extract::WebSocketUpgrade,
) -> impl IntoResponse {
    GraphQLSubscription::new(schema).on_upgrade(ws)
}

/// Serve the GraphiQL playground UI.
async fn graphql_playground() -> impl IntoResponse {
    Html(playground_source(
        GraphQLPlaygroundConfig::new("/graphql").subscription_endpoint("/graphql/ws"),
    ))
}
