//! AI Error Debugger — human-readable failed transaction diagnosis.
//!
//! Used by the explorer at `/tx/{hash}/debug`.

use serde::{Deserialize, Serialize};

/// Result of debugging a failed transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxDebugResult {
    pub tx_hash:      String,
    pub success:      bool,
    pub reason:       Option<String>,
    pub explanation:  String,
    pub suggestions:  Vec<String>,
    pub gas_used:     Option<u64>,
    pub gas_limit:    Option<u64>,
    pub revert_data:  Option<String>,
}

/// Known EVM revert signatures and their explanations.
static KNOWN_REVERTS: &[(&str, &str, &str)] = &[
    // (4-byte selector hex, human reason, suggested fix)
    ("08c379a0", "Custom revert: see message below", "Read the revert message for details."),
    ("4e487b71", "Panic: arithmetic overflow or out-of-bounds access",
     "Check array indices and arithmetic operations in the contract."),
    // Custom ZBX errors.
    ("unauthorized", "Not authorised to call this function",
     "Only the contract owner or authorised role can call this function."),
    ("insufficient", "Insufficient balance or allowance",
     "Check your balance and ensure you have approved the contract to spend your tokens."),
    ("cap exceeded", "Maximum supply cap exceeded",
     "The token has reached its maximum supply. No more can be minted."),
    ("paused", "Contract is paused",
     "The contract is temporarily paused. Contact the project team."),
    ("rate limit", "Rate limited",
     "Too many requests. Wait and retry."),
    ("slippage", "Slippage too high",
     "Increase your slippage tolerance or reduce the swap amount."),
];

/// Diagnose a failed transaction from its receipt and revert data.
pub fn diagnose(
    tx_hash:    String,
    status:     u8,          // 1 = success, 0 = failed
    gas_used:   u64,
    gas_limit:  u64,
    revert_data: Option<&[u8]>,
) -> TxDebugResult {
    if status == 1 {
        return TxDebugResult {
            tx_hash,
            success: true,
            reason: None,
            explanation: "✅ Transaction succeeded.".into(),
            suggestions: vec![],
            gas_used: Some(gas_used),
            gas_limit: Some(gas_limit),
            revert_data: None,
        };
    }

    // Check for out-of-gas (gas_used ≥ 99% of gas_limit).
    if gas_limit > 0 && gas_used * 100 / gas_limit >= 99 {
        return TxDebugResult {
            tx_hash,
            success: false,
            reason: Some("Out of gas".into()),
            explanation: format!(
                "The transaction used {gas_used} gas out of the {gas_limit} gas limit \
                 and ran out of gas before completing."
            ),
            suggestions: vec![
                format!("Increase gas limit to at least {}", gas_limit * 3 / 2),
                "Use eth_estimateGas before submitting to avoid this".into(),
            ],
            gas_used: Some(gas_used),
            gas_limit: Some(gas_limit),
            revert_data: None,
        };
    }

    // Decode revert data.
    let (reason, mut suggestions) = if let Some(data) = revert_data {
        decode_revert_data(data)
    } else {
        ("Unknown revert reason".into(), vec![])
    };

    if suggestions.is_empty() {
        suggestions = vec![
            "Check the contract's verified source code on the explorer".into(),
            "Replay the call with eth_call to capture the revert message".into(),
            "Ensure all input parameters are valid".into(),
        ];
    }

    TxDebugResult {
        tx_hash,
        success: false,
        reason: Some(reason.clone()),
        explanation: format!(
            "❌ Transaction reverted: {reason}\n\n\
             Gas used: {gas_used} / {gas_limit}"
        ),
        suggestions,
        gas_used: Some(gas_used),
        gas_limit: Some(gas_limit),
        revert_data: revert_data.map(hex::encode),
    }
}

/// Decode ABI-encoded revert data into a human string.
fn decode_revert_data(data: &[u8]) -> (String, Vec<String>) {
    if data.is_empty() {
        return ("Empty revert data (no reason provided)".into(), vec![]);
    }

    let hex_data: String = data.iter().map(|b| format!("{:02x}", b)).collect();

    // Check for Error(string) — selector 08c379a0.
    if hex_data.starts_with("08c379a0") && data.len() >= 68 {
        // ABI: selector(4) + offset(32) + length(32) + string data.
        if let Ok(msg) = decode_error_string(&data[4..]) {
            for (sig, reason, fix) in KNOWN_REVERTS {
                if msg.to_lowercase().contains(sig) {
                    return (reason.to_string(), vec![fix.to_string()]);
                }
            }
            return (
                format!("Contract revert: {}", msg),
                vec!["Check the contract documentation for this error message.".into()],
            );
        }
    }

    // Panic(uint256) — selector 4e487b71.
    if hex_data.starts_with("4e487b71") && data.len() >= 36 {
        let code = u32::from_be_bytes([data[32], data[33], data[34], data[35]]);
        let (panic_reason, fix) = decode_panic_code(code);
        return (
            format!("Panic({}): {}", code, panic_reason),
            vec![fix.into()],
        );
    }

    // Unknown error — try to match against known keywords in hex.
    for (sig, reason, fix) in KNOWN_REVERTS {
        if hex_data.contains(sig) {
            return (reason.to_string(), vec![fix.to_string()]);
        }
    }

    ("Unknown revert reason".into(), vec![])
}

/// Decode an ABI-encoded Error(string) payload.
fn decode_error_string(data: &[u8]) -> Result<String, ()> {
    // ABI string encoding: offset(32) + length(32) + data(length).
    if data.len() < 64 { return Err(()); }
    let length_bytes: [u8; 32] = data[32..64].try_into().map_err(|_| ())?;
    let length = u32::from_be_bytes([
        length_bytes[28], length_bytes[29], length_bytes[30], length_bytes[31],
    ]) as usize;
    if data.len() < 64 + length { return Err(()); }
    std::str::from_utf8(&data[64..64 + length])
        .map(|s| s.to_string())
        .map_err(|_| ())
}

/// Decode a Solidity panic code.
fn decode_panic_code(code: u32) -> (&'static str, &'static str) {
    match code {
        0x01 => ("Assert failed",                    "An assert() check failed — this is a bug in the contract."),
        0x11 => ("Arithmetic overflow/underflow",    "An arithmetic operation overflowed or underflowed."),
        0x12 => ("Division or modulo by zero",       "A division by zero occurred."),
        0x21 => ("Enum value out of range",          "An enum conversion received an out-of-range value."),
        0x22 => ("Incorrectly encoded storage",      "A storage byte array was incorrectly encoded."),
        0x31 => ("Pop on empty array",               "pop() was called on an empty array."),
        0x32 => ("Array index out of bounds",        "An array was accessed with an out-of-bounds index."),
        0x41 => ("Out of memory (large allocation)", "Too much memory was allocated."),
        0x51 => ("Call to zero-initialized variable","An uninitialised function pointer was called."),
        _    => ("Unknown panic code",               "Check the contract source for arithmetic or array operations."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_receipt_returns_ok() {
        let result = diagnose("0xabc".into(), 1, 21_000, 50_000, None);
        assert!(result.success);
        assert!(result.explanation.contains("succeeded"));
    }

    #[test]
    fn out_of_gas_detected() {
        let result = diagnose("0xabc".into(), 0, 100_000, 100_000, None);
        assert!(!result.success);
        let reason = result.reason.unwrap_or_default();
        assert_eq!(reason, "Out of gas");
        assert!(!result.suggestions.is_empty());
    }

    #[test]
    fn empty_revert_data_handled() {
        let result = diagnose("0xabc".into(), 0, 21_000, 100_000, Some(&[]));
        assert!(!result.success);
    }

    #[test]
    fn panic_code_11_decoded() {
        // Panic(0x11) = arithmetic overflow.
        let mut data = Vec::new();
        data.extend_from_slice(&[0x4e, 0x48, 0x7b, 0x71]); // selector
        data.extend_from_slice(&[0u8; 28]);
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x11u8]); // code = 17 (0x11)
        let result = diagnose("0xabc".into(), 0, 50_000, 100_000, Some(&data));
        assert!(!result.success);
        let reason = result.reason.unwrap_or_default();
        assert!(reason.contains("overflow") || reason.contains("Panic"), "got: {}", reason);
    }
}
