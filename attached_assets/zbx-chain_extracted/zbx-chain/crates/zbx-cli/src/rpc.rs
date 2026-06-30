//! JSON-RPC 2.0 client helpers — wires zbxctl subcommands to the live chain.
//!
//! M53-04 FIX: Previously every CLI write subcommand bailed with `not_yet_wired`.
//! This module provides:
//!   - `json_rpc_call`: generic JSON-RPC 2.0 POST helper
//!   - ABI encoding helpers (selector, encode_address, encode_uint128)
//!   - `eth_get_nonce` / `eth_base_fee`
//!   - `build_and_send_tx`: full EIP-1559 sign + broadcast (no external deps)
//!   - `eth_call_raw`: read-only `eth_call` helper

use anyhow::Context as _;
use reqwest::Client;
use serde_json::{json, Value};
use sha3::{Digest, Keccak256};
use zbx_crypto::Signature;
use zbx_keystore::KeystoreWallet;
use zbx_types::H256;

// ── Generic JSON-RPC call ─────────────────────────────────────────────────────

/// Call any JSON-RPC 2.0 method. Returns the `result` field on success,
/// or an error describing the JSON-RPC error object.
pub async fn json_rpc_call(
    rpc_url: &str,
    method:  &str,
    params:  Value,
) -> anyhow::Result<Value> {
    let body = json!({
        "jsonrpc": "2.0",
        "id":      1,
        "method":  method,
        "params":  params,
    });
    let resp: Value = Client::new()
        .post(rpc_url)
        .json(&body)
        .send().await
        .with_context(|| format!("POST {rpc_url} ({method})"))?
        .json().await
        .context("RPC response body is not valid JSON")?;

    if let Some(err) = resp.get("error") {
        anyhow::bail!("JSON-RPC error ({method}): {err}");
    }
    Ok(resp["result"].clone())
}

// ── ABI encoding helpers ──────────────────────────────────────────────────────

/// 4-byte Keccak-256 function selector.
/// e.g. `selector("transfer(address,uint256)")` → `[0xa9, 0x05, 0x9c, 0xbb]`
pub fn selector(sig: &str) -> [u8; 4] {
    let h = Keccak256::digest(sig.as_bytes());
    [h[0], h[1], h[2], h[3]]
}

/// ABI-encode a 20-byte Ethereum address as a 32-byte ABI word (left-padded).
pub fn encode_address(addr: &[u8; 20]) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[12..].copy_from_slice(addr);
    w
}

/// ABI-encode a u128 as a 32-byte ABI word (big-endian, left-padded).
pub fn encode_uint128(v: u128) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[16..].copy_from_slice(&v.to_be_bytes());
    w
}

/// ABI-encode a u64 as a 32-byte ABI word (big-endian, left-padded).
pub fn encode_uint64(v: u64) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[24..].copy_from_slice(&v.to_be_bytes());
    w
}

/// ABI-encode a u8 as a 32-byte ABI word (big-endian, left-padded).
pub fn encode_uint8(v: u8) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[31] = v;
    w
}

// ── eth_call (read-only) ──────────────────────────────────────────────────────

/// Perform an `eth_call` and return the raw result bytes.
pub async fn eth_call_raw(
    rpc_url: &str,
    to:      &[u8; 20],
    data:    &[u8],
) -> anyhow::Result<Vec<u8>> {
    let to_hex   = format!("0x{}", hex::encode(to));
    let data_hex = format!("0x{}", hex::encode(data));
    let result = json_rpc_call(rpc_url, "eth_call", json!([
        { "to": to_hex, "data": data_hex },
        "latest"
    ])).await?;
    let hex_str = result.as_str()
        .ok_or_else(|| anyhow::anyhow!("eth_call result is not a string: {result}"))?;
    hex::decode(hex_str.trim_start_matches("0x"))
        .context("eth_call result is not valid hex")
}

// ── Nonce / gas price helpers ─────────────────────────────────────────────────

/// `eth_getTransactionCount` — current nonce for an address.
pub async fn eth_get_nonce(rpc_url: &str, addr: &[u8; 20]) -> anyhow::Result<u64> {
    let addr_hex = format!("0x{}", hex::encode(addr));
    let result = json_rpc_call(rpc_url, "eth_getTransactionCount",
        json!([addr_hex, "latest"])).await?;
    let hex_str = result.as_str()
        .ok_or_else(|| anyhow::anyhow!("eth_getTransactionCount result is not a string"))?;
    u64::from_str_radix(hex_str.trim_start_matches("0x"), 16)
        .context("parse nonce")
}

/// `eth_getBlockByNumber` → `baseFeePerGas` of the latest block.
pub async fn eth_base_fee(rpc_url: &str) -> anyhow::Result<u128> {
    let block = json_rpc_call(rpc_url, "eth_getBlockByNumber",
        json!(["latest", false])).await?;
    let bf = block["baseFeePerGas"].as_str()
        .ok_or_else(|| anyhow::anyhow!("no baseFeePerGas in latest block header"))?;
    u128::from_str_radix(bf.trim_start_matches("0x"), 16)
        .context("parse baseFeePerGas")
}

// ── EIP-1559 transaction builder + broadcaster ────────────────────────────────

/// Build, sign (EIP-1559 type-2), and broadcast a transaction.
///
/// Returns the `0x…` transaction hash returned by the node.
///
/// M53-04 FIX: This is the missing link that previously caused all write
/// subcommands to bail with `not_yet_wired`.
///
/// Fee policy: maxPriorityFeePerGas = 1.5 gwei; maxFeePerGas = 2× baseFee + priority.
/// `gas` defaults to `300_000` if `0` is passed (safe default for most calls).
pub async fn build_and_send_tx(
    rpc_url:  &str,
    wallet:   &KeystoreWallet,
    chain_id: u64,
    to:       Option<&[u8; 20]>,   // None = contract deployment
    calldata: &[u8],
    value:    u128,
    gas:      u64,
) -> anyhow::Result<String> {
    let from = wallet.address();
    let nonce    = eth_get_nonce(rpc_url, from).await?;
    let base_fee = eth_base_fee(rpc_url).await.unwrap_or(1_000_000_000);
    let gas_limit  = if gas == 0 { 300_000u64 } else { gas };
    let priority: u128 = 1_500_000_000;          // 1.5 gwei
    let max_fee:  u128 = base_fee * 2 + priority; // EIP-1559 max fee

    let to_bytes: Vec<u8> = to.map(|a| a.to_vec()).unwrap_or_default();

    // Build and hash the unsigned EIP-1559 transaction.
    let unsigned = eip1559_unsigned_rlp(
        chain_id, nonce, priority, max_fee, gas_limit, &to_bytes, value, calldata,
    );
    let hash = Keccak256::digest(&unsigned);
    let msg_hash = H256::from_slice(&hash);

    // Sign with the keystore wallet (RFC 6979 deterministic, low-S).
    let sig: Signature = wallet.sign(&msg_hash)
        .map_err(|e| anyhow::anyhow!("signing failed: {e}"))?;
    // Signature layout: v=recovery_id(0/1), r=H256, s=H256
    let r = sig.r.as_bytes();
    let s = sig.s.as_bytes();
    let y_parity = sig.v; // 0 or 1 — EIP-1559 yParity

    let signed = eip1559_signed_rlp(
        chain_id, nonce, priority, max_fee, gas_limit, &to_bytes, value, calldata,
        y_parity, r, s,
    );

    let raw_hex = format!("0x{}", hex::encode(&signed));
    let result  = json_rpc_call(rpc_url, "eth_sendRawTransaction", json!([raw_hex])).await?;
    let tx_hash = result.as_str()
        .ok_or_else(|| anyhow::anyhow!("eth_sendRawTransaction returned non-string: {result}"))?
        .to_string();
    Ok(tx_hash)
}

// ── Minimal RLP encoder (EIP-2718 type-2 transactions only) ──────────────────

fn rlp_byte_string(b: &[u8]) -> Vec<u8> {
    if b.len() == 1 && b[0] < 0x80 {
        return b.to_vec();
    }
    let mut out = Vec::with_capacity(1 + b.len());
    rlp_push_length(&mut out, b.len(), 0x80);
    out.extend_from_slice(b);
    out
}

fn rlp_uint(v: u64) -> Vec<u8> {
    if v == 0 { return vec![0x80]; }
    let b = v.to_be_bytes();
    let s = b.iter().position(|&x| x != 0).unwrap_or(7);
    rlp_byte_string(&b[s..])
}

fn rlp_uint128(v: u128) -> Vec<u8> {
    if v == 0 { return vec![0x80]; }
    let b = v.to_be_bytes();
    let s = b.iter().position(|&x| x != 0).unwrap_or(15);
    rlp_byte_string(&b[s..])
}

fn rlp_list(items: &[Vec<u8>]) -> Vec<u8> {
    let payload: Vec<u8> = items.iter().flat_map(|i| i.iter().copied()).collect();
    let mut out = Vec::with_capacity(1 + payload.len());
    rlp_push_length(&mut out, payload.len(), 0xC0);
    out.extend_from_slice(&payload);
    out
}

fn rlp_push_length(out: &mut Vec<u8>, len: usize, base: u8) {
    if len <= 55 {
        out.push(base + len as u8);
    } else {
        let lb = {
            let b = len.to_be_bytes();
            let s = b.iter().position(|&x| x != 0).unwrap_or(7);
            b[s..].to_vec()
        };
        out.push(base + 55 + lb.len() as u8);
        out.extend_from_slice(&lb);
    }
}

/// RLP-encode unsigned EIP-1559 transaction for signing.
/// Returns: `0x02 || RLP([chainId, nonce, maxPriority, maxFee, gasLimit, to, value, data, []])`
fn eip1559_unsigned_rlp(
    chain_id: u64, nonce: u64, max_priority: u128, max_fee: u128,
    gas: u64, to: &[u8], value: u128, data: &[u8],
) -> Vec<u8> {
    let items = vec![
        rlp_uint(chain_id),
        rlp_uint(nonce),
        rlp_uint128(max_priority),
        rlp_uint128(max_fee),
        rlp_uint(gas),
        rlp_byte_string(to),
        rlp_uint128(value),
        rlp_byte_string(data),
        rlp_list(&[]),  // empty accessList
    ];
    let mut out = vec![0x02u8];
    out.extend_from_slice(&rlp_list(&items));
    out
}

/// RLP-encode signed EIP-1559 transaction for broadcasting.
/// Returns: `0x02 || RLP([chainId, nonce, maxPriority, maxFee, gasLimit, to, value, data, [], yParity, r, s])`
fn eip1559_signed_rlp(
    chain_id: u64, nonce: u64, max_priority: u128, max_fee: u128,
    gas: u64, to: &[u8], value: u128, data: &[u8],
    y_parity: u8, r: &[u8], s: &[u8],
) -> Vec<u8> {
    let items = vec![
        rlp_uint(chain_id),
        rlp_uint(nonce),
        rlp_uint128(max_priority),
        rlp_uint128(max_fee),
        rlp_uint(gas),
        rlp_byte_string(to),
        rlp_uint128(value),
        rlp_byte_string(data),
        rlp_list(&[]),                   // empty accessList
        rlp_uint(y_parity as u64),       // yParity
        rlp_byte_string(r),
        rlp_byte_string(s),
    ];
    let mut out = vec![0x02u8];
    out.extend_from_slice(&rlp_list(&items));
    out
}
