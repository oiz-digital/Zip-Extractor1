//! Transaction validation logic.

use crate::{SignedTx, TxError};

// ── TX-VAL-01: Calldata size cap ─────────────────────────────────────────────

/// Maximum calldata size per transaction (128 KiB).
///
/// The EVM itself does not impose a per-tx calldata limit beyond the block gas
/// limit, but very large calldata (a) consumes disproportionate gas, and (b)
/// enables resource-exhaustion attacks on the mempool / P2P layer before gas
/// accounting kicks in.  128 KiB is consistent with the ERC-4337 bundler cap
/// and is well above any realistic ABI-encoded call.
///
/// Note: `zbx-bundler` already enforces this cap at the UserOperation level
/// (BundlerError::CalldataTooLarge).  This check extends the same protection
/// to plain transactions entering the base mempool.
pub const MAX_TX_CALLDATA_SIZE: usize = 128 * 1024;

// ── TX-VAL-02: EIP-3860 initcode cap ─────────────────────────────────────────

/// Maximum initcode size for contract creation transactions (EIP-3860).
///
/// EIP-3860 (Shanghai, block 17034870 on mainnet) caps initcode at
/// `2 × MAX_CODE_SIZE = 2 × 24,576 = 49,152 bytes`.  Without this limit,
/// a large-initcode tx can trigger O(initcode²) gas cost in the CREATE handler
/// while only paying O(initcode) in intrinsic gas — a quadratic DoS vector.
///
/// This limit is enforced at validation time (before the tx enters the pool)
/// so that the EVM execution engine never receives an oversized initcode.
pub const MAX_INITCODE_SIZE: usize = 2 * 24_576;

// ── Validator ─────────────────────────────────────────────────────────────────

/// Validates transactions before pool admission.
pub struct TxValidator {
    pub chain_id:      u64,
    pub max_gas_limit: u64,
    pub min_gas_price: u128,
}

impl TxValidator {
    pub fn new(chain_id: u64) -> Self {
        Self { chain_id, max_gas_limit: 30_000_000, min_gas_price: 1_000_000_000 }
    }

    pub fn validate(&self, tx: &SignedTx) -> Result<(), TxError> {
        // 1. Chain ID must match.
        if tx.tx.chain_id != self.chain_id {
            return Err(TxError::WrongChainId {
                expected: self.chain_id, got: tx.tx.chain_id,
            });
        }
        // 2. Gas limit must be at least the EVM intrinsic minimum.
        if tx.tx.gas_limit < 21_000 {
            return Err(TxError::GasLimitTooLow {
                min: 21_000, got: tx.tx.gas_limit,
            });
        }
        // Gas limit must be within block limit.
        if tx.tx.gas_limit > self.max_gas_limit {
            return Err(TxError::GasLimitTooHigh {
                limit: self.max_gas_limit, got: tx.tx.gas_limit,
            });
        }
        // 3. Max fee must be >= min gas price.
        if tx.tx.max_fee_per_gas < self.min_gas_price {
            return Err(TxError::FeeTooLow {
                min: self.min_gas_price, got: tx.tx.max_fee_per_gas,
            });
        }
        // 4. Priority fee must be <= max fee.
        if tx.tx.max_priority_fee > tx.tx.max_fee_per_gas {
            return Err(TxError::PriorityFeeExceedsMaxFee);
        }
        // 5. Signature (from must be recovered and must not be the zero address).
        //    TX-02 FIX (MEDIUM): a pathological ECDSA input can produce a valid
        //    recovery that resolves to the all-zero address 0x000…000. Accepting
        //    such a transaction lets an attacker drain the protocol-reserved
        //    zero address or bypass sender-address checks in Solidity contracts
        //    that use `address(0)` as a sentinel. Reject it here so it never
        //    reaches the mempool.
        match tx.from {
            None => return Err(TxError::InvalidSignature),
            Some(addr) if addr == [0u8; 20] => return Err(TxError::InvalidSignature),
            _ => {}
        }

        // 6. TX-VAL-01: Calldata size cap (128 KiB).
        //    Applies to both call and contract-creation transactions.
        if tx.tx.data.len() > MAX_TX_CALLDATA_SIZE {
            return Err(TxError::CalldataTooLarge {
                max: MAX_TX_CALLDATA_SIZE,
                got: tx.tx.data.len(),
            });
        }

        // 7. TX-VAL-02 (EIP-3860): Initcode size cap for CREATE transactions.
        //    `to == None` signals contract creation; the `data` field is the initcode.
        if tx.tx.to.is_none() && tx.tx.data.len() > MAX_INITCODE_SIZE {
            return Err(TxError::InitcodeTooLarge {
                max: MAX_INITCODE_SIZE,
                got: tx.tx.data.len(),
            });
        }

        Ok(())
    }
}