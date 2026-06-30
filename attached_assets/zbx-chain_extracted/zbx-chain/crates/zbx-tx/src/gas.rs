//! Multi-token gas fee system for ZBX Chain.
//!
//! ZBX Chain supports two native gas tokens — ZBX and ZUSD.
//! Both are first-class protocol tokens: any transaction can specify
//! `gas_token` to pay fees in whichever native token the sender holds.
//!
//! # Fee deduction model
//!
//! ```text
//! Gas fee = gas_used × effective_gas_price
//!
//! GasToken::Zbx  → deduct from Account.balance (EVM standard)
//! GasToken::Zusd → deduct from sender's ZUSD contract balance
//! ```
//!
//! # Genesis addresses
//!
//! ZUSD is deployed at a well-known genesis address derived from
//! the ZBX mainnet chain ID (0x231D = 8989):
//!
//! | Contract | Address |
//! |----------|---------|
//! | ZUSD | `0x00000000000000000000000000000000231D0001` |
//!
//! # Priority fee routing
//!
//! The priority (tip) portion is transferred to the block producer
//! in the same gas token the sender chose.
//! The base fee portion is burned (or redirected to treasury, per governance).

use crate::types::GasToken;

// ── Genesis contract addresses ────────────────────────────────────────────────

/// Genesis address of the ZUSD stablecoin contract.
/// Derived from ZBX mainnet chain ID: 0x231D = 8989.
pub const ZUSD_GENESIS_ADDR: [u8; 20] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0x23, 0x1D, 0x00, 0x01,
];

// ── Fee computation ───────────────────────────────────────────────────────────

/// Pre-execution gas fee reservation info.
///
/// The runtime reserves `max_cost` before executing the transaction, then
/// refunds `max_cost - actual_cost` after execution completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GasFeeInfo {
    /// Which token pays gas.
    pub token: GasToken,
    /// Maximum fee = gas_limit × max_fee_per_gas (in token base units).
    pub max_cost: u128,
    /// Actual fee = gas_used × effective_price (filled in post-execution).
    pub actual_cost: u128,
    /// Tip = gas_used × priority_fee (paid to block producer).
    pub tip: u128,
}

impl GasFeeInfo {
    /// Compute pre-execution reservation (worst-case cost).
    ///
    /// # Arguments
    /// * `gas_limit` — transaction gas limit
    /// * `max_fee_per_gas` — max price sender will pay per gas unit
    /// * `token` — which native token to use
    pub fn reserve(gas_limit: u64, max_fee_per_gas: u128, token: GasToken) -> Self {
        let max_cost = (gas_limit as u128).saturating_mul(max_fee_per_gas);
        Self { token, max_cost, actual_cost: 0, tip: 0 }
    }

    /// Compute final costs after execution.
    ///
    /// # Arguments
    /// * `gas_used` — actual gas consumed by the transaction
    /// * `base_fee` — block base fee per gas
    /// * `max_fee_per_gas` — from transaction
    /// * `max_priority_fee` — from transaction (tip cap)
    pub fn finalize(
        &mut self,
        gas_used:         u64,
        base_fee:         u128,
        max_fee_per_gas:  u128,
        max_priority_fee: u128,
    ) {
        let effective_price = base_fee
            .saturating_add(max_priority_fee)
            .min(max_fee_per_gas);
        let priority = effective_price.saturating_sub(base_fee);

        self.actual_cost = (gas_used as u128).saturating_mul(effective_price);
        self.tip         = (gas_used as u128).saturating_mul(priority);
    }

    /// Amount to refund to sender after execution (max_cost - actual_cost).
    pub fn refund(&self) -> u128 {
        self.max_cost.saturating_sub(self.actual_cost)
    }
}

/// How the fee should be deducted from the sender's balance.
///
/// The execution engine uses this to apply the correct deduction:
/// native ZBX from `Account.balance`, or a token contract balance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeeDeduction {
    /// Deduct from native ZBX balance (EVM-standard path).
    NativeZbx {
        /// Total to deduct upfront (reservation).
        amount: u128,
    },
    /// Call token contract's internal `deduct_gas(sender, amount)` hook.
    ContractToken {
        /// Genesis contract address (ZUSD).
        contract: [u8; 20],
        /// Amount in token base units (18 decimals for ZUSD).
        amount: u128,
        /// Which token this is.
        token: GasToken,
    },
}

impl FeeDeduction {
    /// Build the correct deduction descriptor for a transaction.
    pub fn for_gas_fee(info: &GasFeeInfo) -> Self {
        match info.token {
            GasToken::Zbx  => Self::NativeZbx  { amount: info.max_cost },
            GasToken::Zusd => Self::ContractToken {
                contract: ZUSD_GENESIS_ADDR,
                amount:   info.max_cost,
                token:    GasToken::Zusd,
            },
        }
    }

    /// Contract address for the gas token (None if native ZBX).
    pub fn contract_addr(&self) -> Option<[u8; 20]> {
        match self {
            Self::NativeZbx { .. }              => None,
            Self::ContractToken { contract, .. } => Some(*contract),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserve_computes_max_cost() {
        let info = GasFeeInfo::reserve(21_000, 1_000_000_000, GasToken::Zbx);
        assert_eq!(info.max_cost, 21_000 * 1_000_000_000u128);
        assert_eq!(info.token, GasToken::Zbx);
    }

    #[test]
    fn finalize_computes_tip_and_refund() {
        let mut info = GasFeeInfo::reserve(21_000, 20_000_000_000, GasToken::Zusd);
        info.finalize(20_000, 10_000_000_000, 20_000_000_000, 2_000_000_000);
        let effective = 12_000_000_000u128;
        assert_eq!(info.actual_cost, 20_000 * effective);
        assert_eq!(info.tip,         20_000 * 2_000_000_000u128);
        let max_cost = 21_000 * 20_000_000_000u128;
        assert_eq!(info.refund(), max_cost - info.actual_cost);
    }

    #[test]
    fn fee_deduction_zbx_is_native() {
        let info = GasFeeInfo::reserve(21_000, 1_000_000_000, GasToken::Zbx);
        let ded = FeeDeduction::for_gas_fee(&info);
        assert!(matches!(ded, FeeDeduction::NativeZbx { .. }));
        assert_eq!(ded.contract_addr(), None);
    }

    #[test]
    fn fee_deduction_zusd_uses_genesis_addr() {
        let info = GasFeeInfo::reserve(21_000, 1_000_000_000, GasToken::Zusd);
        let ded = FeeDeduction::for_gas_fee(&info);
        assert!(matches!(ded, FeeDeduction::ContractToken { token: GasToken::Zusd, .. }));
        assert_eq!(ded.contract_addr(), Some(ZUSD_GENESIS_ADDR));
    }

    #[test]
    fn gas_token_symbols() {
        assert_eq!(GasToken::Zbx.symbol(),  "ZBX");
        assert_eq!(GasToken::Zusd.symbol(), "ZUSD");
    }

    #[test]
    fn gas_token_from_byte() {
        assert_eq!(GasToken::from_byte(0), Some(GasToken::Zbx));
        assert_eq!(GasToken::from_byte(1), Some(GasToken::Zusd));
        assert_eq!(GasToken::from_byte(2), None);
    }

    #[test]
    fn zusd_addr_last_bytes() {
        assert_eq!(&ZUSD_GENESIS_ADDR[16..], &[0x23, 0x1D, 0x00, 0x01]);
        assert_eq!(&ZUSD_GENESIS_ADDR[..16], &[0u8; 16]);
    }
}
