//! Multi-token price converter — oracle-based amount estimation.
//!
//! Used by the payment gateway to estimate how much of `input_token` a
//! customer needs to spend to cover an invoice denominated in a different
//! token.  The actual swap is executed on-chain via ZbxRouter; this module
//! provides off-chain estimates for UX (show "you will pay ~X USDC").

use serde::{Deserialize, Serialize};
use zbx_types::address::Address;

/// Price quote between two tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceQuote {
    /// Source token.
    pub input_token:  Address,
    /// Destination token.
    pub output_token: Address,
    /// Amount of output token per 1e18 of input token.
    pub rate:         u128,
    /// Timestamp of the quote.
    pub timestamp:    u64,
}

/// Simple oracle-backed price feed (populated from ZbxOracle / TWAP data).
#[derive(Debug, Default)]
pub struct PriceFeed {
    /// (input_token, output_token) → PriceQuote
    quotes: std::collections::HashMap<(Address, Address), PriceQuote>,
}

impl PriceFeed {
    pub fn new() -> Self { Self::default() }

    /// Update a price quote (called when oracle events are indexed).
    pub fn update(&mut self, q: PriceQuote) {
        self.quotes.insert((q.input_token, q.output_token), q);
    }

    /// Estimate how much `input_token` is needed to get `output_amount`
    /// of `output_token`.
    ///
    /// Returns None if no quote is available.
    pub fn estimate_input(
        &self,
        input_token:   Address,
        output_token:  Address,
        output_amount: u128,
    ) -> Option<u128> {
        // Direct quote.
        if let Some(q) = self.quotes.get(&(input_token, output_token)) {
            // rate = output per 1e18 input, so input = output * 1e18 / rate
            if q.rate == 0 { return None; }
            let input = (output_amount as u128)
                .checked_mul(1_000_000_000_000_000_000u128)?
                .checked_div(q.rate as u128)?;
            return Some(input);
        }
        None
    }

    /// Estimate output amount from a given input amount.
    pub fn estimate_output(
        &self,
        input_token:  Address,
        output_token: Address,
        input_amount: u128,
    ) -> Option<u128> {
        if let Some(q) = self.quotes.get(&(input_token, output_token)) {
            let output = (input_amount as u128)
                .checked_mul(q.rate as u128)?
                .checked_div(1_000_000_000_000_000_000u128)?;
            return Some(output);
        }
        None
    }

    /// Compute protocol fee amount.
    pub fn compute_fee(amount: u128, fee_bps: u16) -> u128 {
        (amount * fee_bps as u128) / 10_000
    }
}
