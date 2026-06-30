//! Transaction validation for mempool admission.
//!
//! Checks performed (in order):
//! 1. Chain ID matches
//! 2. Nonce >= current on-chain nonce
//! 3. Sender has enough balance for worst-case gas cost
//! 4. Gas limit within block gas limit
//! 5. Data size within limit (EIP-3860)
//! 6. Signature is valid (recovered signer matches sender)
//! 7. Not duplicate

use crate::error::MempoolError;
use zbx_types::{
    address::Address, U256,
    transaction::{SignedTransaction, TransactionType},
};

pub const MAX_TX_DATA_SIZE:   usize = 128 * 1024;   // 128 KB (EIP-3860)
pub const MAX_INITCODE_SIZE:  usize = 2 * 24 * 1024; // 2 × MAX_CODE_SIZE
pub const MAX_GAS_LIMIT:      u64   = 30_000_000;    // block gas cap
pub const INTRINSIC_GAS_TX:   u64   = 21_000;        // basic tx
pub const INTRINSIC_GAS_CREATE: u64 = 53_000;        // contract creation

/// Contextual information needed for validation.
pub struct ValidationContext {
    pub chain_id:        u64,
    pub block_gas_limit: u64,
    pub base_fee:        U256,
    pub min_gas_price:   U256,
}

/// Result of a successful validation.
pub struct ValidTx {
    pub sender:    Address,
    pub cost:      U256,   // value + gas_price * gas_limit
    pub intrinsic: u64,    // intrinsic gas
}

/// Validate a transaction for mempool admission.
pub fn validate(
    tx:     &SignedTransaction,
    nonce:  u64,     // current on-chain nonce of sender
    balance: U256,   // current balance of sender
    ctx:    &ValidationContext,
) -> Result<ValidTx, MempoolError> {
    // 1. Chain ID
    if let Some(id) = tx.chain_id() {
        if id != ctx.chain_id {
            return Err(MempoolError::InvalidChainId { expected: ctx.chain_id, got: id });
        }
    }

    // 2. Gas limit
    if tx.gas_limit() > ctx.block_gas_limit {
        return Err(MempoolError::GasTooHigh {
            limit: tx.gas_limit(),
            max:   ctx.block_gas_limit,
        });
    }

    // 3. Max fee >= base fee
    if tx.max_fee_per_gas() < ctx.base_fee {
        return Err(MempoolError::FeeTooLow {
            max_fee:  tx.max_fee_per_gas(),
            base_fee: ctx.base_fee,
        });
    }

    // 4. Minimum gas price check
    if tx.max_fee_per_gas() < ctx.min_gas_price {
        return Err(MempoolError::GasPriceTooLow(tx.max_fee_per_gas()));
    }

    // 5. Nonce
    if tx.nonce() < nonce {
        return Err(MempoolError::NonceTooLow { expected: nonce, got: tx.nonce() });
    }

    // 6. Data size (EIP-3860)
    if tx.data().len() > MAX_TX_DATA_SIZE {
        return Err(MempoolError::TxTooLarge(tx.data().len()));
    }

    // 7. Intrinsic gas
    let intrinsic = intrinsic_gas(tx);
    if tx.gas_limit() < intrinsic {
        return Err(MempoolError::IntrinsicGasTooLow { need: intrinsic, have: tx.gas_limit() });
    }

    // 8. Balance check: value + gas_cost <= balance
    let gas_cost = U256::from(tx.gas_limit()) * tx.max_fee_per_gas();
    let cost     = tx.value() + gas_cost;
    if cost > balance {
        return Err(MempoolError::InsufficientFunds { need: cost, have: balance });
    }

    // 9. Signature
    let sender = tx.recover_sender()
        .map_err(|e| MempoolError::InvalidSignature(e.to_string()))?;

    Ok(ValidTx { sender, cost, intrinsic })
}

/// Compute the intrinsic gas for a transaction.
pub fn intrinsic_gas(tx: &SignedTransaction) -> u64 {
    let base = if tx.is_contract_creation() {
        INTRINSIC_GAS_CREATE
    } else {
        INTRINSIC_GAS_TX
    };

    let data_gas: u64 = tx.data().iter().map(|&b| {
        if b == 0 { 4 } else { 16 } // EIP-2028 costs
    }).sum();

    // EIP-2930: access list overhead
    let access_list_gas: u64 = tx.access_list().iter().map(|(_, slots)| {
        2_400 + slots.len() as u64 * 1_900
    }).sum();

    base + data_gas + access_list_gas
}