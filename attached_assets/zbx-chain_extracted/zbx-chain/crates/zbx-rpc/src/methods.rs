//! JSON-RPC method dispatch table.
//!
//! This module classifies methods and provides the public `dispatch` entry-point.
//! Actual per-method implementation lives in `eth_api` and `zbx_api`.
//!
//! ## Handler routing
//!
//! `dispatch` converts `Vec<Value>` params to a `Value::Array` and delegates to
//! `eth_api::dispatch_eth` or `zbx_api::dispatch_zbx` depending on the method
//! prefix.  This avoids holding raw function pointers to private handlers and
//! keeps the dispatch table as a pure name→class classification.

use crate::{
    error::RpcError,
    eth_api,
    state::RpcState,
    zbx_api,
};
use serde_json::Value;
use std::collections::HashMap;

// ── Method class ─────────────────────────────────────────────────────────────

/// Classification used to apply per-batch caps and auth checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodClass {
    /// Read-only query — no EVM execution.
    ReadOnly,
    /// EVM simulation (eth_call, eth_estimateGas) — subject to gas cap.
    Simulation,
    /// Mutates mempool (eth_sendRawTransaction) — rate-limited per IP.
    Mutation,
    /// Admin / debug — restricted to localhost or auth header.
    Admin,
}

// ── Method classification table ───────────────────────────────────────────────

struct MethodEntry {
    name: &'static str,
    class: MethodClass,
}

fn build_class_table() -> HashMap<&'static str, MethodClass> {
    let entries: &[MethodEntry] = &[
        // ── web3 / net ──────────────────────────────────────────────────────
        MethodEntry { name: "web3_clientVersion",                          class: MethodClass::ReadOnly },
        MethodEntry { name: "web3_sha3",                                   class: MethodClass::ReadOnly },
        MethodEntry { name: "net_version",                                 class: MethodClass::ReadOnly },
        MethodEntry { name: "net_listening",                               class: MethodClass::ReadOnly },
        MethodEntry { name: "net_peerCount",                               class: MethodClass::ReadOnly },
        // ── eth — block / chain state ────────────────────────────────────────
        MethodEntry { name: "eth_chainId",                                 class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_protocolVersion",                         class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_blockNumber",                             class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_getBlockByNumber",                        class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_getBlockByHash",                          class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_getBlockTransactionCountByNumber",        class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_getBlockTransactionCountByHash",          class: MethodClass::ReadOnly },
        // ── eth — account ────────────────────────────────────────────────────
        MethodEntry { name: "eth_getBalance",                              class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_getTransactionCount",                     class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_getCode",                                 class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_getStorageAt",                            class: MethodClass::ReadOnly },
        // ── eth — transaction ────────────────────────────────────────────────
        MethodEntry { name: "eth_getTransactionByHash",                    class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_getTransactionByBlockNumberAndIndex",     class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_getTransactionByBlockHashAndIndex",       class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_getTransactionReceipt",                   class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_sendRawTransaction",                      class: MethodClass::Mutation },
        // ── eth — simulation ─────────────────────────────────────────────────
        MethodEntry { name: "eth_call",                                    class: MethodClass::Simulation },
        MethodEntry { name: "eth_estimateGas",                             class: MethodClass::Simulation },
        // ── eth — fee / gas ──────────────────────────────────────────────────
        MethodEntry { name: "eth_gasPrice",                                class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_maxPriorityFeePerGas",                    class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_feeHistory",                              class: MethodClass::ReadOnly },
        // ── eth — logs / filters ─────────────────────────────────────────────
        MethodEntry { name: "eth_getLogs",                                 class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_newFilter",                               class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_newBlockFilter",                          class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_getFilterChanges",                        class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_uninstallFilter",                         class: MethodClass::ReadOnly },
        // ── eth — misc ───────────────────────────────────────────────────────
        MethodEntry { name: "eth_syncing",                                 class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_mining",                                  class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_accounts",                                class: MethodClass::ReadOnly },
        MethodEntry { name: "eth_sign",                                    class: MethodClass::ReadOnly },
        // ── txpool (geth-compatible) ─────────────────────────────────────────
        MethodEntry { name: "txpool_status",                               class: MethodClass::ReadOnly },
        MethodEntry { name: "txpool_content",                              class: MethodClass::ReadOnly },
        // ── zbx — staking ────────────────────────────────────────────────────
        MethodEntry { name: "zbx_getValidators",                           class: MethodClass::ReadOnly },
        MethodEntry { name: "zbx_getValidatorInfo",                        class: MethodClass::ReadOnly },
        MethodEntry { name: "zbx_getDelegations",                          class: MethodClass::ReadOnly },
        MethodEntry { name: "zbx_getStakingRewards",                       class: MethodClass::ReadOnly },
        MethodEntry { name: "zbx_sendStakingTx",                          class: MethodClass::Mutation },
        // ── zbx — chain info ─────────────────────────────────────────────────
        MethodEntry { name: "zbx_getChainInfo",                            class: MethodClass::ReadOnly },
        MethodEntry { name: "zbx_getValidatorSet",                         class: MethodClass::ReadOnly },
        MethodEntry { name: "zbx_getStakingInfo",                          class: MethodClass::ReadOnly },
        MethodEntry { name: "zbx_getBlockReward",                          class: MethodClass::ReadOnly },
        MethodEntry { name: "zbx_getEpochInfo",                            class: MethodClass::ReadOnly },
        // ── zbx — bridge ─────────────────────────────────────────────────────
        MethodEntry { name: "zbx_getBridgeInfo",                           class: MethodClass::ReadOnly },
        MethodEntry { name: "zbx_getBridgePendingDeposits",                class: MethodClass::ReadOnly },
        MethodEntry { name: "zbx_getBridgeStatus",                         class: MethodClass::ReadOnly },
        // ── zbx — governance ─────────────────────────────────────────────────
        MethodEntry { name: "zbx_proposeGovernance",                       class: MethodClass::Mutation },
        MethodEntry { name: "zbx_getGovernanceProposal",                   class: MethodClass::ReadOnly },
        // ── zbx — node / network ─────────────────────────────────────────────
        MethodEntry { name: "zbx_nodeInfo",                                class: MethodClass::ReadOnly },
        MethodEntry { name: "zbx_networkId",                               class: MethodClass::ReadOnly },
        // ── zbx — ZK / prover ────────────────────────────────────────────────
        MethodEntry { name: "zbx_getProofStatus",                          class: MethodClass::ReadOnly },
        // ── zbx — AI precompile ───────────────────────────────────────────────
        MethodEntry { name: "zbx_aiInference",                             class: MethodClass::Simulation },
        // ── xcl — cross-chain ────────────────────────────────────────────────
        MethodEntry { name: "xcl_getInfo",                                 class: MethodClass::ReadOnly },
        MethodEntry { name: "xcl_getChannels",                             class: MethodClass::ReadOnly },
        MethodEntry { name: "xcl_getChannel",                              class: MethodClass::ReadOnly },
        MethodEntry { name: "xcl_getClients",                              class: MethodClass::ReadOnly },
        MethodEntry { name: "xcl_getClient",                               class: MethodClass::ReadOnly },
        MethodEntry { name: "xcl_sendPacket",                              class: MethodClass::Mutation },
        MethodEntry { name: "xcl_getPacketStatus",                         class: MethodClass::ReadOnly },
        MethodEntry { name: "xcl_getRelayStats",                           class: MethodClass::ReadOnly },
    ];

    entries.iter().map(|e| (e.name, e.class)).collect()
}

// ── Global class table ────────────────────────────────────────────────────────

static CLASS_TABLE: std::sync::OnceLock<HashMap<&'static str, MethodClass>> =
    std::sync::OnceLock::new();

fn class_table() -> &'static HashMap<&'static str, MethodClass> {
    CLASS_TABLE.get_or_init(build_class_table)
}

// ── Dispatch ─────────────────────────────────────────────────────────────────

/// Dispatch a JSON-RPC method call.
///
/// Routes to `eth_api::dispatch_eth` or `zbx_api::dispatch_zbx` based on
/// method prefix.  Returns `Err(RpcError::MethodNotFound)` for unknown methods.
pub fn dispatch(
    method: &str,
    params: Vec<Value>,
    state: &RpcState,
) -> Result<Value, RpcError> {
    let params_value = Value::Array(params);
    if method.starts_with("eth_")
        || method.starts_with("net_")
        || method.starts_with("web3_")
        || method.starts_with("txpool_")
    {
        eth_api::dispatch_eth(method, &params_value, state)
    } else if method.starts_with("zbx_") || method.starts_with("xcl_") {
        zbx_api::dispatch_zbx(method, &params_value, state)
    } else {
        Err(RpcError::MethodNotFound(method.to_string()))
    }
}

/// Check whether a method is a simulation method (subject to gas cap).
pub fn is_simulation(method: &str) -> bool {
    class_table()
        .get(method)
        .map(|&c| c == MethodClass::Simulation)
        .unwrap_or(false)
}

/// Check whether a method is a mutation (subject to rate-limiting).
pub fn is_mutation(method: &str) -> bool {
    class_table()
        .get(method)
        .map(|&c| c == MethodClass::Mutation)
        .unwrap_or(false)
}

/// Returns all registered method names (sorted alphabetically).
pub fn all_method_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = class_table().keys().copied().collect();
    names.sort_unstable();
    names
}
