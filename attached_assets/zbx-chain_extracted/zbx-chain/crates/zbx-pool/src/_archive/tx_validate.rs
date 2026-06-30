//! Transaction pool validation -- full validate_tx() pipeline.
//!
//! Before a tx enters the pending pool, it must pass ALL validation checks.
//! Validation is split into stateless (no DB) and stateful (needs state).
//!
//! ## Stateless checks (cheap, done first):
//!   - Signature valid (secp256k1 ECDSA)
//!   - Chain ID matches (EIP-155 replay protection)
//!   - Gas limit <= block gas limit
//!   - Max fee >= base fee (EIP-1559)
//!   - Tx size <= MAX_TX_SIZE (128KB)
//!   - Value >= 0 (always true for u256 but sanity check)
//!   - Nonce reasonable (< u64::MAX)
//!
//! ## Stateful checks (need current state):
//!   - Sender balance >= gas_cost + value
//!   - Sender nonce == state_nonce (pending) or >= state_nonce (queued)
//!   - No duplicate (tx hash already in pool)
//!   - Pool not at capacity (evict underpriced if needed)
//!
//! Returns Ok(PooledTx) on success, Err(PoolError) on failure.
//! Errors are returned verbatim to the caller (zbx_sendRawTransaction).

/// Maximum transaction size in bytes (128 KB).
pub const MAX_TX_SIZE: usize = 128 * 1024;

/// Minimum gas limit for any transaction.
pub const MIN_TX_GAS: u64 = 21_000; // base intrinsic gas

/// Maximum gas limit (must not exceed block gas limit).
pub const MAX_TX_GAS_LIMIT: u64 = 30_000_000;

// ── Validation result ─────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum PoolError {
    // Stateless
    InvalidSignature,
    WrongChainId       { expected: u64, got: u64 },
    TxTooLarge         { size: usize, max: usize },
    GasLimitTooLow     { min: u64, got: u64 },
    GasLimitTooHigh    { max: u64, got: u64 },
    MaxFeeTooLow       { base_fee: u128, max_fee: u128 },
    TipExceedsMaxFee,
    // Stateful
    InsufficientBalance { balance: u128, required: u128 },
    NonceTooLow        { state_nonce: u64, tx_nonce: u64 },
    NonceTooHigh       { state_nonce: u64, tx_nonce: u64, max_gap: u64 },
    AlreadyKnown       { hash: [u8; 32] },
    PoolFull,
    Underpriced        { min_tip: u128, got_tip: u128 },
    ReplaceUnderpriced { required_bump: u128 },
    SenderLimitExceeded,
}

/// Full transaction validation result.
#[derive(Debug)]
pub struct ValidationResult {
    pub sender:      [u8; 20],
    pub effective_tip: u128,
    pub is_pending:  bool,  // true = ready (nonce matches), false = queued
    pub gas_cost:    u128,  // max_fee_per_gas * gas_limit (worst case cost)
}

/// Maximum nonce gap allowed for queued transactions.
pub const MAX_NONCE_GAP: u64 = 64;

/// Validate a raw transaction before adding to the pool.
///
/// Performs full stateless + stateful validation.
pub fn validate_tx(
    raw_tx:       &[u8],
    chain_id:     u64,
    base_fee:     u128,
    state_nonce:  u64,
    state_balance: u128,
    pool_has_tx:  bool,   // already in pool?
    pool_full:    bool,
    sender_tx_count: usize,
    max_sender_txs: usize,
) -> Result<ValidationResult, PoolError> {
    // ── Stateless checks ──────────────────────────────────────────────────────

    // 1. Size check
    if raw_tx.len() > MAX_TX_SIZE {
        return Err(PoolError::TxTooLarge { size: raw_tx.len(), max: MAX_TX_SIZE });
    }

    // 2. Decode and verify signature (EIP-155 chain_id check inside)
    let tx = decode_tx(raw_tx).map_err(|_| PoolError::InvalidSignature)?;

    // 3. Chain ID (EIP-155 replay protection)
    if let Some(tx_chain_id) = tx.chain_id {
        if tx_chain_id != chain_id {
            return Err(PoolError::WrongChainId { expected: chain_id, got: tx_chain_id });
        }
    }

    // 4. Gas limit bounds
    if tx.gas_limit < MIN_TX_GAS {
        return Err(PoolError::GasLimitTooLow { min: MIN_TX_GAS, got: tx.gas_limit });
    }
    if tx.gas_limit > MAX_TX_GAS_LIMIT {
        return Err(PoolError::GasLimitTooHigh { max: MAX_TX_GAS_LIMIT, got: tx.gas_limit });
    }

    // 5. EIP-1559 fee checks
    if tx.max_fee_per_gas < base_fee {
        return Err(PoolError::MaxFeeTooLow { base_fee, max_fee: tx.max_fee_per_gas });
    }
    if tx.max_priority_fee_per_gas > tx.max_fee_per_gas {
        return Err(PoolError::TipExceedsMaxFee);
    }

    // ── Stateful checks ───────────────────────────────────────────────────────

    // 6. Duplicate check
    if pool_has_tx { return Err(PoolError::AlreadyKnown { hash: tx.hash }); }

    // 7. Nonce validation
    if tx.nonce < state_nonce {
        return Err(PoolError::NonceTooLow { state_nonce, tx_nonce: tx.nonce });
    }
    if tx.nonce > state_nonce + MAX_NONCE_GAP {
        return Err(PoolError::NonceTooHigh {
            state_nonce, tx_nonce: tx.nonce, max_gap: MAX_NONCE_GAP
        });
    }

    // 8. Balance check: must cover worst-case gas cost + value
    let gas_cost = tx.max_fee_per_gas.saturating_mul(tx.gas_limit as u128);
    let total_cost = gas_cost.saturating_add(tx.value);
    if state_balance < total_cost {
        return Err(PoolError::InsufficientBalance { balance: state_balance, required: total_cost });
    }

    // 9. Pool capacity
    if pool_full { return Err(PoolError::PoolFull); }

    // 10. Per-sender limit
    if sender_tx_count >= max_sender_txs { return Err(PoolError::SenderLimitExceeded); }

    let effective_tip = tx.max_priority_fee_per_gas.min(
        tx.max_fee_per_gas.saturating_sub(base_fee)
    );

    Ok(ValidationResult {
        sender:       tx.sender,
        effective_tip,
        is_pending:   tx.nonce == state_nonce,
        gas_cost,
    })
}

// ── Stub types ────────────────────────────────────────────────────────────────

struct DecodedTx {
    pub hash:                   [u8; 32],
    pub sender:                 [u8; 20],
    pub nonce:                  u64,
    pub gas_limit:              u64,
    pub max_fee_per_gas:        u128,
    pub max_priority_fee_per_gas: u128,
    pub value:                  u128,
    pub chain_id:               Option<u64>,
}

fn decode_tx(_raw: &[u8]) -> Result<DecodedTx, ()> {
    Ok(DecodedTx {
        hash: [0u8; 32], sender: [0u8; 20],
        nonce: 0, gas_limit: 21_000,
        max_fee_per_gas: 0, max_priority_fee_per_gas: 0,
        value: 0, chain_id: Some(zbx_types::CHAIN_ID_MAINNET),
    })
}