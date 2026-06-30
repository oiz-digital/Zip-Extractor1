//! Token factory — create custom ZRC-20 tokens with a paid creation fee.
//!
//! ## Flow
//!
//! ```text
//! Creator ──create_token(name, symbol, decimals, supply, ...)──►
//!   TokenFactory
//!     ├── Validates: name/symbol non-empty, decimals ≤ 18, supply ≤ MAX_SUPPLY
//!     ├── Collects TOKEN_CREATION_FEE_WEI (100 ZBX) from creator
//!     ├── Assigns deterministic token address (based on creator + nonce)
//!     ├── Mints total_supply to creator's wallet (constructor-mint, no post-deploy call)
//!     └── Registers token metadata (name, symbol, decimals, owner, logo_uri)
//! ```
//!
//! ## Fee operations (all require paid fee)
//!
//! | Operation | Fee | Description |
//! |-----------|-----|-------------|
//! | create_token | 100 ZBX | Deploy a new ZRC-20 token |
//! | mint_tokens | 1 ZBX/call | Mint additional supply (if mintable and not paused/finalized) |
//! | burn_tokens | 0 ZBX | Free — reduces supply |
//! | pause_token | 5 ZBX | Emergency pause transfers |
//! | register_metadata | 10 ZBX | Register token icon/website/description |
//!
//! ## ZRC-20 v1.1 surface (ZEP-006 — Session 38)
//!
//! All advanced ZEP-006 features are now tracked per token in the factory registry:
//!   - Freeze (USDC-style blacklist): `freeze_account` / `unfreeze_account`
//!   - Native time-lock: `lock_tokens` / `extend_lock` / `locked_balance_of`
//!   - Mint enable/disable: `pause_minting` / `resume_minting` / `finalize_minting`
//!   - Logo URI: stored at creation; updatable via `update_logo_uri`

use std::collections::HashMap;
use sha3::{Digest, Sha3_256};
use zbx_types::address::Address;
use serde::{Deserialize, Serialize};
use crate::registry::FeeRegistry;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum total supply for any token (10^36; well within u128 range of ~3.4×10^38).
pub const MAX_TOKEN_SUPPLY: u128 = 1_000_000_000_000_000_000_000_000_000_000_000_000u128;

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenFactoryError {
    /// Token name must be 1–64 characters.
    InvalidName,
    /// Token symbol must be 1–12 uppercase ASCII characters.
    InvalidSymbol,
    /// Decimals must be 0–18.
    InvalidDecimals,
    /// Total supply exceeds MAX_TOKEN_SUPPLY.
    SupplyTooLarge,
    /// Creator's ZBX balance is insufficient to pay the creation fee.
    InsufficientFee { have: u128, need: u128 },
    /// Token not found in registry.
    TokenNotFound,
    /// Operation requires the token owner.
    Unauthorized,
    /// Token transfers are paused (emergency stop).
    TokenPaused,
    /// Token is not mintable (was created with fixed supply).
    NotMintable,
    /// Attempted mint would exceed max supply.
    MintExceedsMaxSupply,
    /// Symbol already registered.
    SymbolAlreadyExists,

    // ── ZEP-006 v1.1 additions ────────────────────────────────────────────────

    /// Account is frozen (USDC-style blacklist) — all movement blocked.
    AccountFrozen,
    /// Account is not frozen — unfreeze() called unnecessarily.
    AccountNotFrozen,
    /// Account is already frozen.
    AccountAlreadyFrozen,
    /// Mint blocked because `mintingPaused == true`.
    MintingPaused,
    /// Mint blocked because `mintingFinalized == true` (permanent kill switch).
    MintingFinalized,
    /// `pauseMinting()` called when already paused.
    MintingAlreadyPaused,
    /// `resumeMinting()` called when not paused.
    MintingNotPaused,
    /// `finalizeMinting()` called when already finalized.
    MintingAlreadyFinalized,
    /// `lockTokens()` called while an active lock exists — use `extend_lock`.
    ActiveLockExists,
    /// `extendLock()` requires both new_amount ≥ current and new_unlock_time ≥ current.
    LockMustGrow,
    /// `extendLock()` called when no lock exists for the account.
    LockNotFound,
    /// `extendLock()` called after the lock has already expired.
    LockExpired,
    /// `lockTokens()` called with unlock_time ≤ current_time.
    UnlockTimeInPast,
    /// `lockTokens()` amount exceeds account balance.
    InsufficientBalanceForLock { have: u128, need: u128 },
    /// Transfer would dip into the locked portion.
    TokensLocked { transferable: u128 },
}

impl std::fmt::Display for TokenFactoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidName         => write!(f, "token name must be 1–64 characters"),
            Self::InvalidSymbol       => write!(f, "token symbol must be 1–12 characters"),
            Self::InvalidDecimals     => write!(f, "decimals must be 0–18"),
            Self::SupplyTooLarge      => write!(f, "total supply exceeds maximum"),
            Self::InsufficientFee { have, need } =>
                write!(f, "insufficient fee: have {have} wei, need {need} wei"),
            Self::TokenNotFound       => write!(f, "token not found"),
            Self::Unauthorized        => write!(f, "only the token owner can perform this operation"),
            Self::TokenPaused         => write!(f, "token transfers are paused"),
            Self::NotMintable         => write!(f, "token has a fixed supply"),
            Self::MintExceedsMaxSupply => write!(f, "mint would exceed maximum supply"),
            Self::SymbolAlreadyExists => write!(f, "token symbol already registered"),
            Self::AccountFrozen       => write!(f, "account is frozen"),
            Self::AccountNotFrozen    => write!(f, "account is not frozen"),
            Self::AccountAlreadyFrozen => write!(f, "account is already frozen"),
            Self::MintingPaused       => write!(f, "minting is paused"),
            Self::MintingFinalized    => write!(f, "minting is permanently finalized"),
            Self::MintingAlreadyPaused => write!(f, "minting is already paused"),
            Self::MintingNotPaused    => write!(f, "minting is not currently paused"),
            Self::MintingAlreadyFinalized => write!(f, "minting is already finalized"),
            Self::ActiveLockExists    => write!(f, "active lock exists — use extend_lock"),
            Self::LockMustGrow        => write!(f, "lock update must not shrink amount or time"),
            Self::LockNotFound        => write!(f, "no active lock found for account"),
            Self::LockExpired         => write!(f, "lock has already expired"),
            Self::UnlockTimeInPast    => write!(f, "unlock_time must be in the future"),
            Self::InsufficientBalanceForLock { have, need } =>
                write!(f, "balance {have} is less than lock amount {need}"),
            Self::TokensLocked { transferable } =>
                write!(f, "tokens locked; transferable: {transferable}"),
        }
    }
}
impl std::error::Error for TokenFactoryError {}

// ── Supporting types ──────────────────────────────────────────────────────────

/// Per-account native time-lock entry (ZEP-006 §3.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockEntry {
    /// Base-unit amount locked. 0 = no lock.
    pub amount:      u128,
    /// Unix-seconds timestamp at which the lock expires.
    pub unlock_time: u64,
}

// ── TokenRecord ───────────────────────────────────────────────────────────────

/// Full metadata for a created token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenRecord {
    /// EVM address for this token (deterministically derived).
    pub address:       Address,
    pub name:          String,
    pub symbol:        String,
    pub decimals:      u8,
    /// Total minted supply (wei-denominated, decimals already applied).
    pub total_supply:  u128,
    /// Maximum supply cap (0 = uncapped).
    pub max_supply:    u128,
    /// Owner / deployer address.
    pub owner:         Address,
    /// Whether new tokens can be minted post-creation.
    pub mintable:      bool,
    /// Whether transfers are currently paused (emergency stop).
    pub paused:        bool,
    /// Creation block.
    pub created_at:    u64,
    /// Optional metadata: icon URL, website, description.
    pub metadata_uri:  Option<String>,

    // ── ZEP-006 v1.1 fields ──────────────────────────────────────────────────

    /// On-chain logo URI (IPFS or HTTPS). Updatable by owner via `update_logo_uri`.
    pub logo_uri:          Option<String>,
    /// Temporary mint pause — toggleable by owner; blocked if `minting_finalized`.
    pub minting_paused:    bool,
    /// Permanent mint kill switch — once true, can NEVER be reversed.
    pub minting_finalized: bool,
}

/// Parameters for creating a token.
#[derive(Debug, Clone)]
pub struct CreateTokenParams {
    pub name:          String,
    pub symbol:        String,
    pub decimals:      u8,
    /// Initial total supply in base units (already scaled by 10^decimals if appropriate).
    pub total_supply:  u128,
    /// Optional supply cap. 0 = uncapped.
    pub max_supply:    u128,
    /// If true, owner can mint additional tokens later (costs mint fee per call).
    pub mintable:      bool,
    pub creator:       Address,
    /// Creator's ZBX balance for fee deduction.
    pub creator_zbx_balance: u128,
    pub block_number:  u64,
    /// Optional on-chain logo URI (ZEP-006 §3.5). None = empty string.
    pub logo_uri:      Option<String>,
}

// ── TokenFactory ──────────────────────────────────────────────────────────────

/// Creates and manages custom ZRC-20 tokens on ZBX Chain.
pub struct TokenFactory {
    /// token_address → TokenRecord
    tokens: HashMap<Address, TokenRecord>,
    /// symbol → token_address (uniqueness index)
    symbol_index: HashMap<String, Address>,
    /// creator → nonce (for deterministic address generation)
    creator_nonces: HashMap<Address, u64>,
    /// Collected creation fees (ZBX wei)
    pub treasury_balance: u128,
    fee_registry: FeeRegistry,

    // ── ZEP-006 per-account state ─────────────────────────────────────────────
    // Keyed by (token_address, account_address) to avoid collisions across tokens.

    /// (token, account) → frozen flag. Frozen accounts block all send/receive/mint/burn.
    frozen_accounts: HashMap<(Address, Address), bool>,
    /// (token, account) → native time-lock. Single active lock per account per token.
    token_locks: HashMap<(Address, Address), LockEntry>,
}

impl TokenFactory {
    pub fn new() -> Self {
        TokenFactory {
            tokens:           HashMap::new(),
            symbol_index:     HashMap::new(),
            creator_nonces:   HashMap::new(),
            treasury_balance: 0,
            fee_registry:     FeeRegistry::default(),
            frozen_accounts:  HashMap::new(),
            token_locks:      HashMap::new(),
        }
    }

    // ── Create ─────────────────────────────────────────────────────────────────

    /// Create a new ZRC-20 token. Returns the token address on success.
    ///
    /// `initialSupply` is recorded in the `TokenRecord.total_supply` immediately
    /// — mirrors the ZEP-006 constructor-mint fix in `ZRC20Token.sol`. No
    /// separate `mint()` call is needed.
    pub fn create_token(
        &mut self,
        params: CreateTokenParams,
    ) -> Result<Address, TokenFactoryError> {
        // Validate inputs
        if params.name.is_empty() || params.name.len() > 64 {
            return Err(TokenFactoryError::InvalidName);
        }
        if params.symbol.is_empty() || params.symbol.len() > 12 {
            return Err(TokenFactoryError::InvalidSymbol);
        }
        if params.decimals > 18 {
            return Err(TokenFactoryError::InvalidDecimals);
        }
        if params.total_supply > MAX_TOKEN_SUPPLY {
            return Err(TokenFactoryError::SupplyTooLarge);
        }
        if self.symbol_index.contains_key(&params.symbol) {
            return Err(TokenFactoryError::SymbolAlreadyExists);
        }

        // Fee check
        let fee = self.fee_registry.token_creation_fee();
        if params.creator_zbx_balance < fee {
            return Err(TokenFactoryError::InsufficientFee {
                have: params.creator_zbx_balance,
                need: fee,
            });
        }

        // Deterministic address: SHA3(creator || nonce)
        let nonce = self.creator_nonces.entry(params.creator).or_insert(0);
        let token_addr = derive_token_address(params.creator, *nonce);
        *nonce += 1;

        // Collect fee
        self.treasury_balance = self.treasury_balance.saturating_add(fee);

        let record = TokenRecord {
            address:           token_addr,
            name:              params.name,
            symbol:            params.symbol.clone(),
            decimals:          params.decimals,
            total_supply:      params.total_supply,
            max_supply:        params.max_supply,
            owner:             params.creator,
            mintable:          params.mintable,
            paused:            false,
            created_at:        params.block_number,
            metadata_uri:      None,
            logo_uri:          params.logo_uri,
            minting_paused:    false,
            minting_finalized: false,
        };

        self.symbol_index.insert(params.symbol, token_addr);
        self.tokens.insert(token_addr, record);

        Ok(token_addr)
    }

    // ── Mint ───────────────────────────────────────────────────────────────────

    /// Mint additional tokens. Only callable by owner, only if `mintable == true`.
    /// Charges `mint_fee` in ZBX per call.
    ///
    /// Now also checks `minting_paused` and `minting_finalized` (ZEP-006 §3.3).
    pub fn mint(
        &mut self,
        token:                Address,
        caller:               Address,
        amount:               u128,
        caller_zbx_balance:   u128,
        block_number:         u64,
    ) -> Result<(), TokenFactoryError> {
        let fee = self.fee_registry.token_mint_fee();
        if caller_zbx_balance < fee {
            return Err(TokenFactoryError::InsufficientFee {
                have: caller_zbx_balance,
                need: fee,
            });
        }

        let record = self.tokens.get_mut(&token)
            .ok_or(TokenFactoryError::TokenNotFound)?;

        if record.owner != caller       { return Err(TokenFactoryError::Unauthorized); }
        if !record.mintable             { return Err(TokenFactoryError::NotMintable); }
        if record.minting_finalized     { return Err(TokenFactoryError::MintingFinalized); }
        if record.minting_paused        { return Err(TokenFactoryError::MintingPaused); }
        if record.paused                { return Err(TokenFactoryError::TokenPaused); }
        if record.max_supply > 0 {
            let new_supply = record.total_supply.saturating_add(amount);
            if new_supply > record.max_supply {
                return Err(TokenFactoryError::MintExceedsMaxSupply);
            }
        }

        record.total_supply = record.total_supply.saturating_add(amount);
        self.treasury_balance = self.treasury_balance.saturating_add(fee);
        let _ = block_number;
        Ok(())
    }

    // ── Mint enable/disable (ZEP-006 §3.3) ────────────────────────────────────

    /// `pauseMinting()` — temporarily disable all minting. Reversible.
    pub fn pause_minting(
        &mut self,
        token:  Address,
        caller: Address,
    ) -> Result<(), TokenFactoryError> {
        let record = self.tokens.get_mut(&token)
            .ok_or(TokenFactoryError::TokenNotFound)?;
        if record.owner != caller      { return Err(TokenFactoryError::Unauthorized); }
        if record.minting_finalized    { return Err(TokenFactoryError::MintingFinalized); }
        if record.minting_paused       { return Err(TokenFactoryError::MintingAlreadyPaused); }
        record.minting_paused = true;
        Ok(())
    }

    /// `resumeMinting()` — lift the temporary mint pause.
    pub fn resume_minting(
        &mut self,
        token:  Address,
        caller: Address,
    ) -> Result<(), TokenFactoryError> {
        let record = self.tokens.get_mut(&token)
            .ok_or(TokenFactoryError::TokenNotFound)?;
        if record.owner != caller      { return Err(TokenFactoryError::Unauthorized); }
        if record.minting_finalized    { return Err(TokenFactoryError::MintingFinalized); }
        if !record.minting_paused      { return Err(TokenFactoryError::MintingNotPaused); }
        record.minting_paused = false;
        Ok(())
    }

    /// `finalizeMinting()` — permanent one-way kill switch. Cannot be undone.
    pub fn finalize_minting(
        &mut self,
        token:  Address,
        caller: Address,
    ) -> Result<(), TokenFactoryError> {
        let record = self.tokens.get_mut(&token)
            .ok_or(TokenFactoryError::TokenNotFound)?;
        if record.owner != caller      { return Err(TokenFactoryError::Unauthorized); }
        if record.minting_finalized    { return Err(TokenFactoryError::MintingAlreadyFinalized); }
        record.minting_finalized = true;
        Ok(())
    }

    // ── Pause / Unpause (transfer pause) ──────────────────────────────────────

    /// Pause all transfers for a token. Owner only. Costs `pause_fee`.
    pub fn pause_token(
        &mut self,
        token:              Address,
        caller:             Address,
        caller_zbx_balance: u128,
    ) -> Result<(), TokenFactoryError> {
        let fee = self.fee_registry.token_pause_fee();
        if caller_zbx_balance < fee {
            return Err(TokenFactoryError::InsufficientFee {
                have: caller_zbx_balance,
                need: fee,
            });
        }
        let record = self.tokens.get_mut(&token)
            .ok_or(TokenFactoryError::TokenNotFound)?;
        if record.owner != caller {
            return Err(TokenFactoryError::Unauthorized);
        }
        record.paused = true;
        self.treasury_balance = self.treasury_balance.saturating_add(fee);
        Ok(())
    }

    /// Unpause a token. Owner only. No fee.
    pub fn unpause_token(
        &mut self,
        token:  Address,
        caller: Address,
    ) -> Result<(), TokenFactoryError> {
        let record = self.tokens.get_mut(&token)
            .ok_or(TokenFactoryError::TokenNotFound)?;
        if record.owner != caller {
            return Err(TokenFactoryError::Unauthorized);
        }
        record.paused = false;
        Ok(())
    }

    // ── Freeze (ZEP-006 §3.1 — USDC-style) ────────────────────────────────────

    /// `freeze(account)` — blacklist `account` for this token. Owner only.
    ///
    /// Frozen accounts cannot send, receive, be minted to, or be burned from.
    pub fn freeze_account(
        &mut self,
        token:   Address,
        caller:  Address,
        account: Address,
    ) -> Result<(), TokenFactoryError> {
        let record = self.tokens.get(&token)
            .ok_or(TokenFactoryError::TokenNotFound)?;
        if record.owner != caller { return Err(TokenFactoryError::Unauthorized); }

        let key = (token, account);
        if self.frozen_accounts.get(&key).copied().unwrap_or(false) {
            return Err(TokenFactoryError::AccountAlreadyFrozen);
        }
        self.frozen_accounts.insert(key, true);
        Ok(())
    }

    /// `unfreeze(account)` — lift the freeze. Owner only.
    pub fn unfreeze_account(
        &mut self,
        token:   Address,
        caller:  Address,
        account: Address,
    ) -> Result<(), TokenFactoryError> {
        let record = self.tokens.get(&token)
            .ok_or(TokenFactoryError::TokenNotFound)?;
        if record.owner != caller { return Err(TokenFactoryError::Unauthorized); }

        let key = (token, account);
        if !self.frozen_accounts.get(&key).copied().unwrap_or(false) {
            return Err(TokenFactoryError::AccountNotFrozen);
        }
        self.frozen_accounts.insert(key, false);
        Ok(())
    }

    /// `isFrozen(account)` — view.
    pub fn is_frozen(&self, token: Address, account: Address) -> bool {
        self.frozen_accounts.get(&(token, account)).copied().unwrap_or(false)
    }

    /// `frozenBalance(account)` — returns `balance` if frozen, else 0.
    ///
    /// Note: the factory tracks total_supply but not per-account balances.
    /// Pass in the account's current balance from the chain state layer.
    pub fn frozen_balance(&self, token: Address, account: Address, balance: u128) -> u128 {
        if self.is_frozen(token, account) { balance } else { 0 }
    }

    // ── Native lock (ZEP-006 §3.2) ────────────────────────────────────────────

    /// `lockTokens(account, amount, unlock_time)` — place a fresh lock or replace
    /// an expired one. Owner only.
    ///
    /// - Reverts if an active (non-expired) lock exists — use `extend_lock` instead.
    /// - `current_time`: current Unix timestamp in seconds.
    /// - `account_balance`: current on-chain balance of the account (for `>= amount` check).
    pub fn lock_tokens(
        &mut self,
        token:            Address,
        caller:           Address,
        account:          Address,
        amount:           u128,
        unlock_time:      u64,
        current_time:     u64,
        account_balance:  u128,
    ) -> Result<(), TokenFactoryError> {
        let record = self.tokens.get(&token)
            .ok_or(TokenFactoryError::TokenNotFound)?;
        if record.owner != caller      { return Err(TokenFactoryError::Unauthorized); }
        if amount == 0                 { return Err(TokenFactoryError::InsufficientBalanceForLock { have: 0, need: 1 }); }
        if unlock_time <= current_time { return Err(TokenFactoryError::UnlockTimeInPast); }
        if account_balance < amount    {
            return Err(TokenFactoryError::InsufficientBalanceForLock {
                have: account_balance,
                need: amount,
            });
        }

        let key = (token, account);
        // Check for active (non-expired) lock.
        if let Some(l) = self.token_locks.get(&key) {
            if l.amount > 0 && current_time < l.unlock_time {
                return Err(TokenFactoryError::ActiveLockExists);
            }
        }

        self.token_locks.insert(key, LockEntry { amount, unlock_time });
        Ok(())
    }

    /// `extendLock(account, new_amount, new_unlock_time)` — grow an existing active lock.
    ///
    /// Both `new_amount` and `new_unlock_time` must be `>=` their current values.
    pub fn extend_lock(
        &mut self,
        token:           Address,
        caller:          Address,
        account:         Address,
        new_amount:      u128,
        new_unlock_time: u64,
        current_time:    u64,
        account_balance: u128,
    ) -> Result<(), TokenFactoryError> {
        let record = self.tokens.get(&token)
            .ok_or(TokenFactoryError::TokenNotFound)?;
        if record.owner != caller { return Err(TokenFactoryError::Unauthorized); }

        let key = (token, account);
        let l = self.token_locks.get(&key).cloned()
            .ok_or(TokenFactoryError::LockNotFound)?;
        if l.amount == 0                   { return Err(TokenFactoryError::LockNotFound); }
        if current_time >= l.unlock_time   { return Err(TokenFactoryError::LockExpired); }
        if new_amount     < l.amount       { return Err(TokenFactoryError::LockMustGrow); }
        if new_unlock_time < l.unlock_time { return Err(TokenFactoryError::LockMustGrow); }
        if account_balance < new_amount    {
            return Err(TokenFactoryError::InsufficientBalanceForLock {
                have: account_balance,
                need: new_amount,
            });
        }

        self.token_locks.insert(key, LockEntry {
            amount:      new_amount,
            unlock_time: new_unlock_time,
        });
        Ok(())
    }

    /// `lockedBalanceOf(account)` — auto-zero once `unlock_time` has passed.
    pub fn locked_balance_of(
        &self,
        token:        Address,
        account:      Address,
        current_time: u64,
    ) -> u128 {
        match self.token_locks.get(&(token, account)) {
            Some(l) if l.amount > 0 && current_time < l.unlock_time => l.amount,
            _ => 0,
        }
    }

    /// `transferableBalance(account)` — `balance - lockedBalanceOf(account)`, saturating at 0.
    pub fn transferable_balance(
        &self,
        token:        Address,
        account:      Address,
        current_time: u64,
        balance:      u128,
    ) -> u128 {
        let locked = self.locked_balance_of(token, account, current_time);
        balance.saturating_sub(locked)
    }

    /// `lockInfo(account)` — raw lock data. Returns `(0, 0)` if never locked.
    pub fn lock_info(
        &self,
        token:   Address,
        account: Address,
    ) -> (u128, u64) {
        match self.token_locks.get(&(token, account)) {
            Some(l) => (l.amount, l.unlock_time),
            None    => (0, 0),
        }
    }

    // ── Metadata registration ─────────────────────────────────────────────────

    /// Register metadata URI (icon, website, description). Costs `metadata_fee`.
    pub fn register_metadata(
        &mut self,
        token:              Address,
        caller:             Address,
        metadata_uri:       String,
        caller_zbx_balance: u128,
    ) -> Result<(), TokenFactoryError> {
        let fee = self.fee_registry.metadata_registration_fee();
        if caller_zbx_balance < fee {
            return Err(TokenFactoryError::InsufficientFee {
                have: caller_zbx_balance,
                need: fee,
            });
        }
        let record = self.tokens.get_mut(&token)
            .ok_or(TokenFactoryError::TokenNotFound)?;
        if record.owner != caller {
            return Err(TokenFactoryError::Unauthorized);
        }
        record.metadata_uri = Some(metadata_uri);
        self.treasury_balance = self.treasury_balance.saturating_add(fee);
        Ok(())
    }

    /// `updateLogoURI(newURI)` — update the on-chain logo URI. Owner only. No fee.
    ///
    /// Returns the old URI for event logging (mirrors `LogoURIUpdated(old, new)`).
    pub fn update_logo_uri(
        &mut self,
        token:   Address,
        caller:  Address,
        new_uri: String,
    ) -> Result<Option<String>, TokenFactoryError> {
        let record = self.tokens.get_mut(&token)
            .ok_or(TokenFactoryError::TokenNotFound)?;
        if record.owner != caller { return Err(TokenFactoryError::Unauthorized); }
        let old = record.logo_uri.replace(new_uri);
        Ok(old)
    }

    // ── Queries ────────────────────────────────────────────────────────────────

    pub fn get_token(&self, addr: &Address) -> Option<&TokenRecord> {
        self.tokens.get(addr)
    }

    pub fn find_by_symbol(&self, symbol: &str) -> Option<&TokenRecord> {
        self.symbol_index.get(symbol)
            .and_then(|addr| self.tokens.get(addr))
    }

    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }

    pub fn fee_registry(&self) -> &FeeRegistry {
        &self.fee_registry
    }

    pub fn all_tokens(&self) -> Vec<&TokenRecord> {
        self.tokens.values().collect()
    }
}

impl Default for TokenFactory {
    fn default() -> Self { Self::new() }
}

// ── Address derivation ─────────────────────────────────────────────────────────

/// Deterministic token address: first 20 bytes of SHA3-256(creator_addr || nonce_le8).
fn derive_token_address(creator: Address, nonce: u64) -> Address {
    let mut h = Sha3_256::new();
    h.update(&creator.0);
    h.update(nonce.to_le_bytes());
    let digest = h.finalize();
    let mut bytes = [0u8; 20];
    bytes.copy_from_slice(&digest[..20]);
    Address(bytes)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(n: u8) -> Address { Address([n; 20]) }

    fn default_params(creator: Address, zbx: u128) -> CreateTokenParams {
        let fee = FeeRegistry::default().token_creation_fee();
        CreateTokenParams {
            name:                "Test Token".into(),
            symbol:              "TEST".into(),
            decimals:            18,
            total_supply:        1_000_000 * 10u128.pow(18),
            max_supply:          0,
            mintable:            true,
            creator,
            creator_zbx_balance: zbx.max(fee),
            block_number:        1,
            logo_uri:            Some("ipfs://test".into()),
        }
    }

    const T0: u64 = 1_000_000; // base timestamp (seconds)

    #[test]
    fn create_token_success() {
        let mut f = TokenFactory::new();
        let fee = f.fee_registry().token_creation_fee();
        let p = default_params(addr(1), fee);
        let tok = f.create_token(p).unwrap();
        assert_ne!(tok, addr(0));
        assert_eq!(f.token_count(), 1);
        assert_eq!(f.treasury_balance, fee);
    }

    #[test]
    fn symbol_uniqueness_enforced() {
        let mut f = TokenFactory::new();
        let fee = f.fee_registry().token_creation_fee();
        f.create_token(default_params(addr(1), fee)).unwrap();
        let r = f.create_token(default_params(addr(2), fee));
        assert!(matches!(r, Err(TokenFactoryError::SymbolAlreadyExists)));
    }

    #[test]
    fn insufficient_fee_rejected() {
        let mut f = TokenFactory::new();
        let mut p = default_params(addr(1), 0);
        p.creator_zbx_balance = 0;
        let r = f.create_token(p);
        assert!(matches!(r, Err(TokenFactoryError::InsufficientFee { .. })));
    }

    #[test]
    fn mint_increases_supply() {
        let mut f = TokenFactory::new();
        let fee = f.fee_registry().token_creation_fee();
        let mint_fee = f.fee_registry().token_mint_fee();
        let tok = f.create_token(default_params(addr(1), fee)).unwrap();
        let supply_before = f.get_token(&tok).unwrap().total_supply;
        f.mint(tok, addr(1), 1_000, mint_fee + 100, 2).unwrap();
        assert_eq!(f.get_token(&tok).unwrap().total_supply, supply_before + 1_000);
    }

    #[test]
    fn pause_blocks_unmock_check() {
        let mut f = TokenFactory::new();
        let fee = f.fee_registry().token_creation_fee();
        let pause_fee = f.fee_registry().token_pause_fee();
        let tok = f.create_token(default_params(addr(1), fee)).unwrap();
        f.pause_token(tok, addr(1), pause_fee + 100).unwrap();
        assert!(f.get_token(&tok).unwrap().paused);
        f.unpause_token(tok, addr(1)).unwrap();
        assert!(!f.get_token(&tok).unwrap().paused);
    }

    #[test]
    fn deterministic_address_per_creator() {
        let a1 = derive_token_address(Address([1u8; 20]), 0);
        let a2 = derive_token_address(Address([1u8; 20]), 0);
        assert_eq!(a1, a2);
        let a3 = derive_token_address(Address([1u8; 20]), 1);
        assert_ne!(a1, a3);
    }

    // ── ZEP-006 v1.1 tests ────────────────────────────────────────────────────

    #[test]
    fn pause_minting_blocks_mint() {
        let mut f = TokenFactory::new();
        let fee = f.fee_registry().token_creation_fee();
        let mint_fee = f.fee_registry().token_mint_fee();
        let tok = f.create_token(default_params(addr(1), fee)).unwrap();
        f.pause_minting(tok, addr(1)).unwrap();
        let r = f.mint(tok, addr(1), 100, mint_fee, 2);
        assert!(matches!(r, Err(TokenFactoryError::MintingPaused)));
    }

    #[test]
    fn resume_minting_restores_mint() {
        let mut f = TokenFactory::new();
        let fee = f.fee_registry().token_creation_fee();
        let mint_fee = f.fee_registry().token_mint_fee();
        let tok = f.create_token(default_params(addr(1), fee)).unwrap();
        f.pause_minting(tok, addr(1)).unwrap();
        f.resume_minting(tok, addr(1)).unwrap();
        f.mint(tok, addr(1), 100, mint_fee, 2).unwrap();
        assert_eq!(
            f.get_token(&tok).unwrap().total_supply,
            1_000_000 * 10u128.pow(18) + 100
        );
    }

    #[test]
    fn finalize_minting_permanent() {
        let mut f = TokenFactory::new();
        let fee = f.fee_registry().token_creation_fee();
        let mint_fee = f.fee_registry().token_mint_fee();
        let tok = f.create_token(default_params(addr(1), fee)).unwrap();
        f.finalize_minting(tok, addr(1)).unwrap();
        // Further mint, pause, resume all blocked.
        assert!(matches!(f.mint(tok, addr(1), 100, mint_fee, 2), Err(TokenFactoryError::MintingFinalized)));
        assert!(matches!(f.pause_minting(tok, addr(1)), Err(TokenFactoryError::MintingFinalized)));
        assert!(matches!(f.resume_minting(tok, addr(1)), Err(TokenFactoryError::MintingFinalized)));
        assert!(matches!(f.finalize_minting(tok, addr(1)), Err(TokenFactoryError::MintingAlreadyFinalized)));
    }

    #[test]
    fn freeze_account_blocks_is_frozen_view() {
        let mut f = TokenFactory::new();
        let fee = f.fee_registry().token_creation_fee();
        let tok = f.create_token(default_params(addr(1), fee)).unwrap();
        assert!(!f.is_frozen(tok, addr(2)));
        f.freeze_account(tok, addr(1), addr(2)).unwrap();
        assert!(f.is_frozen(tok, addr(2)));
    }

    #[test]
    fn freeze_already_frozen_reverts() {
        let mut f = TokenFactory::new();
        let fee = f.fee_registry().token_creation_fee();
        let tok = f.create_token(default_params(addr(1), fee)).unwrap();
        f.freeze_account(tok, addr(1), addr(2)).unwrap();
        let r = f.freeze_account(tok, addr(1), addr(2));
        assert!(matches!(r, Err(TokenFactoryError::AccountAlreadyFrozen)));
    }

    #[test]
    fn unfreeze_account_clears_frozen() {
        let mut f = TokenFactory::new();
        let fee = f.fee_registry().token_creation_fee();
        let tok = f.create_token(default_params(addr(1), fee)).unwrap();
        f.freeze_account(tok, addr(1), addr(2)).unwrap();
        f.unfreeze_account(tok, addr(1), addr(2)).unwrap();
        assert!(!f.is_frozen(tok, addr(2)));
    }

    #[test]
    fn frozen_balance_view() {
        let mut f = TokenFactory::new();
        let fee = f.fee_registry().token_creation_fee();
        let tok = f.create_token(default_params(addr(1), fee)).unwrap();
        assert_eq!(f.frozen_balance(tok, addr(2), 500), 0);
        f.freeze_account(tok, addr(1), addr(2)).unwrap();
        assert_eq!(f.frozen_balance(tok, addr(2), 500), 500);
    }

    #[test]
    fn lock_tokens_success() {
        let mut f = TokenFactory::new();
        let fee = f.fee_registry().token_creation_fee();
        let tok = f.create_token(default_params(addr(1), fee)).unwrap();
        f.lock_tokens(tok, addr(1), addr(2), 300, T0 + 3600, T0, 1_000).unwrap();
        assert_eq!(f.locked_balance_of(tok, addr(2), T0), 300);
    }

    #[test]
    fn locked_balance_auto_expires() {
        let mut f = TokenFactory::new();
        let fee = f.fee_registry().token_creation_fee();
        let tok = f.create_token(default_params(addr(1), fee)).unwrap();
        f.lock_tokens(tok, addr(1), addr(2), 300, T0 + 100, T0, 1_000).unwrap();
        assert_eq!(f.locked_balance_of(tok, addr(2), T0 + 100), 0);
        assert_eq!(f.locked_balance_of(tok, addr(2), T0 + 200), 0);
    }

    #[test]
    fn transferable_balance_respects_lock() {
        let mut f = TokenFactory::new();
        let fee = f.fee_registry().token_creation_fee();
        let tok = f.create_token(default_params(addr(1), fee)).unwrap();
        f.lock_tokens(tok, addr(1), addr(2), 700, T0 + 3600, T0, 1_000).unwrap();
        assert_eq!(f.transferable_balance(tok, addr(2), T0, 1_000), 300);
    }

    #[test]
    fn extend_lock_works() {
        let mut f = TokenFactory::new();
        let fee = f.fee_registry().token_creation_fee();
        let tok = f.create_token(default_params(addr(1), fee)).unwrap();
        f.lock_tokens(tok, addr(1), addr(2), 300, T0 + 1000, T0, 2_000).unwrap();
        f.extend_lock(tok, addr(1), addr(2), 600, T0 + 2000, T0, 2_000).unwrap();
        assert_eq!(f.lock_info(tok, addr(2)), (600, T0 + 2000));
    }

    #[test]
    fn active_lock_blocks_new_lock() {
        let mut f = TokenFactory::new();
        let fee = f.fee_registry().token_creation_fee();
        let tok = f.create_token(default_params(addr(1), fee)).unwrap();
        f.lock_tokens(tok, addr(1), addr(2), 300, T0 + 3600, T0, 1_000).unwrap();
        let r = f.lock_tokens(tok, addr(1), addr(2), 100, T0 + 7200, T0, 1_000);
        assert!(matches!(r, Err(TokenFactoryError::ActiveLockExists)));
    }

    #[test]
    fn replace_lock_after_expiry() {
        let mut f = TokenFactory::new();
        let fee = f.fee_registry().token_creation_fee();
        let tok = f.create_token(default_params(addr(1), fee)).unwrap();
        f.lock_tokens(tok, addr(1), addr(2), 300, T0 + 100, T0, 1_000).unwrap();
        // After expiry, fresh lock is allowed.
        f.lock_tokens(tok, addr(1), addr(2), 100, T0 + 200 + 50, T0 + 200, 1_000).unwrap();
        assert_eq!(f.lock_info(tok, addr(2)), (100, T0 + 250));
    }

    #[test]
    fn update_logo_uri() {
        let mut f = TokenFactory::new();
        let fee = f.fee_registry().token_creation_fee();
        let tok = f.create_token(default_params(addr(1), fee)).unwrap();
        let old = f.update_logo_uri(tok, addr(1), "ipfs://new".into()).unwrap();
        assert_eq!(old, Some("ipfs://test".into()));
        assert_eq!(f.get_token(&tok).unwrap().logo_uri, Some("ipfs://new".into()));
    }

    #[test]
    fn logo_uri_stored_at_creation() {
        let mut f = TokenFactory::new();
        let fee = f.fee_registry().token_creation_fee();
        let tok = f.create_token(default_params(addr(1), fee)).unwrap();
        assert_eq!(f.get_token(&tok).unwrap().logo_uri, Some("ipfs://test".into()));
    }
}
