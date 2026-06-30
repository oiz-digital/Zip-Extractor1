//! zbx-lending — decentralised lending protocol.
//!
//! Supports collateralised loans, liquidations, interest-rate models,
//! and supply/borrow markets for ZBX and supported ERC-20 tokens.
//!
//! ## DeFi upgrade (Session 35)
//! * `flash_loan`    — EIP-3156 flash loans, 0.09% fee, reentrancy guard, 50% liquidity cap
//! * `vault`         — ERC-4626 yield vault, deposit/redeem/harvest, management fee
//! * `supply_borrow` — Full supply/borrow/repay/redeem engine, borrow index accrual,
//!                     health factor, borrow caps, 50% close factor liquidation

pub mod market;
pub mod collateral;
pub mod liquidation;
pub mod interest;
pub mod flash_loan;
pub mod vault;
pub mod supply_borrow;