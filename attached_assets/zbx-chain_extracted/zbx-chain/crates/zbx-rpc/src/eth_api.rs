//! Standard Ethereum JSON-RPC methods (eth_*, net_*, web3_*).
//!
//! Every handler is wired to the persistent `RpcState` so the responses
//! reflect the real chain head, mempool, and account state.

use crate::{error::RpcError, state::RpcState};
use zbx_types::{address::Address, H256, U256};
use zbx_evm::{EVMContext, EVMInterpreter, ExitStatus, MockHost};
use zbx_fee::{GasPriceOracle, FeeHistoryEntry};
use serde_json::{json, Value};
use sha3::{Digest, Keccak256};
use tracing::debug;
use std::time::{SystemTime, UNIX_EPOCH};

// SEC-2026-05-09 (Pass-5 C8): hard cap on gas the RPC may forward to the
// EVM during eth_call / eth_estimateGas.  The block gas limit is 30M but
// public RPCs see arbitrary calldata from anonymous callers — without a
// cap a single connection can keep the node CPU-pinned by submitting
// tight loops at the block limit.  50M ≈ 1.5x block size: enough headroom
// for genuine off-chain estimation, far below what an attacker needs to
// cause meaningful damage.
const RPC_GAS_CAP: u64 = 50_000_000;

// SEC-2026-05-09 (Pass-5 H8): hard cap on calldata bytes accepted for
// eth_call / eth_estimateGas.  The HTTP body cap (~1 MB) bounds totals,
// but lets a 50-request batch carry 50 MB of hex-decode + EVM setup work.
const RPC_MAX_CALLDATA: usize = 128 * 1024;

// SEC-2026-05-09 (Pass-6): per-batch cumulative gas budget.  Set by
// `server::handle_request` for the duration of a batch via
// `set_batch_budget(Some(_))`; eth_call / eth_estimateGas consume from
// it via `batch_budget_consume`.  Thread-local because (a) RPC dispatch
// is synchronous and a batch's `.map(...).collect()` runs every
// sub-request on the same OS thread, and (b) a non-batch request must
// see `None` (no cap) on the same thread once the batch returns.
thread_local! {
    static BATCH_BUDGET: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// SEC-2026-05-09 (Pass-6): server entry-point hook to install / clear
/// the per-batch budget.  Called only by `server::handle_request`.
pub fn set_batch_budget(budget: Option<u64>) {
    BATCH_BUDGET.with(|b| b.set(budget));
}

/// SEC-2026-05-09 (Pass-6): consume `amount` gas from the active batch
/// budget if one is set.  Returns `Err` when the consumption would
/// exceed the remaining budget — the calling RPC handler propagates
/// the error to the client as a regular JSON-RPC error response.  No-op
/// when no batch is active (single-request path).
fn batch_budget_consume(amount: u64) -> Result<(), RpcError> {
    BATCH_BUDGET.with(|b| {
        if let Some(remaining) = b.get() {
            if amount > remaining {
                return Err(RpcError::InvalidRequest(format!(
                    "RPC batch gas budget exhausted: requested {} > remaining {} \
                     (per-batch cap = RPC_BATCH_GAS_BUDGET)",
                    amount, remaining
                )));
            }
            b.set(Some(remaining - amount));
        }
        Ok(())
    })
}

/// Dispatch an eth_* / net_* / web3_* method call.
pub fn dispatch_eth(method: &str, params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    debug!(method, "eth/net/web3 RPC call");
    match method {
        // chain / network identity
        "eth_chainId"               => Ok(json!(format!("0x{:x}", state.chain_id))),
        "eth_protocolVersion"       => Ok(json!("0x41")),
        "net_version"               => Ok(json!(state.chain_id.to_string())),
        "net_listening"             => Ok(json!(true)),
        "net_peerCount"             => Ok(json!(format!("0x{:x}", *state.peer_count.read()))),
        "web3_clientVersion"        => Ok(json!(state.client_version)),
        "web3_sha3"                 => web3_sha3(params),

        // chain head
        "eth_blockNumber"           => Ok(json!(format!("0x{:x}", state.latest_height()))),
        "eth_syncing"               => eth_syncing(state),
        "eth_gasPrice"              => gas_price_oracle(state),
        "eth_maxPriorityFeePerGas"  => eth_max_priority_fee(state),
        "eth_feeHistory"            => eth_fee_history(params, state),

        // accounts & balances
        "eth_getBalance"            => eth_get_balance(params, state),
        "eth_getTransactionCount"   => eth_get_transaction_count(params, state),
        "eth_getCode"               => eth_get_code(params, state),
        "eth_getStorageAt"          => eth_get_storage_at(params, state),

        // call / estimate
        "eth_call"                  => eth_call(params, state),
        "eth_estimateGas"           => eth_estimate_gas(params, state),

        // submit
        "eth_sendRawTransaction"    => eth_send_raw_transaction(params, state),

        // blocks
        "eth_getBlockByNumber"      => eth_get_block_by_number(params, state),
        "eth_getBlockByHash"        => eth_get_block_by_hash(params, state),
        "eth_getBlockTransactionCountByNumber"
                                    => eth_get_block_tx_count_by_number(params, state),
        "eth_getBlockTransactionCountByHash"
                                    => eth_get_block_tx_count_by_hash(params, state),

        // transactions / receipts
        "eth_getTransactionByHash"  => eth_get_transaction_by_hash(params, state),
        "eth_getTransactionReceipt" => eth_get_transaction_receipt(params, state),
        "eth_getLogs"               => eth_get_logs(params, state),

        // mempool stats (geth-compatible)
        "txpool_status"             => txpool_status(state),
        "txpool_content"            => Ok(json!({"pending":{}, "queued":{}})),

        _ => Err(RpcError::MethodNotFound(method.to_string())),
    }
}

// ---------------------------------------------------------------------------
// Gas Price Oracle (EIP-1559 base-fee + tip buffer)
// ---------------------------------------------------------------------------

/// Return a recommended gas price based on the median base fee of the last
/// N sealed blocks, plus a 10% buffer and a 1 gwei priority tip.
///
/// Falls back to 1 gwei if no blocks are available (devnet cold start).
fn gas_price_oracle(state: &RpcState) -> Result<Value, RpcError> {
    // EIP-1559 wiring: use zbx-fee GasPriceOracle for a weighted, percentile-
    // based estimate over the last 10 blocks rather than a simple median scan.
    const LOOKBACK: u64 = 10;
    const MIN_PRICE: u64 = 1_000_000_000; // 1 gwei

    let latest = state.latest_height();
    let from   = latest.saturating_sub(LOOKBACK.saturating_sub(1));

    let mut oracle = GasPriceOracle::new(LOOKBACK as usize);
    for h in from..=latest {
        if let Ok(Some(block)) = state.db.get_block_by_number(h) {
            // Collect effective tips for each tx in the block.
            let base_fee = block.header.base_fee_per_gas;
            let tips: Vec<u64> = block.body.transactions.iter().map(|tx| {
                tx.effective_gas_price(base_fee).saturating_sub(base_fee)
            }).collect();
            oracle.update(base_fee, tips);
        }
    }

    let price = oracle.gas_price().max(MIN_PRICE);
    Ok(json!(format!("0x{:x}", price)))
}

/// `eth_maxPriorityFeePerGas` — EIP-1559 tip suggestion based on recent blocks.
///
/// Samples up to 10 blocks of per-tx priority fees, then returns the median
/// (50th-percentile) tip using the zbx-fee PriorityFeeEstimator.  Prior to
/// this wiring the endpoint returned a hardcoded 1 Gwei constant.
fn eth_max_priority_fee(state: &RpcState) -> Result<Value, RpcError> {
    const LOOKBACK: u64 = 10;
    const FALLBACK_TIP: u64 = 1_000_000_000; // 1 gwei

    let latest = state.latest_height();
    let from   = latest.saturating_sub(LOOKBACK.saturating_sub(1));

    let mut oracle = GasPriceOracle::new(LOOKBACK as usize);
    for h in from..=latest {
        if let Ok(Some(block)) = state.db.get_block_by_number(h) {
            let base_fee = block.header.base_fee_per_gas;
            let tips: Vec<u64> = block.body.transactions.iter().map(|tx| {
                tx.effective_gas_price(base_fee).saturating_sub(base_fee)
            }).collect();
            oracle.update(base_fee, tips);
        }
    }

    // gas_price() = base_fee + medium_tip; subtract base_fee to get tip alone.
    let current_base = oracle.base_fee().max(1);
    let recommended_gas_price = oracle.gas_price().max(current_base);
    let tip = recommended_gas_price
        .saturating_sub(current_base)
        .max(FALLBACK_TIP);

    Ok(json!(format!("0x{:x}", tip)))
}

// ---------------------------------------------------------------------------
// eth_getLogs
// ---------------------------------------------------------------------------

/// Filter criteria for `eth_getLogs`.  All fields are optional.
struct LogFilter {
    from_block:  u64,
    to_block:    u64,
    address:     Option<Address>,
    topics:      Vec<Option<H256>>,
}

impl LogFilter {
    fn from_params(params: &Value, state: &RpcState) -> Result<Self, RpcError> {
        let obj = params.get(0).unwrap_or(&Value::Null);

        let from_block = parse_block_number(
            obj.get("fromBlock").or(obj.get("from_block")),
            state,
        ).unwrap_or(0);

        let to_block = parse_block_number(
            obj.get("toBlock").or(obj.get("to_block")),
            state,
        ).unwrap_or_else(|_| state.latest_height());

        let address = obj.get("address")
            .and_then(Value::as_str)
            .and_then(|s| parse_address(s).ok());

        let topics = obj.get("topics")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter().map(|t| {
                    t.as_str()
                        .and_then(|s| parse_h256(s).ok())
                }).collect()
            })
            .unwrap_or_default();

        Ok(LogFilter { from_block, to_block, address, topics })
    }

    /// True if a log emitted by `addr` with `log_topics` passes this filter.
    fn matches(&self, addr: &Address, log_topics: &[H256]) -> bool {
        if let Some(ref filter_addr) = self.address {
            if addr.as_bytes() != filter_addr.as_bytes() {
                return false;
            }
        }
        for (i, topic_filter) in self.topics.iter().enumerate() {
            if let Some(required) = topic_filter {
                if log_topics.get(i) != Some(required) {
                    return false;
                }
            }
        }
        true
    }
}

fn eth_get_logs(params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    let filter = LogFilter::from_params(params, state)?;

    // Clamp range to avoid DoS on very wide queries.
    const MAX_RANGE: u64 = 2_000;
    let effective_from = filter.from_block;
    let effective_to   = filter.to_block.min(effective_from.saturating_add(MAX_RANGE));

    // SEC-2026-05-09 Pass-15 (HIGH-R04 "log-bomb"): pre-fix
    // `eth_getLogs` had no cap on the number of matched log entries
    // returned. A wide topic filter against a chain with even
    // moderate event traffic could return millions of log entries —
    // each ~500 B JSON — saturating the node's RAM allocation and
    // upstream pipe before the response was streamed. Cap matches
    // alchemy/infura defaults.
    const MAX_LOGS_PER_RESPONSE: usize = 10_000;
    let mut logs: Vec<Value> = Vec::new();

    'outer: for h in effective_from..=effective_to {
        let block = match state.db.get_block_by_number(h) {
            Ok(Some(b)) => b,
            _           => continue,
        };

        let block_hash = format!("0x{}", hex::encode(block.hash().as_bytes()));

        for (tx_idx, tx) in block.body.transactions.iter().enumerate() {
            let receipt = match state.db.get_receipt(&tx.hash) {
                Ok(Some(r)) => r,
                _           => continue,
            };

            for (log_idx, log) in receipt.logs.iter().enumerate() {
                let log_topics: Vec<H256> = log.topics
                    .iter()
                    .map(|t| H256::from_slice(t.as_bytes()))
                    .collect();

                if !filter.matches(&log.address, &log_topics) {
                    continue;
                }

                let topics_json: Vec<String> = log_topics.iter()
                    .map(|t| format!("0x{}", hex::encode(t.as_bytes())))
                    .collect();

                logs.push(json!({
                    "removed":          false,
                    "logIndex":         format!("0x{:x}", log_idx),
                    "transactionIndex": format!("0x{:x}", tx_idx),
                    "transactionHash":  format!("0x{}", hex::encode(tx.hash.as_bytes())),
                    "blockHash":        block_hash,
                    "blockNumber":      format!("0x{:x}", h),
                    "address":          format!("0x{}", hex::encode(log.address.as_bytes())),
                    "data":             format!("0x{}", hex::encode(&log.data)),
                    "topics":           topics_json,
                }));

                // SEC-2026-05-09 Pass-15 (HIGH-R04): hard cap. We
                // return what we have rather than erroring so wallet
                // UIs degrade gracefully; clients with stricter needs
                // should narrow their filter range.
                if logs.len() >= MAX_LOGS_PER_RESPONSE {
                    debug!(cap = MAX_LOGS_PER_RESPONSE, "eth_getLogs: log-count cap hit, truncating");
                    break 'outer;
                }
            }
        }
    }

    Ok(json!(logs))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_address(s: &str) -> Result<Address, RpcError> {
    Address::from_hex(s).map_err(|e| RpcError::InvalidParams(format!("address: {e}")))
}

fn parse_block_number(v: Option<&Value>, state: &RpcState) -> Result<u64, RpcError> {
    match v.and_then(Value::as_str) {
        None | Some("latest") | Some("pending") | Some("safe") | Some("finalized") => {
            Ok(state.latest_height())
        }
        Some("earliest") => Ok(0),
        Some(hex) if hex.starts_with("0x") => u64::from_str_radix(&hex[2..], 16)
            .map_err(|e| RpcError::InvalidParams(format!("block number: {e}"))),
        Some(other) => other
            .parse::<u64>()
            .map_err(|e| RpcError::InvalidParams(format!("block number: {e}"))),
    }
}

fn parse_h256(s: &str) -> Result<H256, RpcError> {
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    let bytes = hex::decode(stripped)
        .map_err(|e| RpcError::InvalidParams(format!("hex decode: {e}")))?;
    if bytes.len() != 32 {
        return Err(RpcError::InvalidParams(format!(
            "expected 32-byte hash, got {} bytes",
            bytes.len()
        )));
    }
    Ok(H256::from_slice(&bytes))
}

fn u256_to_hex(u: &U256) -> String {
    // Canonical ETH-style minimal hex: "0x" + lowercase, no leading zeros (except "0x0").
    if u.is_zero() {
        return "0x0".to_string();
    }
    let mut buf = [0u8; 32];
    u.to_big_endian(&mut buf);
    let first_nz = buf.iter().position(|b| *b != 0).unwrap_or(31);
    let bytes = &buf[first_nz..];
    let mut s = String::from("0x");
    let mut started = false;
    for b in bytes {
        if !started && *b < 0x10 {
            s.push_str(&format!("{:x}", b));
        } else {
            s.push_str(&format!("{:02x}", b));
        }
        started = true;
    }
    s
}

// ---------------------------------------------------------------------------
// eth_* handlers
// ---------------------------------------------------------------------------

fn eth_syncing(state: &RpcState) -> Result<Value, RpcError> {
    if *state.syncing.read() {
        Ok(json!({
            "startingBlock": "0x0",
            "currentBlock":  format!("0x{:x}", state.latest_height()),
            "highestBlock":  format!("0x{:x}", state.latest_height()),
        }))
    } else {
        Ok(json!(false))
    }
}

fn eth_get_balance(params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    let addr_s = params
        .get(0)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("missing address".into()))?;
    let addr = parse_address(addr_s)?;
    let acct = state
        .db
        .get_account(&addr)
        .map_err(|e| RpcError::Internal(format!("storage: {e}")))?;
    Ok(json!(u256_to_hex(&acct.balance)))
}

fn eth_get_transaction_count(params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    let addr_s = params
        .get(0)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("missing address".into()))?;
    let addr = parse_address(addr_s)?;
    let acct = state
        .db
        .get_account(&addr)
        .map_err(|e| RpcError::Internal(format!("storage: {e}")))?;
    Ok(json!(format!("0x{:x}", acct.nonce)))
}

fn eth_get_code(params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    let addr_s = params
        .get(0)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("missing address".into()))?;
    let addr = parse_address(addr_s)?;
    let acct = state
        .db
        .get_account(&addr)
        .map_err(|e| RpcError::Internal(format!("storage: {e}")))?;
    let code = state
        .db
        .get_code(&acct.code_hash)
        .map_err(|e| RpcError::Internal(format!("storage: {e}")))?;
    Ok(json!(format!("0x{}", hex::encode(code))))
}

fn eth_get_storage_at(params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    let addr_s = params
        .get(0)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("missing address".into()))?;
    let slot_s = params
        .get(1)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("missing slot".into()))?;
    let addr = parse_address(addr_s)?;
    let slot = parse_h256(slot_s)?;
    let value = state
        .db
        .get_storage(&addr, slot.as_fixed_bytes())
        .map_err(|e| RpcError::Internal(format!("storage: {e}")))?;
    Ok(json!(format!("0x{}", hex::encode(value))))
}

fn eth_call(params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    let call = params
        .get(0)
        .ok_or_else(|| RpcError::InvalidParams("missing call object".into()))?;

    let from = call
        .get("from")
        .and_then(Value::as_str)
        .and_then(|s| Address::from_hex(s).ok())
        .unwrap_or(Address::ZERO);

    let to = call
        .get("to")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("missing 'to' address".into()))
        .and_then(|s| {
            Address::from_hex(s).map_err(|e| RpcError::InvalidParams(format!("to: {e}")))
        })?;

    let calldata = call
        .get("data")
        .or_else(|| call.get("input"))
        .and_then(Value::as_str)
        .map(|s| hex::decode(s.strip_prefix("0x").unwrap_or(s)).unwrap_or_default())
        .unwrap_or_default();

    // SEC-2026-05-09 (Pass-5 H8): bound calldata length per call.
    if calldata.len() > RPC_MAX_CALLDATA {
        return Err(RpcError::InvalidParams(format!(
            "calldata exceeds {} byte cap (got {})",
            RPC_MAX_CALLDATA,
            calldata.len()
        )));
    }

    let gas_limit = call
        .get("gas")
        .and_then(Value::as_str)
        .and_then(|s| u64::from_str_radix(s.strip_prefix("0x").unwrap_or(s), 16).ok())
        .unwrap_or(RPC_GAS_CAP)
        // SEC-2026-05-09 (Pass-5 C8): cap user-supplied gas at the RPC
        // ceiling to bound CPU per request.  The EVM is a step-counter
        // bounded by gas; capping gas caps execution time.
        .min(RPC_GAS_CAP);

    // SEC-2026-05-09 (Pass-6): consume from the per-batch gas budget.
    // No-op when not inside a batch.  Errors propagate to the client.
    batch_budget_consume(gas_limit)?;

    let value_bytes = parse_call_value(call);

    let to_acct = state.db.get_account(&to).unwrap_or_default();
    let code = if to_acct.is_contract() {
        state.db.get_code(&to_acct.code_hash).unwrap_or_default()
    } else {
        Vec::new()
    };

    if code.is_empty() {
        return Ok(json!("0x"));
    }

    let from_acct = state.db.get_account(&from).unwrap_or_default();
    let mut host = MockHost::new();
    host.install_code(&to, code.clone());
    {
        let mut bal = [0u8; 32];
        from_acct.balance.to_big_endian(&mut bal);
        host.credit(&from, bal);
        host.set_nonce(&from, from_acct.nonce);
    }

    let block_number = state.latest_height();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let ctx = EVMContext {
        caller: from,
        callee: to,
        value: value_bytes,
        calldata,
        gas_limit,
        is_static: true,
        block_number,
        timestamp,
        coinbase: Address::ZERO,
        base_fee: 1_000_000_000,
        gas_price: 1_000_000_000,
        tx_origin: from,
        chain_id: state.chain_id,
        randao_mix: [0u8; 32],
    };

    let mut interp = EVMInterpreter::new(ctx, code, &mut host);
    let (status, _gas_used) = interp.run();
    let return_data = interp.return_data().to_vec();

    match status {
        ExitStatus::Succeeded => Ok(json!(format!("0x{}", hex::encode(return_data)))),
        ExitStatus::Reverted => Err(RpcError::Execution(format!(
            "execution reverted: 0x{}",
            hex::encode(return_data)
        ))),
        ExitStatus::Failed(e) => Err(RpcError::Execution(format!("EVM error: {e:?}"))),
    }
}

fn eth_estimate_gas(params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    let call = params
        .get(0)
        .ok_or_else(|| RpcError::InvalidParams("missing call object".into()))?;

    let from = call
        .get("from")
        .and_then(Value::as_str)
        .and_then(|s| Address::from_hex(s).ok())
        .unwrap_or(Address::ZERO);

    let calldata = call
        .get("data")
        .or_else(|| call.get("input"))
        .and_then(Value::as_str)
        .map(|s| hex::decode(s.strip_prefix("0x").unwrap_or(s)).unwrap_or_default())
        .unwrap_or_default();

    // SEC-2026-05-09 (Pass-5 H8): bound calldata length per call.
    if calldata.len() > RPC_MAX_CALLDATA {
        return Err(RpcError::InvalidParams(format!(
            "calldata exceeds {} byte cap (got {})",
            RPC_MAX_CALLDATA,
            calldata.len()
        )));
    }

    let value_bytes = parse_call_value(call);

    let intrinsic: u64 = 21_000
        + calldata
            .iter()
            .map(|&b| if b == 0 { 4u64 } else { 16u64 })
            .sum::<u64>();

    let to = match call.get("to").and_then(Value::as_str) {
        Some(s) => Address::from_hex(s)
            .map_err(|e| RpcError::InvalidParams(format!("to: {e}")))?,
        None => {
            return Ok(json!(format!("0x{:x}", intrinsic + 53_000)));
        }
    };

    let to_acct = state.db.get_account(&to).unwrap_or_default();
    let code = if to_acct.is_contract() {
        state.db.get_code(&to_acct.code_hash).unwrap_or_default()
    } else {
        Vec::new()
    };

    if code.is_empty() {
        return Ok(json!(format!("0x{:x}", intrinsic)));
    }

    let from_acct = state.db.get_account(&from).unwrap_or_default();
    let mut host = MockHost::new();
    host.install_code(&to, code.clone());
    {
        let mut bal = [0u8; 32];
        from_acct.balance.to_big_endian(&mut bal);
        host.credit(&from, bal);
        host.set_nonce(&from, from_acct.nonce);
    }

    let block_number = state.latest_height();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // SEC-2026-05-09 (Pass-6): consume from the per-batch gas budget
    // for estimateGas as well — it runs the same EVM machinery.
    batch_budget_consume(RPC_GAS_CAP)?;

    let ctx = EVMContext {
        caller: from,
        callee: to,
        value: value_bytes,
        calldata,
        // SEC-2026-05-09 (Pass-5 C8): cap estimateGas execution at
        // RPC_GAS_CAP, not the 30M block limit.  estimateGas runs
        // arbitrary user calldata and was the easiest CPU-pin DoS vector.
        gas_limit: RPC_GAS_CAP,
        is_static: false,
        block_number,
        timestamp,
        coinbase: Address::ZERO,
        base_fee: 1_000_000_000,
        gas_price: 1_000_000_000,
        tx_origin: from,
        chain_id: state.chain_id,
        randao_mix: [0u8; 32],
    };

    let mut interp = EVMInterpreter::new(ctx, code, &mut host);
    let (status, evm_gas) = interp.run();

    match status {
        ExitStatus::Succeeded => {
            let total = intrinsic.saturating_add(evm_gas);
            let with_buffer = ((total as f64) * 1.2) as u64;
            Ok(json!(format!("0x{:x}", with_buffer.max(intrinsic))))
        }
        ExitStatus::Reverted => {
            let revert = interp.return_data().to_vec();
            Err(RpcError::Execution(format!(
                "execution reverted: 0x{}",
                hex::encode(revert)
            )))
        }
        ExitStatus::Failed(e) => Err(RpcError::Execution(format!("EVM error: {e:?}"))),
    }
}

/// Parse the `value` field from a call object into a 32-byte big-endian array.
fn parse_call_value(call: &Value) -> [u8; 32] {
    let mut out = [0u8; 32];
    if let Some(s) = call.get("value").and_then(Value::as_str) {
        let stripped = s.strip_prefix("0x").unwrap_or(s);
        let normalized = if stripped.len() % 2 == 1 {
            format!("0{stripped}")
        } else {
            stripped.to_string()
        };
        if let Ok(bytes) = hex::decode(&normalized) {
            let start = 32usize.saturating_sub(bytes.len());
            let copy_len = bytes.len().min(32);
            out[start..start + copy_len].copy_from_slice(&bytes[..copy_len]);
        }
    }
    out
}

fn eth_fee_history(params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    // EIP-1559 wiring (zbx-fee): replaced hardcoded stub with real per-block
    // base fees, gas usage ratios, and percentile reward data read from the
    // chain DB.  Prior implementation returned "0x3B9ACA00" (1 Gwei) for every
    // field, which caused wallets to underestimate fees on high-load networks.

    // --- parse block_count (first param, hex or decimal string or integer) ---
    let count = params
        .get(0)
        .and_then(|v| {
            if let Some(s) = v.as_str() {
                u64::from_str_radix(s.strip_prefix("0x").unwrap_or(s), 16).ok()
                    .or_else(|| s.parse::<u64>().ok())
            } else {
                v.as_u64()
            }
        })
        .unwrap_or(1)
        .clamp(1, 1024);

    // --- optional reward percentiles (third param) ---
    let percentiles: Vec<f64> = params
        .get(2)
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();

    // --- resolve newest block (second param) ---
    let head = state.latest_height();
    let newest = parse_block_number(params.get(1), state).unwrap_or(head).min(head);
    let oldest = newest.saturating_sub(count.saturating_sub(1));

    // --- collect real entries from the DB via zbx-fee FeeHistoryEntry ---
    let mut fee_history = zbx_fee::FeeHistory::new(count as usize);
    for h in oldest..=newest {
        if let Ok(Some(block)) = state.db.get_block_by_number(h) {
            let base_fee = block.header.base_fee_per_gas;
            let gas_limit = block.header.gas_limit;
            let gas_used  = block.header.gas_used;
            let gas_used_ratio = if gas_limit == 0 {
                0.0
            } else {
                (gas_used as f64) / (gas_limit as f64)
            };

            // Per-tx effective tips for reward percentiles.
            let mut tips: Vec<u64> = block.body.transactions.iter().map(|tx| {
                tx.effective_gas_price(base_fee).saturating_sub(base_fee)
            }).collect();
            tips.sort_unstable();

            let rewards: Vec<u64> = percentiles.iter().map(|&p| {
                if tips.is_empty() {
                    0u64
                } else {
                    let idx = (((tips.len() - 1) as f64) * p / 100.0) as usize;
                    tips[idx.min(tips.len() - 1)]
                }
            }).collect();

            fee_history.push(FeeHistoryEntry { base_fee_per_gas: base_fee, gas_used_ratio, rewards });
        }
    }

    // Build baseFeePerGas array: historical entries + predicted next block fee.
    let entries = fee_history.last_n(count as usize);
    let mut base_fees: Vec<String> = entries.iter()
        .map(|e| format!("0x{:x}", e.base_fee_per_gas))
        .collect();
    // Append predicted next block base fee (EIP-1559 spec requires it).
    let next_fee = fee_history.next_base_fee()
        .unwrap_or(1_000_000_000);
    base_fees.push(format!("0x{:x}", next_fee));

    let gas_used_ratios: Vec<f64> = entries.iter()
        .map(|e| e.gas_used_ratio)
        .collect();

    let rewards_json: Vec<Vec<String>> = if percentiles.is_empty() {
        vec![]
    } else {
        entries.iter().map(|e| {
            e.rewards.iter().map(|r| format!("0x{:x}", r)).collect()
        }).collect()
    };

    let actual_oldest = newest.saturating_sub(entries.len().saturating_sub(1) as u64);

    let mut result = json!({
        "oldestBlock":   format!("0x{:x}", actual_oldest),
        "baseFeePerGas": base_fees,
        "gasUsedRatio":  gas_used_ratios,
    });
    if !percentiles.is_empty() {
        result["reward"] = json!(rewards_json);
    }
    Ok(result)
}

fn eth_send_raw_transaction(params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    let raw = params
        .get(0)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("missing rawTransaction".into()))?;
    let stripped = raw.strip_prefix("0x").unwrap_or(raw);
    let bytes = hex::decode(stripped)
        .map_err(|e| RpcError::InvalidParams(format!("hex: {e}")))?;

    // 1. Decode the raw bytes into our internal SignedTransaction (legacy /
    //    EIP-2930 / EIP-1559) and capture the canonical Ethereum tx hash.
    let (signed_tx, eth_hash) = crate::tx_decode::decode_raw_tx(&bytes)?;

    // SEC-2026-05-09 (R4): reject txs whose chain_id does not match this
    // node's chain_id. The previous "chain_id == 0 → accept" carve-out for
    // pre-EIP-155 legacy txs is a cross-chain replay vector — a legacy tx
    // signed once is valid on every EIP-155-aware chain. Modern wallets
    // (MetaMask, Rabby, ethers v6) all sign with EIP-155 by default, so
    // refusing chain_id == 0 costs nothing in practice.
    if signed_tx.tx.chain_id != state.chain_id {
        return Err(RpcError::InvalidParams(format!(
            "R4: wrong chainId (tx={}, node={}); pre-EIP-155 unprotected \
             txs are not accepted",
            signed_tx.tx.chain_id, state.chain_id
        )));
    }

    // 3. Look up sender balance + on-chain nonce so the mempool can run its
    //    cost / nonce / replacement validations.
    let sender_acct = state
        .db
        .get_account(&signed_tx.from)
        .map_err(|e| RpcError::Internal(format!("storage get_account: {e}")))?;
    let sender_balance = sender_acct.balance_u128();
    let sender_nonce = sender_acct.nonce;

    // 4. Insert into the mempool. Translate domain-specific errors into
    //    JSON-RPC error codes that wallets / dApps know how to handle.
    // Clone before move so we can relay the TX over P2P on success.
    let tx_for_relay = signed_tx.clone();
    let added = {
        let mut pool = state.mempool.write();
        pool.add_transaction(signed_tx, sender_balance, sender_nonce)
    };
    match added {
        Ok(_) => {
            // 5. Relay accepted TX to all connected P2P peers so validators
            //    that did NOT receive this RPC call also include it in their
            //    mempool (multi-validator TX propagation).
            let _ = state.tx_relay_tx.send(tx_for_relay);
            Ok(json!(format!("0x{}", hex::encode(eth_hash.as_bytes()))))
        }
        Err(e) => Err(RpcError::InvalidParams(format!("mempool: {e}"))),
    }
}

fn eth_get_block_by_number(params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    let number = parse_block_number(params.get(0), state)?;
    let full_tx = params.get(1).and_then(Value::as_bool).unwrap_or(false);
    match state.db.get_block_by_number(number) {
        Ok(Some(b)) => Ok(block_to_json(&b, full_tx)),
        Ok(None) => Ok(Value::Null),
        Err(e) => Err(RpcError::Internal(format!("storage: {e}"))),
    }
}

fn eth_get_block_by_hash(params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    let hash_s = params
        .get(0)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("missing hash".into()))?;
    let hash = parse_h256(hash_s)?;
    let full_tx = params.get(1).and_then(Value::as_bool).unwrap_or(false);
    match state.db.get_block_by_hash(&hash) {
        Ok(Some(b)) => Ok(block_to_json(&b, full_tx)),
        Ok(None) => Ok(Value::Null),
        Err(e) => Err(RpcError::Internal(format!("storage: {e}"))),
    }
}

fn eth_get_block_tx_count_by_number(params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    let number = parse_block_number(params.get(0), state)?;
    match state.db.get_block_by_number(number) {
        Ok(Some(b)) => Ok(json!(format!("0x{:x}", b.body.transactions.len()))),
        _ => Ok(Value::Null),
    }
}

fn eth_get_block_tx_count_by_hash(params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    let hash_s = params
        .get(0)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("missing hash".into()))?;
    let hash = parse_h256(hash_s)?;
    match state.db.get_block_by_hash(&hash) {
        Ok(Some(b)) => Ok(json!(format!("0x{:x}", b.body.transactions.len()))),
        _ => Ok(Value::Null),
    }
}

fn eth_get_transaction_by_hash(params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    let hash_s = params
        .get(0)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("missing hash".into()))?;
    let hash = parse_h256(hash_s)?;
    match state.db.get_transaction(&hash) {
        Ok(Some(tx)) => Ok(tx_to_json(&tx)),
        Ok(None) => Ok(Value::Null),
        Err(e) => Err(RpcError::Internal(format!("storage: {e}"))),
    }
}

fn eth_get_transaction_receipt(params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    let hash_s = params
        .get(0)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("missing hash".into()))?;
    let hash = parse_h256(hash_s)?;
    match state.db.get_receipt(&hash) {
        Ok(Some(r)) => Ok(json!({
            "transactionHash":   format!("0x{}", hex::encode(r.transaction_hash.as_bytes())),
            "transactionIndex":  format!("0x{:x}", r.transaction_index),
            "blockHash":         format!("0x{}", hex::encode(r.block_hash.as_bytes())),
            "blockNumber":       format!("0x{:x}", r.block_number),
            "from":              format!("0x{}", hex::encode(r.from.as_bytes())),
            "to":                r.to.as_ref().map(|a| format!("0x{}", hex::encode(a.as_bytes()))),
            "gasUsed":           format!("0x{:x}", r.gas_used),
            "cumulativeGasUsed": format!("0x{:x}", r.cumulative_gas_used),
            "status":            format!("0x{:x}", if r.is_success() { 1u8 } else { 0u8 }),
            // RPC-01 fix (2026-05-16): previous code hardcoded `"logs": []` which
            // made every contract event invisible to wallets and dApps querying
            // eth_getTransactionReceipt. Now serialise r.logs per the Ethereum
            // JSON-RPC spec (EIP-658 fields + Ethereum log object shape).
            "logs": r.logs.iter().enumerate().map(|(i, log)| {
                let topics: Vec<String> = log.topics.iter()
                    .map(|t| format!("0x{}", hex::encode(t.as_bytes())))
                    .collect();
                json!({
                    "removed":          false,
                    "logIndex":         format!("0x{:x}", i),
                    "transactionIndex": format!("0x{:x}", r.transaction_index),
                    "transactionHash":  format!("0x{}", hex::encode(r.transaction_hash.as_bytes())),
                    "blockHash":        format!("0x{}", hex::encode(r.block_hash.as_bytes())),
                    "blockNumber":      format!("0x{:x}", r.block_number),
                    "address":          format!("0x{}", hex::encode(log.address.as_bytes())),
                    "data":             format!("0x{}", hex::encode(&log.data)),
                    "topics":           topics,
                })
            }).collect::<Vec<_>>(),
            "logsBloom":         format!("0x{}", hex::encode(&r.logs_bloom[..])),
            "contractAddress":   r.contract_address.as_ref().map(|a| format!("0x{}", hex::encode(a.as_bytes()))),
            "type":              "0x2",
            "effectiveGasPrice": format!("0x{:x}", r.effective_gas_price),
        })),
        Ok(None) => Ok(Value::Null),
        Err(e) => Err(RpcError::Internal(format!("storage: {e}"))),
    }
}

fn txpool_status(state: &RpcState) -> Result<Value, RpcError> {
    let pool = state.mempool.read();
    Ok(json!({
        "pending": format!("0x{:x}", pool.pending_count()),
        "queued":  format!("0x{:x}", pool.queued_count()),
    }))
}

fn web3_sha3(params: &Value) -> Result<Value, RpcError> {
    let raw = params
        .get(0)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("missing input".into()))?;
    let stripped = raw.strip_prefix("0x").unwrap_or(raw);
    let bytes = hex::decode(stripped)
        .map_err(|e| RpcError::InvalidParams(format!("hex: {e}")))?;
    let mut h = Keccak256::new();
    h.update(&bytes);
    let out: [u8; 32] = h.finalize().into();
    Ok(json!(format!("0x{}", hex::encode(out))))
}

// ---------------------------------------------------------------------------
// Encoding helpers
// ---------------------------------------------------------------------------

fn block_to_json(b: &zbx_types::block::Block, full_tx: bool) -> Value {
    let h = &b.header;
    let txs: Vec<Value> = if full_tx {
        b.body.transactions.iter().map(tx_to_json).collect()
    } else {
        b.body
            .transactions
            .iter()
            .map(|t| json!(format!("0x{}", hex::encode(t.hash.as_bytes()))))
            .collect()
    };
    json!({
        "number":           format!("0x{:x}", h.number),
        "hash":             format!("0x{}", hex::encode(b.hash().as_bytes())),
        "parentHash":       format!("0x{}", hex::encode(h.parent_hash.as_bytes())),
        "sha3Uncles":       format!("0x{}", hex::encode(h.uncle_hash.as_bytes())),
        "miner":            format!("0x{}", hex::encode(h.coinbase.as_bytes())),
        "stateRoot":        format!("0x{}", hex::encode(h.state_root.as_bytes())),
        "transactionsRoot": format!("0x{}", hex::encode(h.transactions_root.as_bytes())),
        "receiptsRoot":     format!("0x{}", hex::encode(h.receipts_root.as_bytes())),
        "logsBloom":        format!("0x{}", hex::encode(&h.logs_bloom[..])),
        "difficulty":       u256_to_hex(&h.difficulty),
        "totalDifficulty":  u256_to_hex(&h.difficulty),
        "size":             format!("0x{:x}", 1024u64),
        "gasLimit":         format!("0x{:x}", h.gas_limit),
        "gasUsed":          format!("0x{:x}", h.gas_used),
        "timestamp":        format!("0x{:x}", h.timestamp),
        "extraData":        format!("0x{}", hex::encode(&h.extra_data)),
        "mixHash":          format!("0x{}", hex::encode(h.mix_hash.as_bytes())),
        "nonce":            format!("0x{:016x}", h.nonce),
        "baseFeePerGas":    format!("0x{:x}", h.base_fee_per_gas),
        "transactions":     txs,
        "uncles":           [],
    })
}

fn tx_to_json(t: &zbx_types::transaction::SignedTransaction) -> Value {
    json!({
        "hash":   format!("0x{}", hex::encode(t.hash.as_bytes())),
        "from":   format!("0x{}", hex::encode(t.from.as_bytes())),
        "to":     t.tx.to.as_ref().map(|a| format!("0x{}", hex::encode(a.as_bytes()))),
        "value":  u256_to_hex(&t.tx.value),
        "nonce":  format!("0x{:x}", t.tx.nonce),
        "gas":    format!("0x{:x}", t.tx.gas_limit),
        "gasPrice":             format!("0x{:x}", t.tx.max_fee_per_gas),
        "maxFeePerGas":         format!("0x{:x}", t.tx.max_fee_per_gas),
        "maxPriorityFeePerGas": format!("0x{:x}", t.tx.max_priority_fee_per_gas),
        "input":  format!("0x{}", hex::encode(&t.tx.data)),
        "chainId": format!("0x{:x}", t.tx.chain_id),
        "type":   format!("0x{:x}", t.tx.tx_type as u8),
    })
}
