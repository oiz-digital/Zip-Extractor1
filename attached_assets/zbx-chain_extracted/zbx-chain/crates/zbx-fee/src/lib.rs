//! zbx-fee — EIP-1559 fee market for Zebvix Chain.
//!
//! Implements:
//!   - Base-fee adjustment per block (target 50% gas usage).
//!   - Priority-fee (tip) handling.
//!   - `eth_gasPrice` / `eth_feeHistory` RPC support.
//!   - Gas-price oracle (weighted percentile of recent blocks).

pub mod base_fee;
pub mod error;
pub mod fee_history;
pub mod gas_price;
pub mod priority_fee;

pub use base_fee::{BaseFeeCalculator, BASE_FEE_CHANGE_DENOMINATOR, ELASTICITY_MULTIPLIER};
pub use error::FeeError;
pub use fee_history::{FeeHistory, FeeHistoryEntry};
pub use gas_price::GasPriceOracle;
pub use priority_fee::PriorityFeeEstimator;