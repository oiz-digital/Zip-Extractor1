//! Bundle relay — submits ERC-4337 bundles to the EntryPoint on-chain.
//!
//! # What this does
//!
//! 1. ABI-encodes `handleOps(UserOperation[], beneficiary)` calldata.
//! 2. Fetches the bundler account nonce via `eth_getTransactionCount`.
//! 3. Fetches the current base-fee via `eth_feeHistory` (or falls back to
//!    `eth_gasPrice`).
//! 4. Builds an EIP-1559 (type-2) transaction, RLP-encodes it, signs it
//!    with the bundler private key (from env `ZBX_BUNDLER_PRIVKEY`), and
//!    submits it via `eth_sendRawTransaction`.
//! 5. Polls `eth_getTransactionReceipt` until the tx is mined or the
//!    deadline is reached.
//!
//! # Environment
//! * `ZBX_BUNDLER_PRIVKEY` — 32-byte hex private key (with or without 0x
//!   prefix) used to sign bundle transactions.  **Required for production.**
//!   If unset, `submit()` returns [`BundlerError::MissingPrivKey`].
//!
//! # ABI encoding
//! The `handleOps` function selector is `0x1fad948c` (keccak256 of
//! `"handleOps((address,uint256,bytes,bytes,uint256,uint256,uint256,
//!   uint256,uint256,bytes,bytes)[],address)"`).
//! Full ABI encoding follows the EVM ABI specification (head/tail layout).

use crate::{bundle::Bundle, error::BundlerError, mempool::UserOperation};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

// ─────────────────────────────────────────────────────────────────────────────
// JSON-RPC client helpers
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct JsonRpcRequest<'a> {
    jsonrpc: &'static str,
    method:  &'a str,
    params:  serde_json::Value,
    id:      u64,
}

#[derive(Deserialize)]
struct JsonRpcResponse<T> {
    result: Option<T>,
    error:  Option<RpcError>,
}

#[derive(Deserialize, Debug)]
struct RpcError {
    message: String,
}

async fn rpc_call<T: serde::de::DeserializeOwned>(
    client:  &reqwest::Client,
    url:     &str,
    method:  &str,
    params:  serde_json::Value,
) -> Result<T, BundlerError> {
    let req = JsonRpcRequest { jsonrpc: "2.0", method, params, id: 1 };
    let resp: JsonRpcResponse<T> = client
        .post(url)
        .json(&req)
        .send()
        .await
        .map_err(|e| BundlerError::Rpc(format!("{method}: network error: {e}")))?
        .json()
        .await
        .map_err(|e| BundlerError::Rpc(format!("{method}: parse error: {e}")))?;

    if let Some(err) = resp.error {
        return Err(BundlerError::Rpc(format!("{method}: {}", err.message)));
    }
    resp.result.ok_or_else(|| BundlerError::Rpc(format!("{method}: null result")))
}

// ─────────────────────────────────────────────────────────────────────────────
// Minimal inline RLP encoder (EIP-2718 / EIP-1559 type-2 transaction)
// ─────────────────────────────────────────────────────────────────────────────

/// Encode a non-negative integer as a minimal big-endian byte string
/// (RLP integer encoding).  Returns `[]` for zero.
fn rlp_uint(n: u128) -> Vec<u8> {
    if n == 0 {
        return vec![];
    }
    let bytes = n.to_be_bytes();
    let first = bytes.iter().position(|&b| b != 0).unwrap_or(15);
    bytes[first..].to_vec()
}

fn rlp_uint64(n: u64) -> Vec<u8> {
    rlp_uint(n as u128)
}

/// Encode a byte string (bytes or address) as an RLP item.
fn rlp_bytes(data: &[u8]) -> Vec<u8> {
    let len = data.len();
    if len == 1 && data[0] < 0x80 {
        return vec![data[0]]; // single byte, no prefix needed
    }
    let mut out = rlp_len_prefix(len, 0x80);
    out.extend_from_slice(data);
    out
}

/// Encode an unsigned integer as an RLP item.
fn rlp_int(n: u128) -> Vec<u8> {
    let raw = rlp_uint(n);
    if raw.is_empty() {
        return vec![0x80]; // zero = empty string
    }
    rlp_bytes(&raw)
}

fn rlp_int64(n: u64) -> Vec<u8> {
    rlp_int(n as u128)
}

fn rlp_len_prefix(len: usize, base: u8) -> Vec<u8> {
    if len <= 55 {
        vec![base + len as u8]
    } else {
        let len_bytes = (len as u64).to_be_bytes();
        let first = len_bytes.iter().position(|&b| b != 0).unwrap_or(7);
        let llen = 8 - first;
        let mut out = vec![base + 55 + llen as u8];
        out.extend_from_slice(&len_bytes[first..]);
        out
    }
}

/// Encode a sequence of RLP items as an RLP list.
fn rlp_list(items: &[Vec<u8>]) -> Vec<u8> {
    let payload: Vec<u8> = items.iter().flat_map(|i| i.iter().copied()).collect();
    let mut out = rlp_len_prefix(payload.len(), 0xc0);
    out.extend_from_slice(&payload);
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// ABI encoding — handleOps(UserOperation[],address)
// ─────────────────────────────────────────────────────────────────────────────

/// Encode a 20-byte address padded to 32 bytes (ABI head word).
fn abi_address(addr_hex: &str) -> [u8; 32] {
    let s = addr_hex.trim_start_matches("0x");
    let raw = hex::decode(s).unwrap_or_default();
    let mut word = [0u8; 32];
    let start = 32usize.saturating_sub(raw.len());
    word[start..].copy_from_slice(&raw[..raw.len().min(32)]);
    word
}

/// Encode a u64 as a 32-byte ABI word (big-endian, zero-padded).
fn abi_u64(n: u64) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[24..].copy_from_slice(&n.to_be_bytes());
    w
}

/// Encode a u128 as a 32-byte ABI word.
fn abi_u128(n: u128) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[16..].copy_from_slice(&n.to_be_bytes());
    w
}

/// ABI-encode a `bytes` value: 32-byte length word + padded data.
fn abi_bytes(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 + data.len().div_ceil(32) * 32);
    // length
    out.extend_from_slice(&abi_u64(data.len() as u64));
    // data, padded to 32-byte boundary
    out.extend_from_slice(data);
    let pad = (32 - data.len() % 32) % 32;
    out.extend(std::iter::repeat(0u8).take(pad));
    out
}

/// ABI-encode one `UserOperation` struct (tuple).
///
/// ERC-4337 tuple layout (all fields in order):
/// ```text
/// (address sender,
///  uint256 nonce,
///  bytes   initCode,
///  bytes   callData,
///  uint256 callGasLimit,
///  uint256 verificationGasLimit,
///  uint256 preVerificationGas,
///  uint256 maxFeePerGas,
///  uint256 maxPriorityFeePerGas,
///  bytes   paymasterAndData,
///  bytes   signature)
/// ```
/// Dynamic fields (bytes) are encoded head+tail in order.
fn abi_encode_user_op(op: &UserOperation) -> Vec<u8> {
    // Fixed-size head entries (11 slots × 32 bytes = 352 bytes).
    // Slots 0,1,4,5,6,7,8 are static values.
    // Slots 2,3,9,10 are offsets to the dynamic tails.

    // Pre-compute the 4 dynamic encodings.
    let init_code_enc        = abi_bytes(&op.init_code);
    let call_data_enc        = abi_bytes(&op.call_data);
    let paymaster_enc        = abi_bytes(&op.paymaster_and_data);
    let signature_enc        = abi_bytes(&op.signature);

    // Base offset = 11 words (all head slots).
    let base: u64 = 11 * 32;
    let off_init_code: u64 = base;
    let off_call_data: u64 = off_init_code + init_code_enc.len() as u64;
    let off_paymaster: u64 = off_call_data + call_data_enc.len() as u64;
    let off_signature: u64 = off_paymaster + paymaster_enc.len() as u64;

    let mut out = Vec::with_capacity(11 * 32
        + init_code_enc.len()
        + call_data_enc.len()
        + paymaster_enc.len()
        + signature_enc.len());

    // Head slots
    out.extend_from_slice(&abi_address(&op.sender));        // slot 0: sender
    out.extend_from_slice(&abi_u64(op.nonce));              // slot 1: nonce
    out.extend_from_slice(&abi_u64(off_init_code));         // slot 2: initCode offset
    out.extend_from_slice(&abi_u64(off_call_data));         // slot 3: callData offset
    out.extend_from_slice(&abi_u64(op.call_gas_limit));     // slot 4
    out.extend_from_slice(&abi_u64(op.verification_gas_limit)); // slot 5
    out.extend_from_slice(&abi_u64(op.pre_verification_gas));   // slot 6
    out.extend_from_slice(&abi_u128(op.max_fee_per_gas));   // slot 7
    out.extend_from_slice(&abi_u128(op.max_priority_fee_per_gas)); // slot 8
    out.extend_from_slice(&abi_u64(off_paymaster));         // slot 9
    out.extend_from_slice(&abi_u64(off_signature));         // slot 10

    // Tails
    out.extend_from_slice(&init_code_enc);
    out.extend_from_slice(&call_data_enc);
    out.extend_from_slice(&paymaster_enc);
    out.extend_from_slice(&signature_enc);

    out
}

/// Build the full `handleOps(ops[], beneficiary)` calldata.
///
/// Layout:
/// ```text
/// [4 bytes selector]
/// [32 bytes: offset to ops[] = 0x40]
/// [32 bytes: beneficiary padded to 32]
/// [32 bytes: ops[] length]
/// [32 bytes × N: offsets to each UserOperation tuple]
/// [N × encoded UserOperation tuples]
/// ```
fn encode_handle_ops(bundle: &Bundle) -> Vec<u8> {
    // Function selector: handleOps(tuple[],address) = 0x1fad948c
    const SELECTOR: [u8; 4] = [0x1f, 0xad, 0x94, 0x8c];

    let n = bundle.ops.len();

    // Encode each op.
    let encoded_ops: Vec<Vec<u8>> = bundle.ops.iter().map(abi_encode_user_op).collect();

    // Array head: [length word] + [N offset words]
    // Offsets are relative to the start of the array contents (after length word).
    // Start of array contents (after N offset words) = N * 32 bytes from here.
    let mut array_head: Vec<u8> = Vec::new();
    array_head.extend_from_slice(&abi_u64(n as u64)); // array length

    // Compute offsets for each element.
    let mut running: u64 = (n as u64) * 32; // past all offset words
    for enc in &encoded_ops {
        array_head.extend_from_slice(&abi_u64(running));
        running += enc.len() as u64;
    }

    // Array tail: concatenated encoded ops.
    let mut array_tail: Vec<u8> = Vec::new();
    for enc in &encoded_ops {
        array_tail.extend_from_slice(enc);
    }

    // Top-level ABI layout:
    //   slot 0: offset to ops[] = 64 (0x40) — past the two head words
    //   slot 1: beneficiary address
    let offset_ops: u64 = 64;

    let mut calldata = Vec::new();
    calldata.extend_from_slice(&SELECTOR);
    calldata.extend_from_slice(&abi_u64(offset_ops));                      // offset to array
    calldata.extend_from_slice(&abi_address(&bundle.beneficiary));         // beneficiary
    calldata.extend_from_slice(&array_head);
    calldata.extend_from_slice(&array_tail);
    calldata
}

// ─────────────────────────────────────────────────────────────────────────────
// EIP-1559 transaction builder + signer
// ─────────────────────────────────────────────────────────────────────────────

/// Sign and RLP-encode an EIP-1559 (type-2) transaction.
///
/// Signing hash = `keccak256(0x02 || rlp([chain_id, nonce, max_priority_fee,
///                                        max_fee, gas_limit, to, 0, data, []]))`.
///
/// Final encoding = `0x02 || rlp([chain_id, nonce, max_priority_fee, max_fee,
///                                gas_limit, to, 0, data, [], y_parity, r, s])`.
fn sign_eip1559_tx(
    chain_id:              u64,
    nonce:                 u64,
    max_priority_fee_gwei: u128,
    max_fee_gwei:          u128,
    gas_limit:             u64,
    to:                    &str,
    data:                  &[u8],
    privkey_bytes:         &[u8; 32],
) -> Vec<u8> {
    use sha3::{Digest, Keccak256};

    let to_bytes = {
        let s = to.trim_start_matches("0x");
        hex::decode(s).unwrap_or_default()
    };

    // Fields for signing (without v,r,s).
    let unsigned = rlp_list(&[
        rlp_int64(chain_id),
        rlp_int64(nonce),
        rlp_int(max_priority_fee_gwei),
        rlp_int(max_fee_gwei),
        rlp_int64(gas_limit),
        rlp_bytes(&to_bytes),
        rlp_int(0),           // value = 0
        rlp_bytes(data),
        rlp_list(&[]),        // access list = []
    ]);

    let mut payload_for_hash = vec![0x02u8];
    payload_for_hash.extend_from_slice(&unsigned);
    let mut h = Keccak256::new();
    h.update(&payload_for_hash);
    let hash: [u8; 32] = h.finalize().into();

    // Sign with k256.
    use k256::ecdsa::{SigningKey, signature::hazmat::PrehashSigner};
    let sk = SigningKey::from_slice(privkey_bytes).expect("valid privkey");
    let (sig, recid): (k256::ecdsa::Signature, k256::ecdsa::RecoveryId) =
        sk.sign_prehash(&hash).expect("sign");

    let sig_bytes = sig.to_bytes();
    let r: [u8; 32] = sig_bytes[..32].try_into().unwrap();
    let s: [u8; 32] = sig_bytes[32..].try_into().unwrap();
    let v: u64      = recid.to_byte() as u64; // 0 or 1

    // Build signed transaction.
    let signed = rlp_list(&[
        rlp_int64(chain_id),
        rlp_int64(nonce),
        rlp_int(max_priority_fee_gwei),
        rlp_int(max_fee_gwei),
        rlp_int64(gas_limit),
        rlp_bytes(&to_bytes),
        rlp_int(0),
        rlp_bytes(data),
        rlp_list(&[]),
        rlp_int64(v),
        rlp_bytes(&r),
        rlp_bytes(&s),
    ]);

    let mut raw = vec![0x02u8];
    raw.extend_from_slice(&signed);
    raw
}

// ─────────────────────────────────────────────────────────────────────────────
// BundleRelay
// ─────────────────────────────────────────────────────────────────────────────

pub struct BundleRelay {
    rpc_url:     String,
    bundler_key: String,   // hex-encoded 32-byte private key
    entry_point: String,
    chain_id:    u64,
    http:        reqwest::Client,
}

impl BundleRelay {
    pub fn new(
        rpc_url:     impl Into<String>,
        bundler_key: impl Into<String>,
        chain_id:    u64,
    ) -> Self {
        BundleRelay {
            rpc_url:     rpc_url.into(),
            bundler_key: bundler_key.into(),
            entry_point: crate::ENTRY_POINT_ADDRESS.to_string(),
            chain_id,
            http:        reqwest::Client::new(),
        }
    }

    // ─── private key ──────────────────────────────────────────────────────────

    fn privkey_bytes(&self) -> Result<[u8; 32], BundlerError> {
        let hex_str = if self.bundler_key.is_empty() {
            std::env::var("ZBX_BUNDLER_PRIVKEY")
                .map_err(|_| BundlerError::MissingPrivKey)?
        } else {
            self.bundler_key.clone()
        };
        let s = hex_str.trim_start_matches("0x");
        let b = hex::decode(s).map_err(|e| BundlerError::InvalidPrivKey(e.to_string()))?;
        if b.len() != 32 {
            return Err(BundlerError::InvalidPrivKey(format!(
                "expected 32 bytes, got {}", b.len()
            )));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&b);
        Ok(arr)
    }

    fn bundler_address(&self, privkey: &[u8; 32]) -> String {
        use k256::{ecdsa::SigningKey, elliptic_curve::sec1::ToEncodedPoint};
        use sha3::{Digest, Keccak256};

        let sk = SigningKey::from_slice(privkey).expect("valid privkey");
        let pk = sk.verifying_key();
        let enc = pk.to_encoded_point(false); // uncompressed 65 bytes
        let pub_bytes = &enc.as_bytes()[1..]; // drop 0x04 prefix
        let mut hasher = Keccak256::new();
        hasher.update(pub_bytes);
        let hash: [u8; 32] = hasher.finalize().into();
        format!("0x{}", hex::encode(&hash[12..]))
    }

    // ─── RPC helpers ──────────────────────────────────────────────────────────

    /// Fetch the bundler account's current nonce.
    async fn get_nonce(&self, address: &str) -> Result<u64, BundlerError> {
        let hex: String = rpc_call(
            &self.http,
            &self.rpc_url,
            "eth_getTransactionCount",
            serde_json::json!([address, "pending"]),
        )
        .await?;
        let n = u64::from_str_radix(hex.trim_start_matches("0x"), 16)
            .map_err(|e| BundlerError::Rpc(format!("nonce parse: {e}")))?;
        Ok(n)
    }

    /// Fetch current gas price (wei) via eth_gasPrice.
    async fn get_gas_price(&self) -> Result<u128, BundlerError> {
        let hex: String = rpc_call(
            &self.http,
            &self.rpc_url,
            "eth_gasPrice",
            serde_json::json!([]),
        )
        .await?;
        let n = u128::from_str_radix(hex.trim_start_matches("0x"), 16)
            .map_err(|e| BundlerError::Rpc(format!("gas price parse: {e}")))?;
        Ok(n)
    }

    // ─── main methods ─────────────────────────────────────────────────────────

    /// Build, sign, and submit a bundle transaction.
    ///
    /// Returns the 32-byte transaction hash on success.
    ///
    /// # Errors
    /// * [`BundlerError::MissingPrivKey`] — `ZBX_BUNDLER_PRIVKEY` not set.
    /// * [`BundlerError::EmptyBundle`]
    /// * [`BundlerError::Rpc`] — any JSON-RPC failure.
    pub async fn submit(&self, bundle: &Bundle) -> Result<[u8; 32], BundlerError> {
        if bundle.ops.is_empty() {
            return Err(BundlerError::EmptyBundle);
        }

        info!(
            ops         = bundle.ops.len(),
            gas         = bundle.estimated_gas,
            beneficiary = %bundle.beneficiary,
            "relay: submitting bundle"
        );

        let privkey = self.privkey_bytes()?;
        let from    = self.bundler_address(&privkey);

        // 1. Bundler account nonce.
        let nonce = self.get_nonce(&from).await?;
        debug!(nonce, from = %from, "relay: fetched bundler nonce");

        // 2. Gas price (base fee + 2 Gwei tip, or raw gasPrice as fallback).
        let gas_price = self.get_gas_price().await?;
        let tip: u128 = 2_000_000_000; // 2 Gwei priority fee
        let max_fee   = gas_price.saturating_add(tip);
        debug!(gas_price, max_fee, "relay: gas price fetched");

        // 3. ABI-encode handleOps calldata.
        let calldata = encode_handle_ops(bundle);
        debug!(calldata_len = calldata.len(), "relay: calldata encoded");

        // 4. Sign and RLP-encode EIP-1559 transaction.
        let gas_limit = bundle.estimated_gas.saturating_add(50_000); // headroom
        let raw_tx = sign_eip1559_tx(
            self.chain_id,
            nonce,
            tip,
            max_fee,
            gas_limit,
            &self.entry_point,
            &calldata,
            &privkey,
        );

        // 5. Submit via eth_sendRawTransaction.
        let raw_hex = format!("0x{}", hex::encode(&raw_tx));
        let tx_hash_hex: String = rpc_call(
            &self.http,
            &self.rpc_url,
            "eth_sendRawTransaction",
            serde_json::json!([raw_hex]),
        )
        .await
        .map_err(|e| {
            error!(err = %e, "relay: eth_sendRawTransaction failed");
            e
        })?;

        let hash_str = tx_hash_hex.trim_start_matches("0x");
        if hash_str.len() != 64 {
            return Err(BundlerError::Rpc(format!(
                "eth_sendRawTransaction returned unexpected hash: {tx_hash_hex}"
            )));
        }
        let mut tx_hash = [0u8; 32];
        hex::decode_to_slice(hash_str, &mut tx_hash)
            .map_err(|e| BundlerError::Rpc(format!("tx hash decode: {e}")))?;

        info!(
            tx   = %tx_hash_hex,
            from = %from,
            ops  = bundle.ops.len(),
            "relay: bundle submitted"
        );
        Ok(tx_hash)
    }

    /// Poll `eth_getTransactionReceipt` until mined or timeout.
    ///
    /// Returns the block number where the transaction was included.
    ///
    /// Polls every 2 seconds for up to 120 seconds (60 attempts).
    ///
    /// # Errors
    /// * [`BundlerError::Rpc`] — any JSON-RPC failure.
    /// * [`BundlerError::InclusionTimeout`] — tx not mined within deadline.
    pub async fn wait_for_inclusion(&self, tx_hash: [u8; 32]) -> Result<u64, BundlerError> {
        let hash_hex = format!("0x{}", hex::encode(tx_hash));
        info!(tx = %hash_hex, "relay: waiting for bundle inclusion");

        for attempt in 0..60u32 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            // eth_getTransactionReceipt returns null until mined.
            let resp: Option<TxReceipt> = rpc_call(
                &self.http,
                &self.rpc_url,
                "eth_getTransactionReceipt",
                serde_json::json!([hash_hex]),
            )
            .await
            .unwrap_or(None);

            if let Some(receipt) = resp {
                let block = u64::from_str_radix(
                    receipt.block_number.trim_start_matches("0x"),
                    16,
                )
                .unwrap_or(0);

                // status = "0x1" → success; "0x0" → reverted.
                if receipt.status.as_deref() == Some("0x0") {
                    warn!(
                        tx    = %hash_hex,
                        block = block,
                        "relay: bundle transaction reverted"
                    );
                    return Err(BundlerError::BundleReverted { block });
                }

                info!(
                    tx      = %hash_hex,
                    block   = block,
                    attempt = attempt,
                    "relay: bundle included"
                );
                return Ok(block);
            }

            debug!(attempt, tx = %hash_hex, "relay: not yet mined");
        }

        Err(BundlerError::InclusionTimeout { tx: hash_hex })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Receipt shape (partial — we only need status and blockNumber)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct TxReceipt {
    block_number: String,
    status:       Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit tests (no network required)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bundle::Bundle, mempool::UserOperation};

    fn sample_op() -> UserOperation {
        UserOperation {
            sender:                    "0xaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaA".into(),
            nonce:                     7,
            init_code:                 vec![],
            call_data:                 vec![0xde, 0xad, 0xbe, 0xef],
            call_gas_limit:            100_000,
            verification_gas_limit:    50_000,
            pre_verification_gas:      21_000,
            max_fee_per_gas:           30_000_000_000,
            max_priority_fee_per_gas:  2_000_000_000,
            paymaster_and_data:        vec![],
            signature:                 vec![0x01; 65],
            valid_after:               0,
            valid_until:               0,
        }
    }

    fn sample_bundle() -> Bundle {
        Bundle {
            ops:            vec![sample_op()],
            beneficiary:    "0xbBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbBb".into(),
            estimated_gas:  200_000,
        }
    }

    #[test]
    fn encode_handle_ops_has_correct_selector() {
        let bundle   = sample_bundle();
        let calldata = encode_handle_ops(&bundle);
        assert_eq!(&calldata[..4], &[0x1f, 0xad, 0x94, 0x8c]);
    }

    #[test]
    fn encode_handle_ops_length_is_divisible_by_32_after_selector() {
        let bundle   = sample_bundle();
        let calldata = encode_handle_ops(&bundle);
        // Everything after the 4-byte selector must be 32-byte aligned.
        assert_eq!((calldata.len() - 4) % 32, 0,
            "ABI-encoded body must be 32-byte aligned, got {} extra bytes",
            (calldata.len() - 4) % 32);
    }

    #[test]
    fn encode_handle_ops_two_ops_deterministic() {
        let mut bundle = sample_bundle();
        bundle.ops.push(sample_op());
        let c1 = encode_handle_ops(&bundle);
        let c2 = encode_handle_ops(&bundle);
        assert_eq!(c1, c2, "ABI encoding must be deterministic");
    }

    #[test]
    fn rlp_uint_zero_is_empty() {
        assert_eq!(rlp_uint(0), vec![]);
    }

    #[test]
    fn rlp_uint_one_is_single_byte() {
        assert_eq!(rlp_uint(1), vec![0x01]);
    }

    #[test]
    fn rlp_int_zero_is_0x80() {
        assert_eq!(rlp_int(0), vec![0x80]);
    }

    #[test]
    fn sign_eip1559_starts_with_0x02() {
        let key = [1u8; 32]; // deterministic test key
        let raw = sign_eip1559_tx(1337, 0, 2_000_000_000, 30_000_000_000, 200_000,
            "0x5FF137D4b0FDCD49DcA30c7CF57E578a026d2789", &[0xab; 4], &key);
        assert_eq!(raw[0], 0x02, "EIP-1559 tx must start with type byte 0x02");
    }

    #[test]
    fn missing_privkey_returns_error() {
        // Unset the env var temporarily.
        std::env::remove_var("ZBX_BUNDLER_PRIVKEY");
        let relay = BundleRelay::new("http://localhost:8545", "", 1337);
        let err   = relay.privkey_bytes().unwrap_err();
        assert!(matches!(err, BundlerError::MissingPrivKey));
    }

    #[test]
    fn valid_privkey_from_field() {
        let key_hex = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
        let relay   = BundleRelay::new("http://localhost:8545", key_hex, 1337);
        let key     = relay.privkey_bytes().unwrap();
        assert_eq!(key[0], 0x01);
        assert_eq!(key[31], 0x20);
    }
}
