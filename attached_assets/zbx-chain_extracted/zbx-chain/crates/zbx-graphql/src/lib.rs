//! zbx-graphql — GraphQL API for Zebvix Chain.
//!
//! Exposes a production-ready GraphQL endpoint at `/graphql` using
//! [async-graphql](https://docs.rs/async-graphql). Supports queries,
//! subscriptions, and introspection.
//!
//! ## Endpoint
//!
//! | Path          | Protocol | Purpose                     |
//! |---------------|----------|-----------------------------|
//! | `/graphql`    | HTTP     | Query + Mutation             |
//! | `/graphql/ws` | WebSocket| Subscription (newBlocks, etc)|
//! | `/graphql/ui` | HTTP     | GraphiQL playground          |
//!
//! ## Schema overview
//!
//! ### Queries
//! - `block(number: Int, hash: String)` — fetch a block
//! - `transaction(hash: String!)` — fetch a transaction
//! - `account(address: String!)` — fetch account (balance, nonce, code)
//! - `validator(address: String!)` — validator details
//! - `validators(active: Boolean)` — list validators
//! - `chainInfo` — chain metadata
//!
//! ### Subscriptions
//! - `newBlocks` — emits each new block header
//! - `pendingTransactions` — emits mempool txs
//! - `logs(address, topics)` — filtered event logs

pub mod query;
pub mod schema;
pub mod server;
pub mod subscription;
pub mod types;
pub mod error;

pub use schema::{build_schema, ZbxSchema};
pub use server::GraphqlServer;
pub use error::GraphqlError;
