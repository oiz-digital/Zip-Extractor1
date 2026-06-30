//! OpenAPI 3.1 spec generation using utoipa.

use utoipa::OpenApi;
use crate::types::*;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Zebvix Chain REST API",
        version = "1.0.0",
        description = "Production REST API for Zebvix Chain — blocks, transactions, accounts, validators, and network data.",
        contact(name = "Zebvix Dev", url = "https://docs.zebvix.com", email = "dev@zebvix.com"),
        license(name = "Apache-2.0"),
    ),
    servers(
        (url = "https://rpc.zebvix.com", description = "Mainnet"),
        (url = "https://testnet-rpc.zebvix.com", description = "Testnet"),
        (url = "http://localhost:8547", description = "Local"),
    ),
    paths(
        crate::blocks::get_latest_block,
        crate::blocks::get_block_by_number,
        crate::blocks::get_block_transactions,
        crate::transactions::get_transaction,
        crate::transactions::broadcast_transaction,
        crate::accounts::get_account,
        crate::accounts::get_account_transactions,
        crate::validators::list_validators,
        crate::validators::get_validator,
        crate::network::get_network_info,
        crate::network::get_gas_info,
    ),
    components(
        schemas(
            BlockSummary, BlockDetail, TxSummary, AccountInfo,
            ValidatorInfo, NetworkInfo, GasInfo,
            BroadcastRequest, BroadcastResponse,
        )
    ),
    tags(
        (name = "Blocks",       description = "Block data endpoints"),
        (name = "Transactions", description = "Transaction endpoints"),
        (name = "Accounts",     description = "Account state endpoints"),
        (name = "Validators",   description = "Validator info endpoints"),
        (name = "Network",      description = "Network and gas info"),
    ),
    security(("api_key" = [])),
)]
pub struct ZbxApiDoc;
