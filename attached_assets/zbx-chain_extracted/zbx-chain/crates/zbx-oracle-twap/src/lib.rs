//! On-chain TWAP Oracle for ZBX chain (ZEP-012).
//!
//! # What is TWAP?
//!
//! Time-Weighted Average Price = average of prices over time.
//!
//! Instead of spot price (easy to manipulate in one block),
//! TWAP averages over N blocks (e.g. 30 min = ~900 blocks).
//!
//! Example:
//!   Block 1000: $2.50
//!   Block 1001: $9999 ← flash loan attack (1 block)
//!   Block 1002: $2.51
//!   Block 1003: $2.49
//!   ...
//!   30-min TWAP: ~$2.50 (attacker barely moved the average)
//!
//! # Implementation
//!
//! Uses cumulative price accumulators (same as Uniswap v2/v3):
//!
//! ```
//! price_cumulative += price × time_elapsed
//! TWAP(t1, t2) = (cumulative[t2] - cumulative[t1]) / (t2 - t1)
//! ```
//!
//! This is O(1) storage — just store the cumulative sum.
//!
//! # Manipulation Cost
//!
//! To move 30-min TWAP by 1%:
//!   Attacker must hold 1% price impact for 30 minutes
//!   Cost = opportunity cost of capital × time
//!   For $1M TVL pool: ~$10,000 manipulation cost for 1% move
//!
//! # ZBX AMM Integration
//!
//! ZBX AMM pools automatically update TWAP accumulators each block.
//! No oracle nodes needed — purely on-chain!

pub mod accumulator;
pub mod observer;
pub mod pool_oracle;

pub use accumulator::{PriceAccumulator, TwapWindow};
pub use pool_oracle::PoolOracle;
pub use observer::TwapObserver;