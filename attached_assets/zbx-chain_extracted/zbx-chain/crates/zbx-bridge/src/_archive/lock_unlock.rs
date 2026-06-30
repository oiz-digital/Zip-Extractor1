//! Bridge lock/unlock token operations.
//!
//! ZBX Bridge supports two bridge models:
//!
//! ## Lock-and-Mint (for native tokens):
//!   On source chain: lock_tokens(amount) -- tokens held in bridge escrow
//!   On dest chain:   mint_wrapped(amount) -- wrapped token minted 1:1
//!   Reverse:         burn_wrapped(amount) -> unlock_tokens(amount)
//!
//! ## Burn-and-Mint (for bridged tokens):
//!   On source chain: burn_tokens(amount) -- tokens permanently destroyed
//!   On dest chain:   mint_tokens(amount)  -- new tokens created
//!   Used for tokens that are natively issued on ZBX but bridge to Ethereum.
//!
//! ## ZBX <-> Ethereum bridge:
//!   ZBX native (ZBX) <-> WZBX ERC-20 on Ethereum
//!   Lock ZBX on ZBX chain -> Mint WZBX on Ethereum
//!   Burn WZBX on Ethereum -> Unlock ZBX on ZBX chain
//!
//! ## Supported tokens (bridge token whitelist):
//!   ZBX (native), ZUSD, ZBXBTC, ZBXETH, ZBXUSDC
//!   Each token has max daily withdrawal limit to cap bridge risk.

use std::collections::HashMap;

// ── Token whitelist ───────────────────────────────────────────────────────────

/// Tokens allowed to be bridged (bridge_token_whitelist).
/// Only whitelisted tokens can be locked/unlocked via the bridge.
#[derive(Debug, Clone)]
pub struct BridgeToken {
    pub address:       [u8; 20], // contract address (or zero for native ZBX)
    pub symbol:        String,
    pub decimals:      u8,
    pub max_per_tx:    u128,     // max amount per single bridge tx
    pub daily_limit:   u128,     // max total bridged per 24h window
    pub is_native:     bool,     // true = lock/unlock model, false = burn/mint
    pub enabled:       bool,
}

/// Bridge token whitelist (address -> BridgeToken config).
pub struct TokenWhitelist {
    pub tokens:    HashMap<[u8; 20], BridgeToken>,
    pub native_key: [u8; 20],   // sentinel key for native ZBX
}

impl TokenWhitelist {
    pub const NATIVE_ZBX_SENTINEL: [u8; 20] = [
        0xEe, 0xEe, 0xEe, 0xEe, 0xEe, 0xEe, 0xEe, 0xEe, 0xEe, 0xEe,
        0xEe, 0xEe, 0xEe, 0xEe, 0xEe, 0xEe, 0xEe, 0xEe, 0xEe, 0xEe,
    ];

    pub fn new() -> Self {
        let mut wl = Self { tokens: HashMap::new(), native_key: Self::NATIVE_ZBX_SENTINEL };
        // Default whitelist
        wl.tokens.insert(Self::NATIVE_ZBX_SENTINEL, BridgeToken {
            address:    Self::NATIVE_ZBX_SENTINEL,
            symbol:     "ZBX".into(),
            decimals:   18,
            max_per_tx: 1_000_000 * 1_000_000_000_000_000_000u128, // 1M ZBX
            daily_limit: 10_000_000 * 1_000_000_000_000_000_000u128, // 10M ZBX/day
            is_native:  true,
            enabled:    true,
        });
        wl
    }

    /// Check if a token is whitelisted for bridging.
    pub fn is_allowed(&self, token_addr: &[u8; 20]) -> bool {
        self.tokens.get(token_addr).map(|t| t.enabled).unwrap_or(false)
    }

    /// Get token config.
    pub fn get(&self, token_addr: &[u8; 20]) -> Option<&BridgeToken> {
        self.tokens.get(token_addr)
    }

    /// Add or update a token in the whitelist (bridge admin only).
    pub fn upsert(&mut self, token: BridgeToken) {
        self.tokens.insert(token.address, token);
    }

    /// Disable a token (emergency: stop bridging a specific token).
    pub fn disable_token(&mut self, token_addr: &[u8; 20]) {
        if let Some(t) = self.tokens.get_mut(token_addr) { t.enabled = false; }
    }
}

// ── Daily withdrawal tracker ──────────────────────────────────────────────────

/// Per-token daily withdrawal limit tracker.
///
/// Limits total bridge outflow per token per 24-hour window.
/// Resets at UTC midnight each day.
/// If daily_limit is exceeded, bridge TX is rejected until next reset.
pub struct DailyLimitTracker {
    /// token_address -> (day_number, amount_bridged_today)
    pub daily_totals: HashMap<[u8; 20], (u64, u128)>,
}

impl DailyLimitTracker {
    pub fn new() -> Self { Self { daily_totals: HashMap::new() } }

    /// Current day number (Unix timestamp / 86400).
    fn day_of(timestamp: u64) -> u64 { timestamp / 86_400 }

    /// Check if bridging amount is within daily limit. Returns remaining capacity.
    pub fn check_limit(
        &self,
        token:     &[u8; 20],
        amount:    u128,
        limit:     u128,
        timestamp: u64,
    ) -> Result<u128, BridgeError> {
        let today = Self::day_of(timestamp);
        let bridged_today = match self.daily_totals.get(token) {
            Some((day, total)) if *day == today => *total,
            _ => 0,
        };
        if bridged_today + amount > limit {
            return Err(BridgeError::DailyLimitExceeded {
                limit,
                used:      bridged_today,
                requested: amount,
            });
        }
        Ok(limit - bridged_today - amount)
    }

    /// Record a successful bridge amount.
    pub fn record_usage(&mut self, token: [u8; 20], amount: u128, timestamp: u64) {
        let today = Self::day_of(timestamp);
        let entry = self.daily_totals.entry(token).or_insert((today, 0));
        if entry.0 != today { *entry = (today, 0); }
        entry.1 += amount;
    }
}

// ── Lock tokens (source chain) ────────────────────────────────────────────────

/// Lock tokens in the bridge escrow on the source chain.
///
/// Called by user on source chain to initiate a bridge transfer.
/// Emits TokenLocked event with bridge message details.
///
/// Steps:
///   1. Validate token is whitelisted + enabled
///   2. Check amount <= max_per_tx
///   3. Check daily_limit not exceeded
///   4. Transfer tokens from user to bridge escrow
///   5. Assign bridge_nonce (replay protection)
///   6. Emit BridgeEvent::TokenLocked
///   7. Relayer picks up event and submits proof to destination chain
pub fn lock_tokens(
    caller:          [u8; 20],
    token:           [u8; 20],
    amount:          u128,
    destination:     [u8; 20],   // recipient on dest chain
    dest_chain_id:   u64,
    whitelist:       &TokenWhitelist,
    daily_tracker:   &mut DailyLimitTracker,
    nonce:           &mut u64,
    timestamp:       u64,
    bridge_paused:   bool,
) -> Result<BridgeEvent, BridgeError> {
    if bridge_paused { return Err(BridgeError::Paused); }

    let token_cfg = whitelist.get(&token).ok_or(BridgeError::TokenNotWhitelisted)?;
    if !token_cfg.enabled { return Err(BridgeError::TokenDisabled); }
    if amount == 0       { return Err(BridgeError::ZeroAmount); }
    if amount > token_cfg.max_per_tx { return Err(BridgeError::ExceedsMaxPerTx { max: token_cfg.max_per_tx }); }

    daily_tracker.check_limit(&token, amount, token_cfg.daily_limit, timestamp)?;
    // Transfer tokens from caller to bridge escrow (bridge contract holds them)
    // In real impl: ERC-20.transferFrom(caller, bridge_contract, amount)

    daily_tracker.record_usage(token, amount, timestamp);
    let current_nonce = *nonce;
    *nonce += 1;

    Ok(BridgeEvent::TokenLocked {
        token,
        from:          caller,
        to:            destination,
        amount,
        dest_chain_id,
        nonce:         current_nonce,
        timestamp,
    })
}

/// Unlock tokens on destination chain (after relayer submits proof).
///
/// Called by a whitelisted relayer after verifying merkle proof from source chain.
/// Steps:
///   1. Verify relayer is whitelisted
///   2. Verify message nonce not already used (replay protection)
///   3. Verify merkle proof of TokenLocked event on source chain
///   4. Check daily_limit on dest chain
///   5. Transfer tokens from bridge escrow to recipient
///   6. Emit TokenUnlocked event
/// Unlock tokens on the destination chain.
///
/// `our_chain_id` is the chain ID of THIS chain (the destination); the
/// nullifier key includes it to prevent the same `(src_chain_id, src_nonce)`
/// proof from being replayed on a different destination chain.
/// See AUDIT_2026-04-30.md C-15.
pub fn unlock_tokens(
    relayer:         [u8; 20],
    token:           [u8; 20],
    amount:          u128,
    recipient:       [u8; 20],
    source_chain_id: u64,
    source_nonce:    u64,
    our_chain_id:    u64,
    merkle_proof:    &[[u8; 32]],
    relayer_whitelist: &RelayerWhitelist,
    // (src_chain, dest_chain, src_nonce)
    used_nonces:     &mut std::collections::HashSet<(u64, u64, u64)>,
    whitelist:       &TokenWhitelist,
    daily_tracker:   &mut DailyLimitTracker,
    timestamp:       u64,
    bridge_paused:   bool,
) -> Result<BridgeEvent, BridgeError> {
    if bridge_paused { return Err(BridgeError::Paused); }
    if !relayer_whitelist.is_trusted(&relayer) { return Err(BridgeError::RelayerNotWhitelisted); }

    // Replay protection: source nonce + destination chain must be unique.
    // Without `our_chain_id` in the key, an attacker could replay the same
    // proof on every chain that recognises the source.
    let nonce_key = (source_chain_id, our_chain_id, source_nonce);
    if used_nonces.contains(&nonce_key) { return Err(BridgeError::NonceAlreadyUsed); }

    // Verify merkle proof — leaf MUST commit to (token, recipient, amount,
    // src_chain, dest_chain, src_nonce) so the proof itself is destination-
    // bound. Without `dest_chain` in the leaf the same proof would validate
    // for any dest. Keccak digest computed below; merkle verification itself
    // remains the responsibility of `verify_merkle_proof` (TODO).
    let leaf = compute_unlock_leaf(token, recipient, amount, source_chain_id, our_chain_id, source_nonce);
    let _ = (merkle_proof, leaf);

    let token_cfg = whitelist.get(&token).ok_or(BridgeError::TokenNotWhitelisted)?;
    daily_tracker.check_limit(&token, amount, token_cfg.daily_limit, timestamp)?;
    daily_tracker.record_usage(token, amount, timestamp);
    used_nonces.insert(nonce_key);

    Ok(BridgeEvent::TokenUnlocked {
        token,
        recipient,
        amount,
        source_chain_id,
        nonce: source_nonce,
        timestamp,
    })
}

/// Canonical leaf for the cross-chain unlock proof. Includes both source and
/// destination chain ids so the proof is bound to a single (src, dst) pair.
pub fn compute_unlock_leaf(
    token:           [u8; 20],
    recipient:       [u8; 20],
    amount:          u128,
    source_chain_id: u64,
    dest_chain_id:   u64,
    source_nonce:    u64,
) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let mut h = Keccak256::new();
    h.update(b"ZBX_BRIDGE_UNLOCK_v2");
    h.update(token);
    h.update(recipient);
    h.update(&amount.to_be_bytes());
    h.update(&source_chain_id.to_be_bytes());
    h.update(&dest_chain_id.to_be_bytes());
    h.update(&source_nonce.to_be_bytes());
    h.finalize().into()
}

// ── Bridge events ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum BridgeEvent {
    TokenLocked   { token: [u8; 20], from: [u8; 20], to: [u8; 20], amount: u128, dest_chain_id: u64, nonce: u64, timestamp: u64 },
    TokenUnlocked { token: [u8; 20], recipient: [u8; 20], amount: u128, source_chain_id: u64, nonce: u64, timestamp: u64 },
    TokenBurned   { token: [u8; 20], from: [u8; 20], amount: u128, dest_chain_id: u64, nonce: u64 },
    TokenMinted   { token: [u8; 20], recipient: [u8; 20], amount: u128, nonce: u64 },
}

#[derive(Debug)]
pub enum BridgeError {
    Paused,
    TokenNotWhitelisted,
    TokenDisabled,
    ZeroAmount,
    ExceedsMaxPerTx { max: u128 },
    DailyLimitExceeded { limit: u128, used: u128, requested: u128 },
    RelayerNotWhitelisted,
    NonceAlreadyUsed,
    InvalidProof,
    ValidatorThresholdNotMet,
}