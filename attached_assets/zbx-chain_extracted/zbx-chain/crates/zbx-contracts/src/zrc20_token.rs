//! ZRC-20 v1.1 single-token state engine — Rust mirror of `ZRC20Token.sol`.
//!
//! Implements the full ZRC-20 v1.1 feature surface defined in ZEP-006:
//!   - ERC-20 core (balanceOf, transfer, transferFrom, approve, allowance)
//!   - ZRC-20 extensions (batchTransfer, permit-nonce tracking, tokenInfo)
//!   - Mintable (role-gated mint with cap, `addMinter`/`removeMinter`)
//!   - Burnable (burn, burnFrom, totalBurned)
//!   - Mint enable/disable (`mintingPaused` toggleable, `mintingFinalized` one-way)
//!   - Freeze / USDC-style blacklist (`IZRC20Freezable`)
//!   - Native per-account time-lock (`IZRC20Lockable`)
//!   - Transfer pause (emergency stop)
//!   - Anti-bot max-transfer cap
//!   - 2-step ownership transfer
//!   - Logo URI update (emitted as `LogoURIUpdated` equivalent)
//!
//! ## Hook coverage (mirrors `_beforeTransfer` in ZRC20Token.sol)
//!
//! Every balance movement — transfer, mint, burn, batchTransfer — calls
//! `before_transfer` which enforces: pause ▸ freeze ▸ native lock ▸ anti-bot.
//!
//! ## Decimal convention
//!
//! All amounts are in base units (e.g. 10^18 per token for 18 decimals).
//! The engine does NOT scale — callers must pre-scale.

use std::collections::{HashMap, HashSet};
use zbx_types::address::Address;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Default decimal places (ERC-20 standard).
pub const DEFAULT_DECIMALS: u8 = 18;

/// Maximum recipients in a single batchTransfer (mirrors ZRC20Base.sol).
pub const MAX_BATCH_SIZE: usize = 512;

/// Sentinel "unlimited" mint cap value (equivalent to Solidity type(uint256).max — capped to u128 max).
pub const UNLIMITED_CAP: u128 = u128::MAX;

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Zrc20Error {
    /// address(0) used where forbidden.
    ZeroAddress,
    /// Caller is not the token owner.
    NotOwner,
    /// Caller is not a registered minter.
    NotMinter,
    /// Holder balance is below the requested amount.
    InsufficientBalance { have: u128, need: u128 },
    /// Spender allowance is below the requested amount.
    InsufficientAllowance { have: u128, need: u128 },
    /// Mint would push totalSupply above mintCap.
    MintCapExceeded { cap: u128, would_be: u128 },
    /// `mintingPaused == true` — temporary mint pause active.
    MintingPaused,
    /// `mintingFinalized == true` — permanent mint kill-switch engaged.
    MintingFinalized,
    /// `paused == true` — all transfers emergency-halted.
    TransferPaused,
    /// From-account or to-account is frozen (USDC-style).
    AccountFrozen { account: Address },
    /// Sender's locked balance blocks the outgoing transfer.
    TokensLocked { transferable: u128 },
    /// `value > maxTransferAmount` (anti-bot guard).
    ExceedsMaxTransfer { value: u128, max: u128 },
    /// batchTransfer: `to` and `values` arrays have different lengths.
    BatchLengthMismatch,
    /// batchTransfer: empty arrays are rejected.
    BatchEmpty,
    /// batchTransfer: more than MAX_BATCH_SIZE recipients.
    BatchTooLarge { max: usize },
    /// freeze() called on an already-frozen account.
    AlreadyFrozen,
    /// unfreeze() called on an account that is not frozen.
    NotFrozen,
    /// lockTokens() called while an active lock exists (use extendLock).
    ActiveLockExists,
    /// extendLock() requires new_amount >= current and new_unlock_time >= current.
    LockMustGrow,
    /// extendLock() called when no lock exists.
    LockNotFound,
    /// extendLock() called after lock has already expired.
    LockExpired,
    /// lockTokens() called with unlock_time <= current_time.
    UnlockTimeInPast,
    /// lockTokens() account balance is less than the lock amount.
    InsufficientBalanceForLock { have: u128, need: u128 },
    /// pauseMinting() when already paused.
    MintingAlreadyPaused,
    /// resumeMinting() when not paused.
    MintingNotPaused,
    /// finalizeMinting() when already finalized.
    AlreadyFinalized,
    /// addMinter() for existing minter, removeMinter() for unknown minter, etc.
    InvalidMinterOp,
    /// Mint would overflow u128 (should be unreachable in practice).
    ArithmeticOverflow,
}

impl std::fmt::Display for Zrc20Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroAddress          => write!(f, "ZRC20: zero address"),
            Self::NotOwner             => write!(f, "ZRC20: not owner"),
            Self::NotMinter            => write!(f, "ZRC20: not minter"),
            Self::InsufficientBalance  { have, need } =>
                write!(f, "ZRC20: insufficient balance (have {have}, need {need})"),
            Self::InsufficientAllowance { have, need } =>
                write!(f, "ZRC20: insufficient allowance (have {have}, need {need})"),
            Self::MintCapExceeded      { cap, would_be } =>
                write!(f, "ZRC20: mint cap exceeded (cap {cap}, would be {would_be})"),
            Self::MintingPaused        => write!(f, "ZRC20: minting paused"),
            Self::MintingFinalized     => write!(f, "ZRC20: minting finalized"),
            Self::TransferPaused       => write!(f, "ZRC20: transfers paused"),
            Self::AccountFrozen        { account } =>
                write!(f, "ZRC20: account {:?} is frozen", account),
            Self::TokensLocked         { transferable } =>
                write!(f, "ZRC20: tokens locked (transferable: {transferable})"),
            Self::ExceedsMaxTransfer   { value, max } =>
                write!(f, "ZRC20: exceeds max transfer ({value} > {max})"),
            Self::BatchLengthMismatch  => write!(f, "ZRC20: batch length mismatch"),
            Self::BatchEmpty           => write!(f, "ZRC20: empty batch"),
            Self::BatchTooLarge        { max } =>
                write!(f, "ZRC20: batch too large (max {max})"),
            Self::AlreadyFrozen        => write!(f, "ZRC20: already frozen"),
            Self::NotFrozen            => write!(f, "ZRC20: not frozen"),
            Self::ActiveLockExists     => write!(f, "ZRC20: active lock — use extendLock"),
            Self::LockMustGrow         => write!(f, "ZRC20: new lock params must be >= current"),
            Self::LockNotFound         => write!(f, "ZRC20: no lock found"),
            Self::LockExpired          => write!(f, "ZRC20: lock already expired"),
            Self::UnlockTimeInPast     => write!(f, "ZRC20: unlock time is in the past"),
            Self::InsufficientBalanceForLock { have, need } =>
                write!(f, "ZRC20: balance {have} < lock amount {need}"),
            Self::MintingAlreadyPaused => write!(f, "ZRC20: minting already paused"),
            Self::MintingNotPaused     => write!(f, "ZRC20: minting not paused"),
            Self::AlreadyFinalized     => write!(f, "ZRC20: minting already finalized"),
            Self::InvalidMinterOp      => write!(f, "ZRC20: invalid minter operation"),
            Self::ArithmeticOverflow   => write!(f, "ZRC20: arithmetic overflow"),
        }
    }
}

impl std::error::Error for Zrc20Error {}

// ── Supporting types ──────────────────────────────────────────────────────────

/// Per-account native time-lock (mirrors `LockInfo` struct in ZRC20Token.sol).
#[derive(Debug, Clone, Default)]
pub struct LockInfo {
    /// Base-unit amount that is locked. 0 = no active lock.
    pub amount: u128,
    /// Unix-seconds timestamp at which the lock expires. 0 = never locked.
    pub unlock_time: u64,
}

/// Snapshot of all publicly-visible token metadata (mirrors `tokenInfo()` view).
#[derive(Debug, Clone)]
pub struct TokenInfo {
    pub name:           String,
    pub symbol:         String,
    pub decimals:       u8,
    pub total_supply:   u128,
    pub owner:          Address,
    pub logo_uri:       String,
}

// ── Zrc20Token ────────────────────────────────────────────────────────────────

/// ZRC-20 v1.1 single-token state machine (deploy-once per token instance).
///
/// Closely mirrors `ZRC20Token.sol`: same semantics, same hook ordering,
/// same error conditions — expressed in Rust for the native ZBX Chain runtime.
#[derive(Debug)]
pub struct Zrc20Token {
    // ── Metadata ──────────────────────────────────────────────────────────────
    name:    String,
    symbol:  String,
    decimals: u8,
    logo_uri: String,

    // ── Supply tracking ───────────────────────────────────────────────────────
    total_supply:   u128,
    total_burned:   u128,
    /// 0 stored internally as UNLIMITED_CAP (u128::MAX).
    mint_cap:       u128,

    // ── Core ERC-20 state ─────────────────────────────────────────────────────
    balances:   HashMap<Address, u128>,
    allowances: HashMap<(Address, Address), u128>,

    // ── Access control ────────────────────────────────────────────────────────
    owner:         Address,
    pending_owner: Option<Address>,
    minters:       HashSet<Address>,

    // ── Mint enable/disable (ZEP-006 §3.3) ───────────────────────────────────
    /// Temporary pause — toggleable by owner; cannot be set if finalized.
    pub minting_paused:    bool,
    /// Permanent kill switch — once true, can NEVER be false again.
    pub minting_finalized: bool,

    // ── Transfer pause (emergency stop) ──────────────────────────────────────
    pub transfer_paused: bool,

    // ── Anti-bot: max transfer per tx (0 = disabled) ─────────────────────────
    pub max_transfer_amount: u128,

    // ── Freeze: USDC-style compliance (ZEP-006 §3.1) ─────────────────────────
    frozen: HashSet<Address>,

    // ── Native time-lock (ZEP-006 §3.2) ──────────────────────────────────────
    locks: HashMap<Address, LockInfo>,
}

impl Zrc20Token {
    // ── Constructor ───────────────────────────────────────────────────────────

    /// Create a new ZRC-20 v1.1 token. Mirrors `ZRC20Token` constructor.
    ///
    /// - `initial_supply > 0` is minted to `owner` immediately (fixes the old
    ///   factory bug where a post-deploy `mint()` call always reverted).
    /// - `mint_cap == 0` is stored internally as `UNLIMITED_CAP`.
    /// - `current_time` is only needed if `initial_supply > 0` (passes through
    ///   `before_transfer`); pass `0` when `initial_supply == 0`.
    pub fn new(
        name:           String,
        symbol:         String,
        decimals:       u8,
        initial_supply: u128,
        mint_cap:       u128,
        logo_uri:       String,
        owner:          Address,
        current_time:   u64,
    ) -> Result<Self, Zrc20Error> {
        if owner == Address::default() {
            return Err(Zrc20Error::ZeroAddress);
        }
        let resolved_cap = if mint_cap == 0 { UNLIMITED_CAP } else { mint_cap };
        if initial_supply > resolved_cap {
            return Err(Zrc20Error::MintCapExceeded {
                cap:      resolved_cap,
                would_be: initial_supply,
            });
        }

        let mut token = Zrc20Token {
            name,
            symbol,
            decimals,
            logo_uri,
            total_supply:       0,
            total_burned:       0,
            mint_cap:           resolved_cap,
            balances:           HashMap::new(),
            allowances:         HashMap::new(),
            owner,
            pending_owner:      None,
            minters:            HashSet::new(),
            minting_paused:     false,
            minting_finalized:  false,
            transfer_paused:    false,
            max_transfer_amount: 0,
            frozen:             HashSet::new(),
            locks:              HashMap::new(),
        };

        token.minters.insert(owner);

        if initial_supply > 0 {
            token.internal_mint(owner, initial_supply, current_time)?;
        }

        Ok(token)
    }

    // ── IZRC20 Core views ─────────────────────────────────────────────────────

    pub fn name(&self)         -> &str  { &self.name }
    pub fn symbol(&self)       -> &str  { &self.symbol }
    pub fn decimals(&self)     -> u8    { self.decimals }
    pub fn total_supply(&self) -> u128  { self.total_supply }
    pub fn logo_uri(&self)     -> &str  { &self.logo_uri }
    pub fn owner(&self)        -> Address { self.owner }
    pub fn mint_cap(&self)     -> u128  { self.mint_cap }
    pub fn total_burned(&self) -> u128  { self.total_burned }

    pub fn balance_of(&self, account: &Address) -> u128 {
        self.balances.get(account).copied().unwrap_or(0)
    }

    pub fn allowance(&self, owner: &Address, spender: &Address) -> u128 {
        self.allowances.get(&(*owner, *spender)).copied().unwrap_or(0)
    }

    pub fn token_info(&self) -> TokenInfo {
        TokenInfo {
            name:         self.name.clone(),
            symbol:       self.symbol.clone(),
            decimals:     self.decimals,
            total_supply: self.total_supply,
            owner:        self.owner,
            logo_uri:     self.logo_uri.clone(),
        }
    }

    // ── ERC-20 mutators ───────────────────────────────────────────────────────

    /// `transfer(to, value)` — sends `value` tokens from `from` to `to`.
    pub fn transfer(
        &mut self,
        from:         Address,
        to:           Address,
        value:        u128,
        current_time: u64,
    ) -> Result<(), Zrc20Error> {
        self.internal_transfer(from, to, value, current_time)
    }

    /// `approve(spender, value)` — sets allowance for `spender` on behalf of `owner`.
    pub fn approve(
        &mut self,
        owner:   Address,
        spender: Address,
        value:   u128,
    ) -> Result<(), Zrc20Error> {
        if owner   == Address::default() { return Err(Zrc20Error::ZeroAddress); }
        if spender == Address::default() { return Err(Zrc20Error::ZeroAddress); }
        self.allowances.insert((owner, spender), value);
        Ok(())
    }

    /// `transferFrom(from, to, value)` — spender spends allowance granted by `from`.
    pub fn transfer_from(
        &mut self,
        spender:      Address,
        from:         Address,
        to:           Address,
        value:        u128,
        current_time: u64,
    ) -> Result<(), Zrc20Error> {
        self.spend_allowance(from, spender, value)?;
        self.internal_transfer(from, to, value, current_time)
    }

    /// `batchTransfer(to[], values[])` — sends to up to MAX_BATCH_SIZE recipients.
    ///
    /// Every individual leg passes through `internal_transfer` so `before_transfer`
    /// (pause + freeze + lock + anti-bot) fires on each leg. (ZEP-006 CRIT-1 fix.)
    pub fn batch_transfer(
        &mut self,
        from:         Address,
        to:           &[Address],
        values:       &[u128],
        current_time: u64,
    ) -> Result<(), Zrc20Error> {
        if to.len() != values.len() { return Err(Zrc20Error::BatchLengthMismatch); }
        if to.is_empty()            { return Err(Zrc20Error::BatchEmpty); }
        if to.len() > MAX_BATCH_SIZE {
            return Err(Zrc20Error::BatchTooLarge { max: MAX_BATCH_SIZE });
        }
        for (recipient, &value) in to.iter().zip(values.iter()) {
            self.internal_transfer(from, *recipient, value, current_time)?;
        }
        Ok(())
    }

    // ── IZRC20Mintable ────────────────────────────────────────────────────────

    /// `mint(to, value)` — minter-gated; checks `minting_paused` and `minting_finalized`.
    pub fn mint(
        &mut self,
        caller:       Address,
        to:           Address,
        value:        u128,
        current_time: u64,
    ) -> Result<(), Zrc20Error> {
        if !self.minters.contains(&caller) { return Err(Zrc20Error::NotMinter); }
        if self.minting_finalized          { return Err(Zrc20Error::MintingFinalized); }
        if self.minting_paused             { return Err(Zrc20Error::MintingPaused); }
        self.internal_mint(to, value, current_time)
    }

    pub fn is_minter(&self, account: &Address) -> bool {
        self.minters.contains(account)
    }

    pub fn add_minter(&mut self, caller: Address, account: Address) -> Result<(), Zrc20Error> {
        if caller  != self.owner          { return Err(Zrc20Error::NotOwner); }
        if account == Address::default()  { return Err(Zrc20Error::ZeroAddress); }
        self.minters.insert(account);
        Ok(())
    }

    pub fn remove_minter(&mut self, caller: Address, account: Address) -> Result<(), Zrc20Error> {
        if caller != self.owner { return Err(Zrc20Error::NotOwner); }
        self.minters.remove(&account);
        Ok(())
    }

    // ── IZRC20Burnable ────────────────────────────────────────────────────────

    /// `burn(value)` — holder burns their own tokens.
    pub fn burn(
        &mut self,
        from:         Address,
        value:        u128,
        current_time: u64,
    ) -> Result<(), Zrc20Error> {
        self.internal_burn(from, value, current_time)
    }

    /// `burnFrom(from, value)` — spender burns on behalf of `from`.
    pub fn burn_from(
        &mut self,
        spender:      Address,
        from:         Address,
        value:        u128,
        current_time: u64,
    ) -> Result<(), Zrc20Error> {
        self.spend_allowance(from, spender, value)?;
        self.internal_burn(from, value, current_time)
    }

    // ── Mint enable/disable (ZEP-006 §3.3) ───────────────────────────────────

    /// `pauseMinting()` — temporarily disable all minting. Reversible.
    ///
    /// Reverts if already finalized or already paused.
    pub fn pause_minting(&mut self, caller: Address) -> Result<(), Zrc20Error> {
        if caller != self.owner           { return Err(Zrc20Error::NotOwner); }
        if self.minting_finalized         { return Err(Zrc20Error::MintingFinalized); }
        if self.minting_paused            { return Err(Zrc20Error::MintingAlreadyPaused); }
        self.minting_paused = true;
        Ok(())
    }

    /// `resumeMinting()` — lift the temporary mint pause.
    ///
    /// Reverts if finalized or if not currently paused.
    pub fn resume_minting(&mut self, caller: Address) -> Result<(), Zrc20Error> {
        if caller != self.owner    { return Err(Zrc20Error::NotOwner); }
        if self.minting_finalized  { return Err(Zrc20Error::MintingFinalized); }
        if !self.minting_paused    { return Err(Zrc20Error::MintingNotPaused); }
        self.minting_paused = false;
        Ok(())
    }

    /// `finalizeMinting()` — permanent one-way kill switch. Cannot be undone.
    pub fn finalize_minting(&mut self, caller: Address) -> Result<(), Zrc20Error> {
        if caller != self.owner    { return Err(Zrc20Error::NotOwner); }
        if self.minting_finalized  { return Err(Zrc20Error::AlreadyFinalized); }
        self.minting_finalized = true;
        Ok(())
    }

    // ── Transfer pause ────────────────────────────────────────────────────────

    pub fn pause_transfers(&mut self, caller: Address) -> Result<(), Zrc20Error> {
        if caller != self.owner { return Err(Zrc20Error::NotOwner); }
        self.transfer_paused = true;
        Ok(())
    }

    pub fn unpause_transfers(&mut self, caller: Address) -> Result<(), Zrc20Error> {
        if caller != self.owner { return Err(Zrc20Error::NotOwner); }
        self.transfer_paused = false;
        Ok(())
    }

    // ── Anti-bot ──────────────────────────────────────────────────────────────

    pub fn set_max_transfer_amount(&mut self, caller: Address, amount: u128) -> Result<(), Zrc20Error> {
        if caller != self.owner { return Err(Zrc20Error::NotOwner); }
        self.max_transfer_amount = amount;
        Ok(())
    }

    // ── IZRC20Freezable (ZEP-006 §3.1) ───────────────────────────────────────

    /// `freeze(account)` — USDC-style blacklist. Blocks all send/receive/mint/burn.
    pub fn freeze(&mut self, caller: Address, account: Address) -> Result<(), Zrc20Error> {
        if caller  != self.owner         { return Err(Zrc20Error::NotOwner); }
        if account == Address::default() { return Err(Zrc20Error::ZeroAddress); }
        if self.frozen.contains(&account) { return Err(Zrc20Error::AlreadyFrozen); }
        self.frozen.insert(account);
        Ok(())
    }

    /// `unfreeze(account)` — lift the freeze.
    pub fn unfreeze(&mut self, caller: Address, account: Address) -> Result<(), Zrc20Error> {
        if caller != self.owner { return Err(Zrc20Error::NotOwner); }
        if !self.frozen.contains(&account) { return Err(Zrc20Error::NotFrozen); }
        self.frozen.remove(&account);
        Ok(())
    }

    /// `isFrozen(account)` — view.
    pub fn is_frozen(&self, account: &Address) -> bool {
        self.frozen.contains(account)
    }

    /// `frozenBalance(account)` — returns full balance if frozen, else 0.
    pub fn frozen_balance(&self, account: &Address) -> u128 {
        if self.frozen.contains(account) {
            self.balance_of(account)
        } else {
            0
        }
    }

    // ── IZRC20Lockable (ZEP-006 §3.2) ────────────────────────────────────────

    /// `lockTokens(account, amount, unlock_time)` — place a fresh lock or replace an expired one.
    ///
    /// - `current_time`: current Unix timestamp in seconds.
    /// - Reverts if a lock is active (call `extend_lock` instead).
    pub fn lock_tokens(
        &mut self,
        caller:       Address,
        account:      Address,
        amount:       u128,
        unlock_time:  u64,
        current_time: u64,
    ) -> Result<(), Zrc20Error> {
        if caller  != self.owner         { return Err(Zrc20Error::NotOwner); }
        if account == Address::default() { return Err(Zrc20Error::ZeroAddress); }
        if amount  == 0                  { return Err(Zrc20Error::InsufficientBalanceForLock { have: 0, need: 1 }); }
        if unlock_time <= current_time   { return Err(Zrc20Error::UnlockTimeInPast); }

        let bal = self.balance_of(&account);
        if bal < amount {
            return Err(Zrc20Error::InsufficientBalanceForLock { have: bal, need: amount });
        }

        // Check for active lock (not expired).
        if let Some(l) = self.locks.get(&account) {
            if l.amount > 0 && current_time < l.unlock_time {
                return Err(Zrc20Error::ActiveLockExists);
            }
        }

        self.locks.insert(account, LockInfo { amount, unlock_time });
        Ok(())
    }

    /// `extendLock(account, new_amount, new_unlock_time)` — grow an existing active lock.
    ///
    /// Both `new_amount` and `new_unlock_time` must be `>=` the current values.
    pub fn extend_lock(
        &mut self,
        caller:          Address,
        account:         Address,
        new_amount:      u128,
        new_unlock_time: u64,
        current_time:    u64,
    ) -> Result<(), Zrc20Error> {
        if caller != self.owner { return Err(Zrc20Error::NotOwner); }

        let l = self.locks.get(&account).cloned()
            .ok_or(Zrc20Error::LockNotFound)?;

        if l.amount == 0 { return Err(Zrc20Error::LockNotFound); }
        if current_time >= l.unlock_time { return Err(Zrc20Error::LockExpired); }
        if new_amount     < l.amount     { return Err(Zrc20Error::LockMustGrow); }
        if new_unlock_time < l.unlock_time { return Err(Zrc20Error::LockMustGrow); }

        let bal = self.balance_of(&account);
        if bal < new_amount {
            return Err(Zrc20Error::InsufficientBalanceForLock { have: bal, need: new_amount });
        }

        self.locks.insert(account, LockInfo {
            amount:      new_amount,
            unlock_time: new_unlock_time,
        });
        Ok(())
    }

    /// `lockedBalanceOf(account)` — auto-zero once `unlock_time` has passed.
    pub fn locked_balance_of(&self, account: &Address, current_time: u64) -> u128 {
        match self.locks.get(account) {
            Some(l) if l.amount > 0 && current_time < l.unlock_time => l.amount,
            _ => 0,
        }
    }

    /// `transferableBalance(account)` — `balance - locked`, saturating at 0.
    pub fn transferable_balance(&self, account: &Address, current_time: u64) -> u128 {
        let bal    = self.balance_of(account);
        let locked = self.locked_balance_of(account, current_time);
        bal.saturating_sub(locked)
    }

    /// `lockInfo(account)` — raw lock data. Returns `(0, 0)` if never locked.
    pub fn lock_info(&self, account: &Address) -> (u128, u64) {
        match self.locks.get(account) {
            Some(l) => (l.amount, l.unlock_time),
            None    => (0, 0),
        }
    }

    // ── Ownership (2-step — mirrors ZRC20Token.sol) ───────────────────────────

    /// Begin a 2-step ownership transfer (pending owner must call `accept_ownership`).
    pub fn transfer_ownership(&mut self, caller: Address, new_owner: Address) -> Result<(), Zrc20Error> {
        if caller    != self.owner         { return Err(Zrc20Error::NotOwner); }
        if new_owner == Address::default() { return Err(Zrc20Error::ZeroAddress); }
        self.pending_owner = Some(new_owner);
        Ok(())
    }

    /// Complete the 2-step ownership transfer. Only callable by `pending_owner`.
    pub fn accept_ownership(&mut self, caller: Address) -> Result<(), Zrc20Error> {
        match self.pending_owner {
            Some(p) if p == caller => {
                self.owner         = caller;
                self.pending_owner = None;
                Ok(())
            }
            _ => Err(Zrc20Error::NotOwner),
        }
    }

    pub fn renounce_ownership(&mut self, caller: Address) -> Result<(), Zrc20Error> {
        if caller != self.owner { return Err(Zrc20Error::NotOwner); }
        self.owner = Address::default();
        Ok(())
    }

    pub fn pending_owner(&self) -> Option<Address> { self.pending_owner }

    // ── Logo URI update (ZEP-006 §3.5) ───────────────────────────────────────

    /// `updateLogoURI(newURI)` — persists via internal `_setLogoURI` equivalent.
    ///
    /// Caller receives the old URI for event emission (equivalent to `LogoURIUpdated`).
    pub fn update_logo_uri(
        &mut self,
        caller:  Address,
        new_uri: String,
    ) -> Result<String, Zrc20Error> {
        if caller != self.owner { return Err(Zrc20Error::NotOwner); }
        let old = std::mem::replace(&mut self.logo_uri, new_uri);
        Ok(old)  // caller emits LogoURIUpdated(old, new)
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Combined `_beforeTransfer` hook: pause ▸ freeze ▸ lock ▸ anti-bot.
    ///
    /// Mirrors the ZRC20Token._beforeTransfer override exactly:
    ///   1. Transfer pause (all movements, including mint/burn).
    ///   2. Freeze: from-frozen reverts (skip when from = 0 sentinel).
    ///              to-frozen   reverts (skip when to   = 0 sentinel).
    ///   3. Native lock: skipped when from = 0 (mint exempt per ZEP-006 §3.2).
    ///   4. Anti-bot: peer-to-peer only (mint and burn exempt).
    fn before_transfer(
        &self,
        from:         Address,
        to:           Address,
        value:        u128,
        current_time: u64,
    ) -> Result<(), Zrc20Error> {
        let zero = Address::default();

        // 1. Transfer pause.
        if self.transfer_paused { return Err(Zrc20Error::TransferPaused); }

        // 2. Freeze — address(0) is never in the set (freeze() require-gate).
        if from != zero && self.frozen.contains(&from) {
            return Err(Zrc20Error::AccountFrozen { account: from });
        }
        if to != zero && self.frozen.contains(&to) {
            return Err(Zrc20Error::AccountFrozen { account: to });
        }

        // 3. Native lock — only for outgoing (from != 0).
        if from != zero {
            let locked = self.locked_balance_of(&from, current_time);
            if locked > 0 {
                let bal = self.balance_of(&from);
                // bal is pre-debit; need: bal - locked >= value.
                if bal < locked || bal - locked < value {
                    let transferable = if bal > locked { bal - locked } else { 0 };
                    return Err(Zrc20Error::TokensLocked { transferable });
                }
            }
        }

        // 4. Anti-bot — peer-to-peer only.
        if self.max_transfer_amount > 0 && from != zero && to != zero {
            if value > self.max_transfer_amount {
                return Err(Zrc20Error::ExceedsMaxTransfer {
                    value,
                    max: self.max_transfer_amount,
                });
            }
        }

        Ok(())
    }

    fn internal_transfer(
        &mut self,
        from:         Address,
        to:           Address,
        value:        u128,
        current_time: u64,
    ) -> Result<(), Zrc20Error> {
        if from == Address::default() { return Err(Zrc20Error::ZeroAddress); }
        if to   == Address::default() { return Err(Zrc20Error::ZeroAddress); }

        self.before_transfer(from, to, value, current_time)?;

        let from_bal = self.balance_of(&from);
        if from_bal < value {
            return Err(Zrc20Error::InsufficientBalance { have: from_bal, need: value });
        }

        self.balances.insert(from, from_bal - value);
        let to_bal = self.balance_of(&to);
        self.balances.insert(to, to_bal.saturating_add(value));
        Ok(())
    }

    /// Internal mint — also called by the constructor for initial supply.
    ///
    /// `from = address(0)` sentinel passes through `before_transfer` for hook
    /// coverage on mint (ZEP-006 CRIT-2 fix): freeze on `to` is enforced,
    /// but lock and anti-bot are skipped for mint paths.
    fn internal_mint(
        &mut self,
        to:           Address,
        value:        u128,
        current_time: u64,
    ) -> Result<(), Zrc20Error> {
        if to == Address::default() { return Err(Zrc20Error::ZeroAddress); }

        let zero = Address::default();
        self.before_transfer(zero, to, value, current_time)?;

        let new_supply = self.total_supply
            .checked_add(value)
            .ok_or(Zrc20Error::ArithmeticOverflow)?;
        if new_supply > self.mint_cap {
            return Err(Zrc20Error::MintCapExceeded {
                cap:      self.mint_cap,
                would_be: new_supply,
            });
        }

        self.total_supply = new_supply;
        let bal = self.balance_of(&to);
        self.balances.insert(to, bal.saturating_add(value));
        Ok(())
    }

    /// Internal burn — `to = address(0)` sentinel fires all hook checks on `from`.
    ///
    /// Freeze on `from` blocks burn (USDC compliance).
    /// Lock on `from` blocks burning the locked portion (ZEP-006 §3.2 rationale).
    fn internal_burn(
        &mut self,
        from:         Address,
        value:        u128,
        current_time: u64,
    ) -> Result<(), Zrc20Error> {
        if from == Address::default() { return Err(Zrc20Error::ZeroAddress); }

        let zero = Address::default();
        self.before_transfer(from, zero, value, current_time)?;

        let bal = self.balance_of(&from);
        if bal < value {
            return Err(Zrc20Error::InsufficientBalance { have: bal, need: value });
        }

        self.balances.insert(from, bal - value);
        self.total_supply  = self.total_supply.saturating_sub(value);
        self.total_burned  = self.total_burned.saturating_add(value);
        Ok(())
    }

    fn spend_allowance(
        &mut self,
        owner:   Address,
        spender: Address,
        value:   u128,
    ) -> Result<(), Zrc20Error> {
        let current = self.allowance(&owner, &spender);
        if current == u128::MAX {
            return Ok(()); // Infinite allowance — no debit (mirrors Solidity).
        }
        if current < value {
            return Err(Zrc20Error::InsufficientAllowance { have: current, need: value });
        }
        self.allowances.insert((owner, spender), current - value);
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(n: u8) -> Address { Address([n; 20]) }
    const OWNER: fn() -> Address = || addr(1);
    const ALICE: fn() -> Address = || addr(2);
    const BOB:   fn() -> Address = || addr(3);
    const T0: u64 = 1_000_000; // base timestamp

    fn make_token(initial: u128, cap: u128) -> Zrc20Token {
        Zrc20Token::new(
            "Test Token".into(),
            "TEST".into(),
            18,
            initial,
            cap,
            "ipfs://test".into(),
            OWNER(),
            T0,
        ).unwrap()
    }

    // ── Constructor ───────────────────────────────────────────────────────────

    #[test]
    fn constructor_mints_initial_supply() {
        let tok = make_token(1_000, 10_000);
        assert_eq!(tok.balance_of(&OWNER()), 1_000);
        assert_eq!(tok.total_supply(), 1_000);
    }

    #[test]
    fn constructor_zero_initial_no_supply() {
        let tok = make_token(0, 10_000);
        assert_eq!(tok.total_supply(), 0);
    }

    #[test]
    fn constructor_rejects_zero_owner() {
        let r = Zrc20Token::new("T".into(), "T".into(), 18, 0, 0, "".into(), Address::default(), 0);
        assert!(matches!(r, Err(Zrc20Error::ZeroAddress)));
    }

    #[test]
    fn constructor_rejects_initial_above_cap() {
        let r = Zrc20Token::new("T".into(), "T".into(), 18, 1001, 1000, "".into(), OWNER(), 0);
        assert!(matches!(r, Err(Zrc20Error::MintCapExceeded { .. })));
    }

    // ── Transfer ──────────────────────────────────────────────────────────────

    #[test]
    fn transfer_moves_balance() {
        let mut tok = make_token(1_000, 0);
        tok.transfer(OWNER(), ALICE(), 400, T0).unwrap();
        assert_eq!(tok.balance_of(&OWNER()), 600);
        assert_eq!(tok.balance_of(&ALICE()), 400);
    }

    #[test]
    fn transfer_insufficient_balance_reverts() {
        let mut tok = make_token(100, 0);
        let r = tok.transfer(OWNER(), ALICE(), 200, T0);
        assert!(matches!(r, Err(Zrc20Error::InsufficientBalance { .. })));
    }

    // ── Approve + transferFrom ────────────────────────────────────────────────

    #[test]
    fn transfer_from_with_allowance() {
        let mut tok = make_token(1_000, 0);
        tok.approve(OWNER(), ALICE(), 500).unwrap();
        tok.transfer_from(ALICE(), OWNER(), BOB(), 300, T0).unwrap();
        assert_eq!(tok.balance_of(&BOB()), 300);
        assert_eq!(tok.allowance(&OWNER(), &ALICE()), 200);
    }

    #[test]
    fn transfer_from_insufficient_allowance() {
        let mut tok = make_token(1_000, 0);
        tok.approve(OWNER(), ALICE(), 50).unwrap();
        let r = tok.transfer_from(ALICE(), OWNER(), BOB(), 100, T0);
        assert!(matches!(r, Err(Zrc20Error::InsufficientAllowance { .. })));
    }

    // ── Batch transfer ────────────────────────────────────────────────────────

    #[test]
    fn batch_transfer_multi_recipient() {
        let mut tok = make_token(1_000, 0);
        tok.batch_transfer(OWNER(), &[ALICE(), BOB()], &[300, 200], T0).unwrap();
        assert_eq!(tok.balance_of(&ALICE()), 300);
        assert_eq!(tok.balance_of(&BOB()),   200);
        assert_eq!(tok.balance_of(&OWNER()), 500);
    }

    #[test]
    fn batch_transfer_length_mismatch() {
        let mut tok = make_token(1_000, 0);
        let r = tok.batch_transfer(OWNER(), &[ALICE()], &[100, 200], T0);
        assert!(matches!(r, Err(Zrc20Error::BatchLengthMismatch)));
    }

    #[test]
    fn batch_transfer_empty_reverts() {
        let mut tok = make_token(1_000, 0);
        let r = tok.batch_transfer(OWNER(), &[], &[], T0);
        assert!(matches!(r, Err(Zrc20Error::BatchEmpty)));
    }

    // ── Mint ──────────────────────────────────────────────────────────────────

    #[test]
    fn mint_increases_supply() {
        let mut tok = make_token(0, 10_000);
        tok.mint(OWNER(), ALICE(), 500, T0).unwrap();
        assert_eq!(tok.balance_of(&ALICE()), 500);
        assert_eq!(tok.total_supply(), 500);
    }

    #[test]
    fn mint_cap_enforced() {
        let mut tok = make_token(900, 1_000);
        let r = tok.mint(OWNER(), ALICE(), 200, T0);
        assert!(matches!(r, Err(Zrc20Error::MintCapExceeded { .. })));
    }

    #[test]
    fn mint_reverts_not_minter() {
        let mut tok = make_token(0, 0);
        let r = tok.mint(ALICE(), BOB(), 100, T0);
        assert!(matches!(r, Err(Zrc20Error::NotMinter)));
    }

    // ── Burn ──────────────────────────────────────────────────────────────────

    #[test]
    fn burn_reduces_supply_and_balance() {
        let mut tok = make_token(1_000, 0);
        tok.burn(OWNER(), 400, T0).unwrap();
        assert_eq!(tok.balance_of(&OWNER()), 600);
        assert_eq!(tok.total_supply(), 600);
        assert_eq!(tok.total_burned(), 400);
    }

    // ── Mint enable/disable (ZEP-006 §3.3) ───────────────────────────────────

    #[test]
    fn pause_minting_blocks_mint() {
        let mut tok = make_token(0, 0);
        tok.pause_minting(OWNER()).unwrap();
        let r = tok.mint(OWNER(), ALICE(), 100, T0);
        assert!(matches!(r, Err(Zrc20Error::MintingPaused)));
    }

    #[test]
    fn resume_minting_restores_mint() {
        let mut tok = make_token(0, 0);
        tok.pause_minting(OWNER()).unwrap();
        tok.resume_minting(OWNER()).unwrap();
        tok.mint(OWNER(), ALICE(), 100, T0).unwrap();
        assert_eq!(tok.balance_of(&ALICE()), 100);
    }

    #[test]
    fn finalize_minting_permanent() {
        let mut tok = make_token(0, 0);
        tok.finalize_minting(OWNER()).unwrap();
        assert!(matches!(tok.mint(OWNER(), ALICE(), 100, T0), Err(Zrc20Error::MintingFinalized)));
        assert!(matches!(tok.pause_minting(OWNER()), Err(Zrc20Error::MintingFinalized)));
        assert!(matches!(tok.resume_minting(OWNER()), Err(Zrc20Error::MintingFinalized)));
    }

    #[test]
    fn finalize_minting_already_finalized_reverts() {
        let mut tok = make_token(0, 0);
        tok.finalize_minting(OWNER()).unwrap();
        assert!(matches!(tok.finalize_minting(OWNER()), Err(Zrc20Error::AlreadyFinalized)));
    }

    // ── Freeze (ZEP-006 §3.1) ─────────────────────────────────────────────────

    #[test]
    fn freeze_blocks_send() {
        let mut tok = make_token(1_000, 0);
        tok.transfer(OWNER(), ALICE(), 500, T0).unwrap();
        tok.freeze(OWNER(), ALICE()).unwrap();
        let r = tok.transfer(ALICE(), BOB(), 100, T0);
        assert!(matches!(r, Err(Zrc20Error::AccountFrozen { .. })));
    }

    #[test]
    fn freeze_blocks_receive() {
        let mut tok = make_token(1_000, 0);
        tok.freeze(OWNER(), ALICE()).unwrap();
        let r = tok.transfer(OWNER(), ALICE(), 100, T0);
        assert!(matches!(r, Err(Zrc20Error::AccountFrozen { .. })));
    }

    #[test]
    fn freeze_blocks_mint_to() {
        let mut tok = make_token(0, 0);
        tok.freeze(OWNER(), ALICE()).unwrap();
        let r = tok.mint(OWNER(), ALICE(), 100, T0);
        assert!(matches!(r, Err(Zrc20Error::AccountFrozen { .. })));
    }

    #[test]
    fn freeze_blocks_burn_from() {
        let mut tok = make_token(1_000, 0);
        tok.transfer(OWNER(), ALICE(), 500, T0).unwrap();
        tok.freeze(OWNER(), ALICE()).unwrap();
        let r = tok.burn(ALICE(), 100, T0);
        assert!(matches!(r, Err(Zrc20Error::AccountFrozen { .. })));
    }

    #[test]
    fn unfreeze_restores_transfers() {
        let mut tok = make_token(1_000, 0);
        tok.transfer(OWNER(), ALICE(), 500, T0).unwrap();
        tok.freeze(OWNER(), ALICE()).unwrap();
        tok.unfreeze(OWNER(), ALICE()).unwrap();
        tok.transfer(ALICE(), BOB(), 100, T0).unwrap();
        assert_eq!(tok.balance_of(&BOB()), 100);
    }

    #[test]
    fn frozen_balance_view() {
        let mut tok = make_token(1_000, 0);
        tok.transfer(OWNER(), ALICE(), 400, T0).unwrap();
        assert_eq!(tok.frozen_balance(&ALICE()), 0);
        tok.freeze(OWNER(), ALICE()).unwrap();
        assert_eq!(tok.frozen_balance(&ALICE()), 400);
    }

    #[test]
    fn freeze_already_frozen_reverts() {
        let mut tok = make_token(0, 0);
        tok.freeze(OWNER(), ALICE()).unwrap();
        assert!(matches!(tok.freeze(OWNER(), ALICE()), Err(Zrc20Error::AlreadyFrozen)));
    }

    #[test]
    fn unfreeze_not_frozen_reverts() {
        let mut tok = make_token(0, 0);
        assert!(matches!(tok.unfreeze(OWNER(), ALICE()), Err(Zrc20Error::NotFrozen)));
    }

    // ── Native lock (ZEP-006 §3.2) ────────────────────────────────────────────

    #[test]
    fn lock_blocks_outgoing_transfer() {
        let mut tok = make_token(1_000, 0);
        tok.transfer(OWNER(), ALICE(), 1_000, T0).unwrap();
        tok.lock_tokens(OWNER(), ALICE(), 800, T0 + 3600, T0).unwrap();
        // Only 200 is transferable — trying 300 should revert.
        let r = tok.transfer(ALICE(), BOB(), 300, T0);
        assert!(matches!(r, Err(Zrc20Error::TokensLocked { .. })));
    }

    #[test]
    fn lock_allows_partial_transfer_of_unlocked_portion() {
        let mut tok = make_token(1_000, 0);
        tok.transfer(OWNER(), ALICE(), 1_000, T0).unwrap();
        tok.lock_tokens(OWNER(), ALICE(), 700, T0 + 3600, T0).unwrap();
        tok.transfer(ALICE(), BOB(), 300, T0).unwrap();
        assert_eq!(tok.balance_of(&BOB()), 300);
    }

    #[test]
    fn transferable_balance_math() {
        let mut tok = make_token(0, 0);
        tok.mint(OWNER(), ALICE(), 1_000, T0).unwrap();
        tok.lock_tokens(OWNER(), ALICE(), 600, T0 + 100, T0).unwrap();
        assert_eq!(tok.transferable_balance(&ALICE(), T0), 400);
        assert_eq!(tok.locked_balance_of(&ALICE(), T0), 600);
    }

    #[test]
    fn lock_auto_expires() {
        let mut tok = make_token(0, 0);
        tok.mint(OWNER(), ALICE(), 1_000, T0).unwrap();
        tok.lock_tokens(OWNER(), ALICE(), 800, T0 + 100, T0).unwrap();
        // After unlock_time, locked_balance_of should return 0.
        assert_eq!(tok.locked_balance_of(&ALICE(), T0 + 100), 0);
        assert_eq!(tok.locked_balance_of(&ALICE(), T0 + 200), 0);
        // And transfer should now succeed.
        tok.transfer(ALICE(), BOB(), 900, T0 + 200).unwrap();
    }

    #[test]
    fn mint_exempt_from_lock_check() {
        let mut tok = make_token(0, 0);
        tok.mint(OWNER(), ALICE(), 500, T0).unwrap();
        tok.lock_tokens(OWNER(), ALICE(), 500, T0 + 3600, T0).unwrap();
        // Minting more to Alice (from = 0) should succeed even though Alice is locked.
        tok.mint(OWNER(), ALICE(), 100, T0).unwrap();
        assert_eq!(tok.balance_of(&ALICE()), 600);
    }

    #[test]
    fn lock_blocks_burn() {
        let mut tok = make_token(0, 0);
        tok.mint(OWNER(), ALICE(), 1_000, T0).unwrap();
        tok.lock_tokens(OWNER(), ALICE(), 1_000, T0 + 3600, T0).unwrap();
        let r = tok.burn(ALICE(), 100, T0);
        assert!(matches!(r, Err(Zrc20Error::TokensLocked { .. })));
    }

    #[test]
    fn extend_lock_grows_both_fields() {
        let mut tok = make_token(0, 0);
        tok.mint(OWNER(), ALICE(), 2_000, T0).unwrap();
        tok.lock_tokens(OWNER(), ALICE(), 500, T0 + 1000, T0).unwrap();
        tok.extend_lock(OWNER(), ALICE(), 1_000, T0 + 2000, T0).unwrap();
        assert_eq!(tok.lock_info(&ALICE()), (1_000, T0 + 2000));
    }

    #[test]
    fn extend_lock_shrinking_reverts() {
        let mut tok = make_token(0, 0);
        tok.mint(OWNER(), ALICE(), 2_000, T0).unwrap();
        tok.lock_tokens(OWNER(), ALICE(), 1_000, T0 + 2000, T0).unwrap();
        let r = tok.extend_lock(OWNER(), ALICE(), 500, T0 + 2000, T0);
        assert!(matches!(r, Err(Zrc20Error::LockMustGrow)));
    }

    #[test]
    fn replace_lock_after_expiry() {
        let mut tok = make_token(0, 0);
        tok.mint(OWNER(), ALICE(), 1_000, T0).unwrap();
        tok.lock_tokens(OWNER(), ALICE(), 800, T0 + 100, T0).unwrap();
        // After expiry, a new smaller lock is allowed.
        tok.lock_tokens(OWNER(), ALICE(), 200, T0 + 200 + 50, T0 + 200).unwrap();
        assert_eq!(tok.lock_info(&ALICE()), (200, T0 + 250));
    }

    #[test]
    fn lock_active_reverts_on_new_lock() {
        let mut tok = make_token(0, 0);
        tok.mint(OWNER(), ALICE(), 1_000, T0).unwrap();
        tok.lock_tokens(OWNER(), ALICE(), 500, T0 + 3600, T0).unwrap();
        let r = tok.lock_tokens(OWNER(), ALICE(), 200, T0 + 7200, T0);
        assert!(matches!(r, Err(Zrc20Error::ActiveLockExists)));
    }

    // ── Ownership ─────────────────────────────────────────────────────────────

    #[test]
    fn two_step_ownership_transfer() {
        let mut tok = make_token(0, 0);
        tok.transfer_ownership(OWNER(), ALICE()).unwrap();
        assert_eq!(tok.owner(), OWNER()); // Still old owner until accepted.
        assert_eq!(tok.pending_owner(), Some(ALICE()));
        tok.accept_ownership(ALICE()).unwrap();
        assert_eq!(tok.owner(), ALICE());
        assert_eq!(tok.pending_owner(), None);
    }

    #[test]
    fn accept_ownership_wrong_caller_reverts() {
        let mut tok = make_token(0, 0);
        tok.transfer_ownership(OWNER(), ALICE()).unwrap();
        let r = tok.accept_ownership(BOB());
        assert!(matches!(r, Err(Zrc20Error::NotOwner)));
    }

    // ── Logo URI ──────────────────────────────────────────────────────────────

    #[test]
    fn update_logo_uri_persists() {
        let mut tok = make_token(0, 0);
        let old = tok.update_logo_uri(OWNER(), "ipfs://new".into()).unwrap();
        assert_eq!(old, "ipfs://test");
        assert_eq!(tok.logo_uri(), "ipfs://new");
    }

    #[test]
    fn update_logo_uri_not_owner_reverts() {
        let mut tok = make_token(0, 0);
        let r = tok.update_logo_uri(ALICE(), "ipfs://hack".into());
        assert!(matches!(r, Err(Zrc20Error::NotOwner)));
    }

    // ── Transfer pause ────────────────────────────────────────────────────────

    #[test]
    fn pause_blocks_all_transfers() {
        let mut tok = make_token(1_000, 0);
        tok.pause_transfers(OWNER()).unwrap();
        let r = tok.transfer(OWNER(), ALICE(), 100, T0);
        assert!(matches!(r, Err(Zrc20Error::TransferPaused)));
    }

    #[test]
    fn unpause_restores_transfers() {
        let mut tok = make_token(1_000, 0);
        tok.pause_transfers(OWNER()).unwrap();
        tok.unpause_transfers(OWNER()).unwrap();
        tok.transfer(OWNER(), ALICE(), 100, T0).unwrap();
        assert_eq!(tok.balance_of(&ALICE()), 100);
    }

    // ── Anti-bot ──────────────────────────────────────────────────────────────

    #[test]
    fn max_transfer_cap_enforced() {
        let mut tok = make_token(1_000, 0);
        tok.set_max_transfer_amount(OWNER(), 50).unwrap();
        let r = tok.transfer(OWNER(), ALICE(), 100, T0);
        assert!(matches!(r, Err(Zrc20Error::ExceedsMaxTransfer { .. })));
    }

    #[test]
    fn max_transfer_zero_means_disabled() {
        let mut tok = make_token(1_000, 0);
        tok.set_max_transfer_amount(OWNER(), 0).unwrap();
        tok.transfer(OWNER(), ALICE(), 1_000, T0).unwrap();
        assert_eq!(tok.balance_of(&ALICE()), 1_000);
    }

    #[test]
    fn max_transfer_does_not_apply_to_mint() {
        let mut tok = make_token(0, 0);
        tok.set_max_transfer_amount(OWNER(), 10).unwrap();
        tok.mint(OWNER(), ALICE(), 1_000, T0).unwrap();
        assert_eq!(tok.balance_of(&ALICE()), 1_000);
    }
}
