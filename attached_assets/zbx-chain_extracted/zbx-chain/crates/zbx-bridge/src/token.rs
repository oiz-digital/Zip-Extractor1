//! Multi-token whitelist and daily rate-limiting for the ZBX bridge.
//!
//! ## Token models
//!
//! ### Lock-and-Mint (native tokens)
//!   Source chain : `lock_tokens()`  → tokens held in bridge escrow
//!   Dest chain   : `mint_wrapped()` → wrapped token minted 1:1
//!   Reverse      : burn_wrapped → unlock_tokens
//!
//! ## Supported tokens (default whitelist)
//!
//! ZBX Chain has two native tokens supported on the bridge:
//!
//! | Token  | Model         | Max/tx        | Daily limit       |
//! |--------|---------------|---------------|-------------------|
//! | ZBX    | Lock-and-Mint | 1M ZBX        | 10M ZBX           |
//! | ZUSD   | Lock-and-Mint | 5M ZUSD       | 50M ZUSD          |
//!
//! Both are `is_native = true` — they are ZBX Chain's first-class
//! protocol tokens and use Lock-and-Mint (escrow on ZBX, mint wrapped
//! on Ethereum / BSC / Polygon).

use crate::error::BridgeError;
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Sentinel address used as key for native ZBX (no contract address).
pub const NATIVE_ZBX_SENTINEL: [u8; 20] = [0xEEu8; 20];

/// Genesis address of the ZUSD stablecoin contract on ZBX Chain.
/// Derived from mainnet chain ID 0x231D = 8989.
pub const ZUSD_GENESIS_ADDR: [u8; 20] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0x23, 0x1D, 0x00, 0x01,
];

/// Per-token configuration for the bridge whitelist.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeToken {
    /// Contract address on ZBX Chain (or `NATIVE_ZBX_SENTINEL` for native ZBX).
    pub address:     [u8; 20],
    /// Token symbol, e.g. "ZBX", "ZUSD".
    pub symbol:      String,
    /// Token decimals (18 for all ZBX-chain native tokens).
    pub decimals:    u8,
    /// Maximum amount in a single bridge transaction (in token base units).
    pub max_per_tx:  u128,
    /// Maximum total bridged in a rolling 24-hour window (in token base units).
    pub daily_limit: u128,
    /// True = Lock-and-Mint. All native tokens use Lock-and-Mint.
    pub is_native:   bool,
    /// False = bridging this token is temporarily paused (emergency stop).
    pub enabled:     bool,
}

impl BridgeToken {
    /// Native ZBX — Lock-and-Mint, 1M max/tx, 10M/day.
    pub fn native_zbx() -> Self {
        BridgeToken {
            address:     NATIVE_ZBX_SENTINEL,
            symbol:      "ZBX".into(),
            decimals:    18,
            max_per_tx:  1_000_000 * 10u128.pow(18),   // 1M ZBX
            daily_limit: 10_000_000 * 10u128.pow(18),  // 10M ZBX/day
            is_native:   true,
            enabled:     true,
        }
    }

    /// ZUSD — USD stablecoin, Lock-and-Mint, 5M max/tx, 50M/day.
    pub fn zusd(address: [u8; 20]) -> Self {
        BridgeToken {
            address,
            symbol:      "ZUSD".into(),
            decimals:    18,
            max_per_tx:  5_000_000 * 10u128.pow(18),   // 5M ZUSD
            daily_limit: 50_000_000 * 10u128.pow(18),  // 50M ZUSD/day
            is_native:   true,  // ZBX-native stablecoin — Lock-and-Mint
            enabled:     true,
        }
    }
}

/// Whitelist of tokens allowed to use the bridge.
///
/// Only ZBX Chain's two native tokens (ZBX, ZUSD) are supported.
/// Bridge admin can update limits or pause individual tokens.
#[derive(Debug, Default)]
pub struct TokenWhitelist {
    tokens: HashMap<[u8; 20], BridgeToken>,
}

impl TokenWhitelist {
    /// Create the default mainnet whitelist seeded with both native tokens.
    pub fn default_mainnet() -> Self {
        let mut wl = TokenWhitelist::default();
        wl.insert(BridgeToken::native_zbx());
        wl.insert(BridgeToken::zusd(ZUSD_GENESIS_ADDR));
        wl
    }

    /// Insert or replace a token entry.
    pub fn insert(&mut self, token: BridgeToken) {
        info!(symbol = %token.symbol, "bridge: token registered in whitelist");
        self.tokens.insert(token.address, token);
    }

    /// Disable a token — stops new bridge requests; in-flight requests unaffected.
    pub fn disable(&mut self, token_addr: &[u8; 20]) {
        if let Some(t) = self.tokens.get_mut(token_addr) {
            warn!(symbol = %t.symbol, "bridge: token disabled");
            t.enabled = false;
        }
    }

    /// Re-enable a previously disabled token.
    pub fn enable(&mut self, token_addr: &[u8; 20]) {
        if let Some(t) = self.tokens.get_mut(token_addr) {
            info!(symbol = %t.symbol, "bridge: token re-enabled");
            t.enabled = true;
        }
    }

    /// Check whether `token_addr` is allowed (whitelisted and enabled).
    pub fn get_enabled(&self, token_addr: &[u8; 20]) -> Result<&BridgeToken, BridgeError> {
        match self.tokens.get(token_addr) {
            None    => Err(BridgeError::TokenNotWhitelisted(hex::encode(token_addr))),
            Some(t) if !t.enabled => Err(BridgeError::TokenDisabled(t.symbol.clone())),
            Some(t) => Ok(t),
        }
    }

    /// Iterate all tokens (for status/monitoring).
    pub fn all(&self) -> impl Iterator<Item = &BridgeToken> {
        self.tokens.values()
    }
}

/// Per-token daily withdrawal limit tracker.
///
/// Resets at UTC midnight (Unix timestamp / 86_400).
/// Prevents large one-day outflows from draining the bridge treasury.
#[derive(Debug, Default)]
pub struct DailyLimitTracker {
    /// token_address → (day_number, amount_bridged_today)
    daily_totals: HashMap<[u8; 20], (u64, u128)>,
}

impl DailyLimitTracker {
    /// Create a new in-memory daily limit tracker.
    ///
    /// L-6 warning: this tracker is held entirely in process memory.
    /// A node restart resets all daily totals to zero, allowing a new
    /// 24-hour quota from scratch. In production, daily totals MUST be
    /// persisted to RocksDB (or the same persistence layer as the spent-ops
    /// set) and reloaded on startup, otherwise a restart can be used to
    /// bypass daily bridge limits. Until persistence is added, operators
    /// should monitor for abnormal restarts and audit daily volumes manually.
    pub fn new() -> Self {
        tracing::warn!(
            "DailyLimitTracker: daily totals are in-memory only — \
             node restart resets limits. Persistent storage required for production."
        );
        DailyLimitTracker::default()
    }

    fn day_of(timestamp: u64) -> u64 {
        timestamp / 86_400
    }

    /// Amount bridged today for `token`.
    pub fn bridged_today(&self, token: &[u8; 20], timestamp: u64) -> u128 {
        let today = Self::day_of(timestamp);
        match self.daily_totals.get(token) {
            Some((day, total)) if *day == today => *total,
            _ => 0,
        }
    }

    /// Verify the pending `amount` fits within the daily limit.
    /// Returns remaining capacity after this transfer if Ok.
    pub fn check(
        &self,
        token:     &[u8; 20],
        amount:    u128,
        limit:     u128,
        timestamp: u64,
    ) -> Result<u128, BridgeError> {
        let today_total = self.bridged_today(token, timestamp);
        if today_total + amount > limit {
            return Err(BridgeError::DailyLimitExceeded {
                limit,
                used:      today_total,
                requested: amount,
            });
        }
        Ok(limit - today_total - amount)
    }

    /// Record a confirmed bridge amount (call only after all validations pass).
    pub fn record(&mut self, token: [u8; 20], amount: u128, timestamp: u64) {
        let today = Self::day_of(timestamp);
        let entry = self.daily_totals.entry(token).or_insert((today, 0));
        if entry.0 != today {
            *entry = (today, 0);
        }
        entry.1 += amount;
    }
}
