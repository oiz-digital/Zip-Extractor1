//! Price feed definitions and data types.

use serde::{Serialize, Deserialize};
use std::fmt;
use crate::OracleError;

/// Number of decimal places for all prices (same as Chainlink: 8 decimals).
/// Price = raw_value / 10^8
/// e.g. ZBX/USD = 2_50000000 → $2.50
pub const DECIMALS: u8 = 8;
pub const DECIMALS_MULTIPLIER: u128 = 100_000_000; // 10^8

/// A feed identifier (e.g. "ZBX/USD", "ETH/USD").
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FeedId(pub String);

impl FeedId {
    // ── ZBX native ────────────────────────────────────────────────────────────
    pub fn zbx_usd()  -> Self { Self("ZBX/USD".into())  }
    pub fn zusd_usd() -> Self { Self("ZUSD/USD".into()) }
    /// ZNS/USD — Zebvix Name Service token price (like ENS on Ethereum).
    pub fn zns_usd()  -> Self { Self("ZNS/USD".into())  }

    // ── Major crypto ─────────────────────────────────────────────────────────
    pub fn eth_usd()  -> Self { Self("ETH/USD".into())  }
    pub fn btc_usd()  -> Self { Self("BTC/USD".into())  }
    pub fn bnb_usd()  -> Self { Self("BNB/USD".into())  }

    // ── New: alt-coins + L2 tokens (Session 40) ───────────────────────────────
    /// SOL/USD — Solana price feed.
    pub fn sol_usd()  -> Self { Self("SOL/USD".into())  }
    /// AVAX/USD — Avalanche native token.
    pub fn avax_usd() -> Self { Self("AVAX/USD".into()) }
    /// MATIC/USD — Polygon native token (also used on Polygon zkEVM).
    pub fn matic_usd() -> Self { Self("MATIC/USD".into()) }
    /// ARB/USD — Arbitrum governance token.
    pub fn arb_usd()  -> Self { Self("ARB/USD".into())  }
    /// OP/USD — Optimism governance token.
    pub fn op_usd()   -> Self { Self("OP/USD".into())   }
    /// LINK/USD — Chainlink token (oracle-of-oracles reference price).
    pub fn link_usd() -> Self { Self("LINK/USD".into()) }
    /// DOT/USD — Polkadot relay chain token (cross-chain reference).
    pub fn dot_usd()  -> Self { Self("DOT/USD".into())  }

    // ── INR feeds ─────────────────────────────────────────────────────────────
    /// USD/INR — Indian Rupee forex rate.
    /// Sources: RBI reference rate, ExchangeRate-API, WazirX, CoinDCX, AI LLM.
    pub fn usd_inr()  -> Self { Self("USD/INR".into())  }

    /// All standard feed IDs (14 total after Session 40 upgrade).
    pub fn all() -> Vec<Self> {
        vec![
            // ZBX native
            Self::zbx_usd(), Self::zusd_usd(), Self::zns_usd(),
            // Major crypto
            Self::eth_usd(), Self::btc_usd(), Self::bnb_usd(),
            // Alt-coins + L2 (new Session 40)
            Self::sol_usd(), Self::avax_usd(), Self::matic_usd(),
            Self::arb_usd(), Self::op_usd(), Self::link_usd(), Self::dot_usd(),
            // Forex
            Self::usd_inr(),
        ]
    }

    /// Crypto-only feeds (no forex). Used by CEX fetcher.
    pub fn crypto_feeds() -> Vec<Self> {
        vec![
            Self::zbx_usd(), Self::zusd_usd(), Self::zns_usd(),
            Self::eth_usd(), Self::btc_usd(), Self::bnb_usd(),
            Self::sol_usd(), Self::avax_usd(), Self::matic_usd(),
            Self::arb_usd(), Self::op_usd(), Self::link_usd(), Self::dot_usd(),
        ]
    }
}

impl fmt::Display for FeedId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{}", self.0) }
}

/// A raw price value (8 decimal places, like Chainlink).
/// $2.50 → Price(250_000_000)
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Price(pub i128);

impl Price {
    /// From a float (e.g. 2.5 → Price(250_000_000)).
    pub fn from_f64(f: f64) -> Self {
        Self((f * DECIMALS_MULTIPLIER as f64) as i128)
    }

    /// As a float.
    pub fn to_f64(self) -> f64 {
        self.0 as f64 / DECIMALS_MULTIPLIER as f64
    }

    /// Is this price sane? (> 0, < 10 billion dollars)
    pub fn is_valid(self) -> bool {
        self.0 > 0 && self.0 < 1_000_000_000 * DECIMALS_MULTIPLIER as i128
    }

    /// Percent deviation from another price (absolute).
    pub fn deviation_pct(self, other: Price) -> f64 {
        if other.0 == 0 { return f64::MAX; }
        ((self.0 - other.0).abs() as f64 / other.0 as f64) * 100.0
    }
}

impl fmt::Display for Price {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "${:.8}", self.to_f64())
    }
}

/// Configuration for a single price feed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PriceFeed {
    /// Feed identifier (e.g. "ZBX/USD")
    pub id:                  FeedId,
    /// Number of decimal places (always 8 for ZBX oracle)
    pub decimals:            u8,
    /// Minimum reporters needed for a valid round
    pub min_reporters:       u32,
    /// Deviation threshold to trigger an update (basis points, 50 = 0.5%)
    pub deviation_threshold: u32,
    /// Maximum time between updates (seconds) — heartbeat
    pub heartbeat_secs:      u64,
    /// On-chain contract address storing this feed's data
    pub contract_address:    [u8; 20],
    /// Last known valid price
    pub last_price:          Option<Price>,
    /// Unix timestamp of last update
    pub last_updated:        u64,
    /// Minimum valid answer (circuit breaker — like Chainlink minAnswer).
    /// If reported price < min_answer, the round is rejected.
    /// Prevents manipulation with near-zero prices.
    /// Example ZBX/USD: min_answer = 1_000_000 (= $0.01, 8 decimals)
    pub min_answer:          i128,
    /// Maximum valid answer (circuit breaker — like Chainlink maxAnswer).
    /// If reported price > max_answer, the round is rejected.
    /// Prevents astronomical price manipulation.
    /// Example ZBX/USD: max_answer = 1_000_000_000_000 (= $10,000, 8 decimals)
    pub max_answer:          i128,
}

impl PriceFeed {
    pub fn zbx_usd(contract: [u8; 20]) -> Self {
        Self {
            id:                  FeedId::zbx_usd(),
            decimals:            DECIMALS,
            min_reporters:       5,
            deviation_threshold: 50,          // 0.5%
            heartbeat_secs:      3600,         // 1 hour
            contract_address:    contract,
            last_price:          None,
            last_updated:        0,
            min_answer:          1_000_000i128,        // $0.01 (8 decimals) — circuit breaker low
            max_answer:          100_000_000_000i128,  // $1,000 (8 decimals) — circuit breaker high
        }
    }

    pub fn zusd_usd(contract: [u8; 20]) -> Self {
        Self {
            id:                  FeedId::zusd_usd(),
            decimals:            DECIMALS,
            min_reporters:       5,
            deviation_threshold: 10,           // 0.1% — tighter for stablecoin
            heartbeat_secs:      1800,          // 30 min
            contract_address:    contract,
            last_price:          None,
            last_updated:        0,
            min_answer:          90_000_000i128,   // $0.90 — stablecoin circuit breaker low
            max_answer:          110_000_000i128,  // $1.10 — stablecoin circuit breaker high
        }
    }

    pub fn eth_usd(contract: [u8; 20]) -> Self {
        Self {
            id:                  FeedId::eth_usd(),
            decimals:            DECIMALS,
            min_reporters:       5,
            deviation_threshold: 50,
            heartbeat_secs:      3600,
            contract_address:    contract,
            last_price:          None,
            last_updated:        0,
            min_answer:          1_00000000i128,          // $1 minimum
            max_answer:          1_000_000_00000000i128,  // $1M maximum
        }
    }

    pub fn btc_usd(contract: [u8; 20]) -> Self {
        Self {
            id:                  FeedId::btc_usd(),
            decimals:            DECIMALS,
            min_reporters:       5,
            deviation_threshold: 50,
            heartbeat_secs:      3600,
            contract_address:    contract,
            last_price:          None,
            last_updated:        0,
            min_answer:          1_000_00000000i128,       // $1,000 minimum
            max_answer:          10_000_000_00000000i128,  // $10M maximum
        }
    }

    /// ZNS/USD — Zebvix Name Service token price oracle feed.
    /// ZNS (.zbx domain names) — like ENS on Ethereum, native to ZBX chain.
    pub fn zns_usd(contract: [u8; 20]) -> Self {
        Self {
            id:                  FeedId::zns_usd(),
            decimals:            DECIMALS,
            min_reporters:       3,
            deviation_threshold: 100,
            heartbeat_secs:      7200,
            contract_address:    contract,
            last_price:          None,
            last_updated:        0,
            min_answer:          10_000i128,
            max_answer:          1_000_00000000i128,
        }
    }

    // ── INR feeds ──────────────────────────────────────────────────────────────

    /// USD/INR — Indian Rupee forex rate (8 decimals).
    ///
    /// Sources (priority): RBI reference rate → ExchangeRate-API →
    ///   Fixer.io → WazirX USDT/INR → CoinDCX USDT/INR.
    ///
    /// Price encoding: ₹83.50/USD → 83_50000000 (8 decimals).
    ///
    /// Circuit breakers:
    ///   min_answer = 60_00000000  (₹60/USD — INR all-time strong)
    ///   max_answer = 120_00000000 (₹120/USD — extreme depreciation guard)
    pub fn usd_inr(contract: [u8; 20]) -> Self {
        Self {
            id:                  FeedId::usd_inr(),
            decimals:            DECIMALS,
            min_reporters:       3,            // forex sources, not validator-based
            deviation_threshold: 20,           // 0.2% — forex moves slowly
            heartbeat_secs:      3_600,        // 1 hour — RBI rate updated daily
            contract_address:    contract,
            last_price:          None,
            last_updated:        0,
            min_answer:          60_00000000i128,   // ₹60/USD floor
            max_answer:          120_00000000i128,  // ₹120/USD ceiling
        }
    }

    // ── New feeds — Session 40 ────────────────────────────────────────────────

    pub fn sol_usd(contract: [u8; 20]) -> Self {
        Self {
            id:                  FeedId::sol_usd(),
            decimals:            DECIMALS,
            min_reporters:       5,
            deviation_threshold: 50,
            heartbeat_secs:      3600,
            contract_address:    contract,
            last_price:          None,
            last_updated:        0,
            min_answer:          1_00000000i128,       // $1
            max_answer:          100_000_00000000i128, // $100,000
        }
    }

    pub fn avax_usd(contract: [u8; 20]) -> Self {
        Self {
            id:                  FeedId::avax_usd(),
            decimals:            DECIMALS,
            min_reporters:       5,
            deviation_threshold: 50,
            heartbeat_secs:      3600,
            contract_address:    contract,
            last_price:          None,
            last_updated:        0,
            min_answer:          1_00000000i128,      // $1
            max_answer:          10_000_00000000i128, // $10,000
        }
    }

    pub fn matic_usd(contract: [u8; 20]) -> Self {
        Self {
            id:                  FeedId::matic_usd(),
            decimals:            DECIMALS,
            min_reporters:       3,
            deviation_threshold: 100,
            heartbeat_secs:      7200,
            contract_address:    contract,
            last_price:          None,
            last_updated:        0,
            min_answer:          100_000i128,         // $0.001
            max_answer:          1_000_00000000i128,  // $1,000
        }
    }

    pub fn arb_usd(contract: [u8; 20]) -> Self {
        Self {
            id:                  FeedId::arb_usd(),
            decimals:            DECIMALS,
            min_reporters:       3,
            deviation_threshold: 100,
            heartbeat_secs:      7200,
            contract_address:    contract,
            last_price:          None,
            last_updated:        0,
            min_answer:          100_000i128,         // $0.001
            max_answer:          10_000_00000000i128, // $10,000
        }
    }

    pub fn op_usd(contract: [u8; 20]) -> Self {
        Self {
            id:                  FeedId::op_usd(),
            decimals:            DECIMALS,
            min_reporters:       3,
            deviation_threshold: 100,
            heartbeat_secs:      7200,
            contract_address:    contract,
            last_price:          None,
            last_updated:        0,
            min_answer:          100_000i128,
            max_answer:          10_000_00000000i128,
        }
    }

    pub fn link_usd(contract: [u8; 20]) -> Self {
        Self {
            id:                  FeedId::link_usd(),
            decimals:            DECIMALS,
            min_reporters:       3,
            deviation_threshold: 50,
            heartbeat_secs:      7200,
            contract_address:    contract,
            last_price:          None,
            last_updated:        0,
            min_answer:          10_000_000i128,         // $0.10
            max_answer:          100_000_00000000i128,   // $100,000
        }
    }

    pub fn dot_usd(contract: [u8; 20]) -> Self {
        Self {
            id:                  FeedId::dot_usd(),
            decimals:            DECIMALS,
            min_reporters:       3,
            deviation_threshold: 100,
            heartbeat_secs:      7200,
            contract_address:    contract,
            last_price:          None,
            last_updated:        0,
            min_answer:          1_00000000i128,      // $1
            max_answer:          10_000_00000000i128, // $10,000
        }
    }

    /// Validate a reported price against circuit breaker bounds.
    /// Returns Err if price is outside [min_answer, max_answer].
    pub fn validate_answer(&self, price: i128) -> Result<(), OracleError> {
        if price < self.min_answer {
            return Err(OracleError::BelowMinAnswer {
                feed:       self.id.0.clone(),
                reported:   price,
                min_answer: self.min_answer,
            });
        }
        if price > self.max_answer {
            return Err(OracleError::AboveMaxAnswer {
                feed:       self.id.0.clone(),
                reported:   price,
                max_answer: self.max_answer,
            });
        }
        Ok(())
    }

    /// Check if this feed needs an update.
    pub fn needs_update(&self, current_time: u64, new_price: Price) -> bool {
        // Heartbeat: too long since last update
        if current_time.saturating_sub(self.last_updated) >= self.heartbeat_secs {
            return true;
        }
        // Deviation: price moved too much
        if let Some(last) = self.last_price {
            let deviation_bps = (last.deviation_pct(new_price) * 100.0) as u32;
            if deviation_bps >= self.deviation_threshold {
                return true;
            }
        } else {
            return true; // No price yet
        }
        false
    }
}