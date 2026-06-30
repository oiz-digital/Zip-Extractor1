//! ERC-4626 Tokenized Vault Standard.
//!
//! ERC-4626 standardizes yield-bearing vaults:
//!   - Users deposit an "asset" token (e.g. USDC, ZBX)
//!   - They receive "shares" (vault tokens) representing their stake
//!   - Shares appreciate as the vault earns yield
//!   - Shares can be redeemed for the underlying asset + yield
//!
//! Conversion formula:
//!   shares = assets * totalShares / totalAssets  (deposit)
//!   assets = shares * totalAssets / totalShares  (redeem)
//!
//! ZBX uses ERC-4626 for:
//!   - ZBX Staking Vault (deposit ZBX -> sZBX shares)
//!   - Liquidity pools (LP tokens)
//!   - Lending markets (deposit USDC -> zbxUSDC yield tokens)

pub const INTERFACE_ID_ERC4626: [u8; 4] = [0x39, 0x08, 0x84, 0x50]; // 0x39088450

pub struct Erc4626 {
    /// The underlying asset token address (e.g. ZBX address)
    pub asset:         [u8; 20],
    /// Total assets held by the vault (increases with yield)
    pub total_assets:  u128,
    /// Total shares outstanding
    pub total_shares:  u128,
    /// Balances: holder -> shares
    pub shares:        std::collections::HashMap<[u8; 20], u128>,
    /// Allowances for share transfers
    pub allowances:    std::collections::HashMap<([u8; 20], [u8; 20]), u128>,
    /// Vault name and symbol (ERC-20 compatible)
    pub name:          String,
    pub symbol:        String,
}

impl Erc4626 {
    pub fn new(asset: [u8; 20], name: String, symbol: String) -> Self {
        Self { asset, total_assets: 0, total_shares: 0,
               shares: Default::default(), allowances: Default::default(), name, symbol }
    }

    /// Preview how many shares depositing assets would yield.
    pub fn preview_deposit(&self, assets: u128) -> u128 {
        self.convert_to_shares(assets)
    }

    /// Preview how many assets withdrawing shares would yield.
    pub fn preview_redeem(&self, shares: u128) -> u128 {
        self.convert_to_assets(shares)
    }

    /// Convert assets to shares (current exchange rate).
    pub fn convert_to_shares(&self, assets: u128) -> u128 {
        if self.total_assets == 0 || self.total_shares == 0 {
            assets // 1:1 initial rate
        } else {
            assets * self.total_shares / self.total_assets
        }
    }

    /// Convert shares to assets (current exchange rate).
    pub fn convert_to_assets(&self, shares: u128) -> u128 {
        if self.total_shares == 0 {
            shares
        } else {
            shares * self.total_assets / self.total_shares
        }
    }

    /// Deposit assets, receive shares.
    pub fn deposit(&mut self, caller: [u8; 20], assets: u128, receiver: [u8; 20]) -> Result<u128, VaultError> {
        if assets == 0 { return Err(VaultError::ZeroAssets); }
        let shares = self.convert_to_shares(assets);
        if shares == 0 { return Err(VaultError::ZeroShares); }
        // Transfer assets from caller to vault (handled by ERC-20 transfer)
        self.total_assets += assets;
        self.total_shares += shares;
        *self.shares.entry(receiver).or_insert(0) += shares;
        Ok(shares)
    }

    /// Redeem shares, receive assets.
    pub fn redeem(&mut self, caller: [u8; 20], shares: u128, receiver: [u8; 20], owner: [u8; 20]) -> Result<u128, VaultError> {
        if shares == 0 { return Err(VaultError::ZeroShares); }
        if caller != owner {
            let allowance = self.allowances.entry((owner, caller)).or_insert(0);
            if *allowance < shares { return Err(VaultError::InsufficientAllowance); }
            *allowance -= shares;
        }
        let owner_shares = self.shares.get(&owner).copied().unwrap_or(0);
        if owner_shares < shares { return Err(VaultError::InsufficientShares); }
        let assets = self.convert_to_assets(shares);
        if assets == 0 { return Err(VaultError::ZeroAssets); }
        *self.shares.entry(owner).or_insert(0) -= shares;
        self.total_shares -= shares;
        self.total_assets -= assets;
        // Transfer assets to receiver (handled by ERC-20 transfer)
        Ok(assets)
    }

    /// Mint exact shares (ERC-4626 "mint" function).
    pub fn mint_shares(&mut self, caller: [u8; 20], shares: u128, receiver: [u8; 20]) -> Result<u128, VaultError> {
        let assets = self.convert_to_assets(shares);
        self.deposit(caller, assets, receiver)
    }

    /// Withdraw exact assets (ERC-4626 "withdraw" function).
    pub fn withdraw(&mut self, caller: [u8; 20], assets: u128, receiver: [u8; 20], owner: [u8; 20]) -> Result<u128, VaultError> {
        let shares = self.convert_to_shares(assets);
        self.redeem(caller, shares, receiver, owner)
    }

    /// Max deposit for a given receiver (unlimited by default).
    pub fn max_deposit(&self, _receiver: [u8; 20]) -> u128 { u128::MAX }

    /// Max redeem for a given owner.
    pub fn max_redeem(&self, owner: [u8; 20]) -> u128 {
        self.shares.get(&owner).copied().unwrap_or(0)
    }
}

#[derive(Debug)]
pub enum VaultError {
    ZeroAssets, ZeroShares, InsufficientShares, InsufficientAllowance, VaultPaused,
}