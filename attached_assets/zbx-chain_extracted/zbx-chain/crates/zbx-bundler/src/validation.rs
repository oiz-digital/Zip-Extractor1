//! UserOperation validation rules.
//!
//! Enforces ERC-4337 validation constraints (storage access rules, etc.)
//! to prevent griefing attacks on the bundler mempool.

use crate::{mempool::UserOperation, error::BundlerError};

/// SEC-2026-05-09 Pass-15 (HIGH-R05 + Tier-2 paymaster-no-validUntil):
/// validate the (validAfter, validUntil) time window. Pre-fix the
/// bundler ignored both fields entirely — a UserOp signed at time T
/// with `validUntil = T+10` could sit in the mempool for hours and
/// be bundled long after expiry, defeating the wallet's freshness
/// guarantee. Returns `Ok(())` if currently valid (or if both fields
/// are 0 — backwards compat with v0.6 callers).
pub fn validate_user_op_time(op: &UserOperation, now_unix: u64) -> Result<(), BundlerError> {
    if !op.is_currently_valid(now_unix) {
        return Err(BundlerError::Expired {
            valid_after: op.valid_after,
            valid_until: op.valid_until,
            now: now_unix,
        });
    }
    Ok(())
}

/// Validate a UserOperation before simulation.
pub fn validate_user_op(op: &UserOperation) -> Result<(), BundlerError> {
    // Sender must be a valid address (20 bytes when decoded)
    if op.sender.trim_start_matches("0x").len() != 40 {
        return Err(BundlerError::InvalidSender);
    }

    // Signature must not be empty
    if op.signature.is_empty() {
        return Err(BundlerError::MissingSignature);
    }

    // Max allowed calldata size (128 KB)
    if op.call_data.len() > 131_072 {
        return Err(BundlerError::CalldataTooLarge(op.call_data.len()));
    }

    // Must have either initCode (new wallet) or non-empty callData
    if op.init_code.is_empty() && op.call_data.is_empty() {
        return Err(BundlerError::EmptyOperation);
    }

    // Gas limits must be reasonable
    if op.verification_gas_limit < 10_000 {
        return Err(BundlerError::VerificationGasTooLow);
    }
    if op.call_gas_limit == 0 && op.call_data.len() > 0 {
        return Err(BundlerError::CallGasZero);
    }

    Ok(())
}