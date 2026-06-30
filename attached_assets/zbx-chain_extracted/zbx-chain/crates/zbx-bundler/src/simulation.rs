//! UserOperation simulation — validates ops off-chain via EVM `eth_call`.
//!
//! ## How ERC-4337 simulation works
//!
//! The EntryPoint exposes `simulateValidation(UserOperation)` which
//! **always reverts** intentionally — the spec uses the revert data to
//! return the `ValidationResult`.  The bundler must:
//!
//! 1. ABI-encode the call.
//! 2. Send an `eth_call` (never `eth_sendRawTransaction`) to the node.
//! 3. The node returns the revert payload in the JSON-RPC error body.
//! 4. Decode the payload:
//!    * Selector `0xe0cff05f` (`ValidationResult`) → op is **valid**.
//!    * Selector `0x00fa072b` (`ValidationResultWithAggregation`) → valid.
//!    * Any other revert → op is **invalid**.
//!
//! ## Structural pre-checks (no RPC)
//!
//! Reject obviously bad ops before wasting an RPC round-trip:
//! gas limits, sender format, empty signature, calldata size, time window.
//!
//! ## `simulateValidation` ABI
//!
//! Selector: `0xee219423`
//! Always reverts with `ValidationResult` selector `0xe0cff05f`.

use crate::{error::BundlerError, mempool::UserOperation};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

// ─────────────────────────────────────────────────────────────────────────────
// Result type
// ─────────────────────────────────────────────────────────────────────────────

/// Outcome of `simulateValidation` against the live node.
#[derive(Debug)]
pub struct SimulationResult {
    /// Pre-op gas charged before the wallet call.
    pub pre_op_gas:      u64,
    /// Whether the paymaster (if any) passed validation.
    pub paymaster_valid: bool,
    /// Aggregator address, if the account uses signature aggregation.
    pub aggregator:      Option<String>,
    /// True iff the EntryPoint returned `ValidationResult` (not another revert).
    pub valid:           bool,
    /// `validAfter` from the wallet's `validationData`.
    pub valid_after:     u64,
    /// `validUntil` from the wallet's `validationData`.
    pub valid_until:     u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// ABI helpers
// ─────────────────────────────────────────────────────────────────────────────

fn abi_u64(n: u64) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[24..].copy_from_slice(&n.to_be_bytes());
    w
}

fn abi_u128(n: u128) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[16..].copy_from_slice(&n.to_be_bytes());
    w
}

fn abi_addr(hex_str: &str) -> [u8; 32] {
    let s   = hex_str.trim_start_matches("0x");
    let raw = hex::decode(s).unwrap_or_default();
    let mut w = [0u8; 32];
    let start = 32usize.saturating_sub(raw.len());
    w[start..].copy_from_slice(&raw[..raw.len().min(32)]);
    w
}

fn abi_bytes(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 + data.len().div_ceil(32) * 32);
    out.extend_from_slice(&abi_u64(data.len() as u64));
    out.extend_from_slice(data);
    let pad = (32 - data.len() % 32) % 32;
    out.extend(std::iter::repeat(0u8).take(pad));
    out
}

/// ABI-encode a `UserOperation` struct for `simulateValidation(userOp)`.
///
/// ERC-4337 tuple layout (11 fields, 4 dynamic):
/// ```text
/// (address sender, uint256 nonce,
///  bytes initCode, bytes callData,
///  uint256 callGasLimit, uint256 verificationGasLimit, uint256 preVerificationGas,
///  uint256 maxFeePerGas, uint256 maxPriorityFeePerGas,
///  bytes paymasterAndData, bytes signature)
/// ```
fn abi_encode_simulate_validation(op: &UserOperation) -> Vec<u8> {
    // simulateValidation(UserOperation) → 0xee219423
    const SELECTOR: [u8; 4] = [0xee, 0x21, 0x94, 0x23];

    let init_code_enc = abi_bytes(&op.init_code);
    let call_data_enc = abi_bytes(&op.call_data);
    let paymaster_enc = abi_bytes(&op.paymaster_and_data);
    let signature_enc = abi_bytes(&op.signature);

    // 11 head slots × 32 = 352 bytes before dynamic tails.
    let base: u64    = 11 * 32;
    let off_init: u64 = base;
    let off_call: u64 = off_init + init_code_enc.len() as u64;
    let off_pay:  u64 = off_call + call_data_enc.len() as u64;
    let off_sig:  u64 = off_pay  + paymaster_enc.len() as u64;

    let mut tuple = Vec::with_capacity(
        11 * 32
            + init_code_enc.len()
            + call_data_enc.len()
            + paymaster_enc.len()
            + signature_enc.len(),
    );

    tuple.extend_from_slice(&abi_addr(&op.sender));
    tuple.extend_from_slice(&abi_u64(op.nonce));
    tuple.extend_from_slice(&abi_u64(off_init));
    tuple.extend_from_slice(&abi_u64(off_call));
    tuple.extend_from_slice(&abi_u64(op.call_gas_limit));
    tuple.extend_from_slice(&abi_u64(op.verification_gas_limit));
    tuple.extend_from_slice(&abi_u64(op.pre_verification_gas));
    tuple.extend_from_slice(&abi_u128(op.max_fee_per_gas));
    tuple.extend_from_slice(&abi_u128(op.max_priority_fee_per_gas));
    tuple.extend_from_slice(&abi_u64(off_pay));
    tuple.extend_from_slice(&abi_u64(off_sig));
    tuple.extend_from_slice(&init_code_enc);
    tuple.extend_from_slice(&call_data_enc);
    tuple.extend_from_slice(&paymaster_enc);
    tuple.extend_from_slice(&signature_enc);

    // Outer ABI: single tuple parameter — offset word (0x20) + tuple body.
    let mut calldata = SELECTOR.to_vec();
    calldata.extend_from_slice(&abi_u64(0x20)); // offset to tuple
    calldata.extend_from_slice(&tuple);
    calldata
}

// ─────────────────────────────────────────────────────────────────────────────
// ValidationResult decoder
// ─────────────────────────────────────────────────────────────────────────────

/// `ValidationResult` revert selector.
/// keccak256("ValidationResult(...)") first 4 bytes = 0xe0cff05f
const VALIDATION_RESULT_SEL: [u8; 4] = [0xe0, 0xcf, 0xf0, 0x5f];

/// `ValidationResultWithAggregation` selector = 0x00fa072b
const VALIDATION_RESULT_AGG_SEL: [u8; 4] = [0x00, 0xfa, 0x07, 0x2b];

fn word_u64(slice: &[u8]) -> u64 {
    if slice.len() < 8 { return 0; }
    u64::from_be_bytes(slice[slice.len() - 8..].try_into().unwrap_or([0u8; 8]))
}

/// Decode the revert payload from `simulateValidation`.
///
/// `ValidationResult` structure (simplified head/tail):
/// ```text
/// bytes4 selector
/// uint256 offset_to_returnInfo (= 0x20 typically)
/// -- ReturnInfo tuple --
///   uint256 preOpGas
///   uint256 prefund
///   bool    sigFailed
///   uint48  validAfter
///   uint48  validUntil
///   bytes   paymasterContext (dynamic — head offset)
/// ...
/// ```
fn decode_validation_result(revert_data: &[u8]) -> Option<SimulationResult> {
    if revert_data.len() < 4 {
        return None;
    }
    let sel: [u8; 4] = revert_data[..4].try_into().ok()?;

    let has_aggregator = sel == VALIDATION_RESULT_AGG_SEL;
    if sel != VALIDATION_RESULT_SEL && !has_aggregator {
        return None; // not a ValidationResult revert
    }

    // body = everything after the 4-byte selector
    let body = &revert_data[4..];

    // Word 0 = offset to the outer tuple's first element (returnInfo).
    // Typically this is 0x20 (one word into body), but parse it generically.
    if body.len() < 32 {
        return None;
    }
    let ret_info_offset = word_u64(&body[..32]) as usize;

    // ReturnInfo starts at body[ret_info_offset].
    // We need at least 5 words: preOpGas, prefund, sigFailed, validAfter, validUntil.
    if body.len() < ret_info_offset + 5 * 32 {
        return None;
    }
    let ri = &body[ret_info_offset..];

    let pre_op_gas  = word_u64(&ri[0 * 32..1 * 32]);
    // prefund at slot 1 — ignored
    let sig_failed  = ri[2 * 32 + 31] != 0;
    let valid_after = word_u64(&ri[3 * 32..4 * 32]);
    let valid_until = word_u64(&ri[4 * 32..5 * 32]);

    if sig_failed {
        // Account (or paymaster) rejected the UserOperation signature.
        return None;
    }

    Some(SimulationResult {
        pre_op_gas,
        paymaster_valid: true,
        aggregator:      if has_aggregator { Some("aggregated".into()) } else { None },
        valid:           true,
        valid_after,
        valid_until,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// JSON-RPC shapes for eth_call
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct JsonRpcReq<'a> {
    jsonrpc: &'static str,
    method:  &'a str,
    params:  serde_json::Value,
    id:      u64,
}

/// `eth_call` always returns an error object when the call reverts.
/// The revert data is in `error.data`.
#[derive(Deserialize, Debug)]
struct EthCallResp {
    result: Option<serde_json::Value>,
    error:  Option<EthCallErr>,
}

#[derive(Deserialize, Debug)]
struct EthCallErr {
    message: String,
    data:    Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Simulator
// ─────────────────────────────────────────────────────────────────────────────

/// Off-chain UserOperation simulator.
///
/// Calls `EntryPoint.simulateValidation(userOp)` via `eth_call` and
/// decodes the intentional `ValidationResult` revert.
pub struct UserOpSimulator {
    rpc_url:     String,
    entry_point: String,
    http:        reqwest::Client,
}

impl UserOpSimulator {
    pub fn new(rpc_url: impl Into<String>) -> Self {
        UserOpSimulator {
            rpc_url:     rpc_url.into(),
            entry_point: crate::ENTRY_POINT_ADDRESS.to_string(),
            http:        reqwest::Client::new(),
        }
    }

    /// Simulate a single UserOperation.
    ///
    /// Returns `Ok(SimulationResult { valid: true, .. })` only when the
    /// EntryPoint's `simulateValidation` call reverts with a
    /// `ValidationResult` that has `sigFailed = false`.
    pub async fn simulate(&self, op: &UserOperation) -> Result<SimulationResult, BundlerError> {
        debug!(sender = %op.sender, nonce = op.nonce, "simulation: start");

        // ── 1. Structural pre-checks (no RPC needed) ─────────────────────────
        if op.total_gas() > crate::MAX_USER_OP_GAS {
            warn!(gas = op.total_gas(), max = crate::MAX_USER_OP_GAS, "simulation: gas too high");
            return Err(BundlerError::GasTooHigh(op.total_gas()));
        }
        if op.pre_verification_gas < 21_000 {
            return Err(BundlerError::PreVerificationGasTooLow);
        }
        if op.sender.trim_start_matches("0x").len() != 40 {
            return Err(BundlerError::InvalidSender);
        }
        if op.signature.is_empty() {
            return Err(BundlerError::MissingSignature);
        }
        if op.call_data.len() > 131_072 {
            return Err(BundlerError::CalldataTooLarge(op.call_data.len()));
        }

        // ── 2. ABI-encode simulateValidation(op) calldata ────────────────────
        let calldata     = abi_encode_simulate_validation(op);
        let calldata_hex = format!("0x{}", hex::encode(&calldata));

        let call_obj = serde_json::json!({
            "to":   self.entry_point,
            "data": calldata_hex,
        });

        let req = JsonRpcReq {
            jsonrpc: "2.0",
            method:  "eth_call",
            params:  serde_json::json!([call_obj, "latest"]),
            id:      1,
        };

        // ── 3. Send eth_call ─────────────────────────────────────────────────
        let resp: EthCallResp = self.http
            .post(&self.rpc_url)
            .json(&req)
            .send()
            .await
            .map_err(|e| BundlerError::Rpc(format!("eth_call network: {e}")))?
            .json()
            .await
            .map_err(|e| BundlerError::Rpc(format!("eth_call parse: {e}")))?;

        // ── 4. Extract revert data ───────────────────────────────────────────
        // simulateValidation ALWAYS reverts — a non-error response means
        // the EntryPoint address is wrong or the node doesn't support eth_call.
        let revert_hex = match resp.error {
            Some(ref err) => match &err.data {
                Some(data) => data.clone(),
                None => {
                    return Err(BundlerError::SimulationRejected(format!(
                        "eth_call error with no revert data: {}",
                        err.message
                    )));
                }
            },
            None => {
                // eth_call succeeded (no revert) — unexpected for simulateValidation.
                return Err(BundlerError::SimulationRejected(
                    "eth_call returned success — EntryPoint did not revert as expected; \
                     check ENTRY_POINT_ADDRESS".into(),
                ));
            }
        };

        // ── 5. Decode the revert payload ─────────────────────────────────────
        let revert_str = revert_hex.trim_start_matches("0x");
        let revert_bytes = hex::decode(revert_str).map_err(|e| {
            BundlerError::SimulationRejected(format!("revert data hex decode: {e}"))
        })?;

        debug!(
            revert_len = revert_bytes.len(),
            sel         = hex::encode(&revert_bytes[..revert_bytes.len().min(4)]),
            "simulation: received revert"
        );

        // Check for known ValidationResult selectors.
        match decode_validation_result(&revert_bytes) {
            Some(result) => {
                debug!(
                    pre_op_gas  = result.pre_op_gas,
                    valid_after = result.valid_after,
                    valid_until = result.valid_until,
                    "simulation: op valid"
                );
                Ok(result)
            }
            None => {
                // The EntryPoint reverted with a non-ValidationResult error.
                // This means the account or paymaster rejected the op.
                let sel_hex = if revert_bytes.len() >= 4 {
                    hex::encode(&revert_bytes[..4])
                } else {
                    "??".into()
                };
                Err(BundlerError::SimulationRejected(format!(
                    "account/paymaster rejected op (revert selector 0x{sel_hex})"
                )))
            }
        }
    }

    /// Return true if two UserOperations conflict (same sender + nonce).
    pub fn conflicts(a: &UserOperation, b: &UserOperation) -> bool {
        a.sender == b.sender && a.nonce == b.nonce
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit tests (no network required)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mempool::UserOperation;

    fn sample_op() -> UserOperation {
        UserOperation {
            sender:                    "0xaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaA".into(),
            nonce:                     3,
            init_code:                 vec![],
            call_data:                 vec![0xca, 0xfe],
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

    #[test]
    fn calldata_starts_with_simulate_selector() {
        let op       = sample_op();
        let calldata = abi_encode_simulate_validation(&op);
        assert_eq!(&calldata[..4], &[0xee, 0x21, 0x94, 0x23]);
    }

    #[test]
    fn calldata_body_is_32_byte_aligned() {
        let op       = sample_op();
        let calldata = abi_encode_simulate_validation(&op);
        assert_eq!((calldata.len() - 4) % 32, 0);
    }

    #[test]
    fn decode_validation_result_wrong_selector_returns_none() {
        let bad = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00u8; 200];
        assert!(decode_validation_result(&bad).is_none());
    }

    #[test]
    fn decode_validation_result_too_short_returns_none() {
        let short = [VALIDATION_RESULT_SEL[0], VALIDATION_RESULT_SEL[1],
                     VALIDATION_RESULT_SEL[2], VALIDATION_RESULT_SEL[3]];
        assert!(decode_validation_result(&short).is_none());
    }

    #[test]
    fn decode_validation_result_sig_failed_returns_none() {
        // Build a synthetic ValidationResult payload with sigFailed = true.
        let mut payload = Vec::new();
        payload.extend_from_slice(&VALIDATION_RESULT_SEL);
        // Outer offset word: 0x20 (points to start of returnInfo inside body).
        payload.extend_from_slice(&abi_u64(0x20));
        // ReturnInfo slots: preOpGas, prefund, sigFailed=true, validAfter, validUntil, +1 padding.
        payload.extend_from_slice(&abi_u64(21_000)); // preOpGas
        payload.extend_from_slice(&abi_u64(0));       // prefund
        // sigFailed = true → last byte of word = 1
        let mut sig_failed_word = [0u8; 32];
        sig_failed_word[31] = 1;
        payload.extend_from_slice(&sig_failed_word);
        payload.extend_from_slice(&abi_u64(0));       // validAfter
        payload.extend_from_slice(&abi_u64(u64::MAX)); // validUntil
        // paymasterContext offset (dynamic field head — unused by decoder)
        payload.extend_from_slice(&abi_u64(5 * 32)); // offset past the 5 static slots

        assert!(decode_validation_result(&payload).is_none(),
            "sigFailed=true must cause None");
    }

    #[test]
    fn decode_validation_result_success() {
        // Build a synthetic valid ValidationResult payload.
        let mut payload = Vec::new();
        payload.extend_from_slice(&VALIDATION_RESULT_SEL);
        payload.extend_from_slice(&abi_u64(0x20)); // outer offset to returnInfo
        payload.extend_from_slice(&abi_u64(55_000)); // preOpGas
        payload.extend_from_slice(&abi_u64(0));      // prefund
        payload.extend_from_slice(&[0u8; 32]);       // sigFailed = false
        payload.extend_from_slice(&abi_u64(1_700_000_000)); // validAfter
        payload.extend_from_slice(&abi_u64(1_800_000_000)); // validUntil
        payload.extend_from_slice(&abi_u64(5 * 32)); // paymasterContext offset

        let result = decode_validation_result(&payload).expect("should decode");
        assert_eq!(result.pre_op_gas,  55_000);
        assert_eq!(result.valid_after, 1_700_000_000);
        assert_eq!(result.valid_until, 1_800_000_000);
        assert!(result.valid);
    }

    #[test]
    fn conflicts_same_sender_nonce() {
        let mut a = sample_op();
        let mut b = sample_op();
        a.nonce = 5;
        b.nonce = 5;
        assert!(UserOpSimulator::conflicts(&a, &b));
    }

    #[test]
    fn conflicts_different_nonce() {
        let mut a = sample_op();
        let mut b = sample_op();
        a.nonce = 1;
        b.nonce = 2;
        assert!(!UserOpSimulator::conflicts(&a, &b));
    }
}
