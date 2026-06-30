//! Network REST endpoints.

use axum::Json;
use crate::{error::RestError, types::{GasInfo, NetworkInfo}};

/// GET /network/info
#[utoipa::path(
    get,
    path = "/api/v1/network/info",
    tag = "Network",
    responses(
        (status = 200, description = "Network info", body = NetworkInfo),
    )
)]
pub async fn get_network_info() -> Result<Json<NetworkInfo>, RestError> {
    Ok(Json(NetworkInfo {
        chain_id:        8990,
        chain_name:      "Zebvix Testnet".to_string(),
        network:         "testnet".to_string(),
        latest_block:    0,
        finalized_block: 0,
        peer_count:      0,
        sync_status:     "synced".to_string(),
        node_version:    "1.0.0".to_string(),
    }))
}

/// GET /network/gas
#[utoipa::path(
    get,
    path = "/api/v1/network/gas",
    tag = "Network",
    responses(
        (status = 200, description = "Current gas prices", body = GasInfo),
    )
)]
pub async fn get_gas_info() -> Result<Json<GasInfo>, RestError> {
    Ok(Json(GasInfo {
        base_fee:        "1000000000".to_string(),
        safe_gas_price:  "1100000000".to_string(),
        fast_gas_price:  "1500000000".to_string(),
        rapid_gas_price: "2000000000".to_string(),
    }))
}
