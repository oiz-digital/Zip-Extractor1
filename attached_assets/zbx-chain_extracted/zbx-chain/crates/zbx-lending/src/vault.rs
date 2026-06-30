//! YieldVault — ERC-4626 style yield vault.
//!
//! Users deposit an underlying asset and receive vault shares.
//! Shares appreciate as yield is harvested into the vault's `total_assets`.
//! Conversion: shares ↔ assets via `convert_to_shares` / `convert_to_assets`.
//!
//! Operations: deposit, mint, withdraw, redeem, harvest (strategy yield credit).

use std::collections::HashMap;
use zbx_types::address::Address;
use crate::market::MarketId;

/// Vault errors.
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("zero shares minted — deposit amount too small")]
    ZeroShares,
    #[error("zero assets — cannot withdraw zero")]
    ZeroAssets,
    #[error("insufficient shares: have {have}, need {need}")]
    InsufficientShares { have: u128, need: u128 },
    #[error("insufficient assets in vault: have {have}, need {need}")]
    InsufficientAssets { have: u128, need: u128 },
    #[error("deposit exceeds per-wallet cap of {cap}")]
    DepositCapExceeded { cap: u128 },
    #[error("vault is paused")]
    Paused,
    #[error("caller is not the vault manager")]
    NotManager,
}

/// A single yield vault for one underlying asset.
#[derive(Debug)]
pub struct YieldVault {
    pub asset:         MarketId,
    /// Total underlying assets held (principal + harvested yield).
    pub total_assets:  u128,
    /// Total vault shares issued.
    pub total_shares:  u128,
    /// Per-account share balances.
    balances:          HashMap<Address, u128>,
    /// Optional per-wallet deposit cap (0 = unlimited).
    pub deposit_cap:   u128,
    /// Paused by manager.
    pub paused:        bool,
    /// Manager address (may harvest, pause).
    pub manager:       Address,
    /// Accumulated management fee (10 bps/yr deducted from harvested yield).
    pub fee_reserve:   u128,
    /// Management fee in bps per harvest (default 10 bps = 0.10%).
    pub management_fee_bps: u32,
}

impl YieldVault {
    pub fn new(asset: MarketId, manager: Address) -> Self {
        Self {
            asset,
            total_assets:  0,
            total_shares:  0,
            balances:      HashMap::new(),
            deposit_cap:   0,
            paused:        false,
            manager,
            fee_reserve:   0,
            management_fee_bps: 10,
        }
    }

    // ── Share ↔ Asset conversion ───────────────────────────────────────────

    /// Convert `assets` to shares at current exchange rate.
    /// Returns 0 if total_assets or total_shares are 0 (initial deposit uses 1:1).
    pub fn convert_to_shares(&self, assets: u128) -> u128 {
        if self.total_assets == 0 || self.total_shares == 0 {
            assets // 1:1 for initial deposit
        } else {
            assets * self.total_shares / self.total_assets
        }
    }

    /// Convert `shares` to assets at current exchange rate.
    pub fn convert_to_assets(&self, shares: u128) -> u128 {
        if self.total_shares == 0 {
            shares // 1:1 when empty
        } else {
            shares * self.total_assets / self.total_shares
        }
    }

    // ── ERC-4626 operations ────────────────────────────────────────────────

    /// Deposit `assets` and receive shares. Returns shares minted.
    pub fn deposit(&mut self, depositor: Address, assets: u128) -> Result<u128, VaultError> {
        if self.paused { return Err(VaultError::Paused); }
        if self.deposit_cap > 0 {
            let current = self.balances.get(&depositor).copied().unwrap_or(0);
            let current_assets = self.convert_to_assets(current);
            if current_assets + assets > self.deposit_cap {
                return Err(VaultError::DepositCapExceeded { cap: self.deposit_cap });
            }
        }
        let shares = self.convert_to_shares(assets);
        if shares == 0 { return Err(VaultError::ZeroShares); }
        self.total_assets += assets;
        self.total_shares += shares;
        *self.balances.entry(depositor).or_default() += shares;
        Ok(shares)
    }

    /// Withdraw `assets` worth of underlying, burning the equivalent shares.
    /// Returns shares burned.
    pub fn withdraw(&mut self, owner: Address, assets: u128) -> Result<u128, VaultError> {
        if self.paused { return Err(VaultError::Paused); }
        if assets == 0 { return Err(VaultError::ZeroAssets); }
        if self.total_assets < assets {
            return Err(VaultError::InsufficientAssets { have: self.total_assets, need: assets });
        }
        let shares = self.convert_to_shares(assets).max(1);
        let have = self.balances.get(&owner).copied().unwrap_or(0);
        if have < shares {
            return Err(VaultError::InsufficientShares { have, need: shares });
        }
        *self.balances.get_mut(&owner).unwrap() -= shares;
        self.total_shares = self.total_shares.saturating_sub(shares);
        self.total_assets = self.total_assets.saturating_sub(assets);
        Ok(shares)
    }

    /// Redeem `shares` for underlying assets. Returns assets returned.
    pub fn redeem(&mut self, owner: Address, shares: u128) -> Result<u128, VaultError> {
        if self.paused { return Err(VaultError::Paused); }
        if shares == 0 { return Err(VaultError::ZeroShares); }
        let have = self.balances.get(&owner).copied().unwrap_or(0);
        if have < shares {
            return Err(VaultError::InsufficientShares { have, need: shares });
        }
        let assets = self.convert_to_assets(shares);
        if self.total_assets < assets {
            return Err(VaultError::InsufficientAssets { have: self.total_assets, need: assets });
        }
        *self.balances.get_mut(&owner).unwrap() -= shares;
        self.total_shares = self.total_shares.saturating_sub(shares);
        self.total_assets = self.total_assets.saturating_sub(assets);
        Ok(assets)
    }

    /// Harvest yield — credits `yield_amount` to the vault's `total_assets`.
    /// Deducts management fee (management_fee_bps) before crediting.
    /// Only the manager may call this.
    pub fn harvest(&mut self, caller: Address, yield_amount: u128) -> Result<u128, VaultError> {
        if caller != self.manager { return Err(VaultError::NotManager); }
        let fee = yield_amount * (self.management_fee_bps as u128) / 10_000;
        let net_yield = yield_amount.saturating_sub(fee);
        self.fee_reserve += fee;
        self.total_assets += net_yield;
        Ok(net_yield)
    }

    /// Pause or unpause the vault (manager-only).
    pub fn set_paused(&mut self, caller: Address, paused: bool) -> Result<(), VaultError> {
        if caller != self.manager { return Err(VaultError::NotManager); }
        self.paused = paused;
        Ok(())
    }

    /// Current share balance of `addr`.
    pub fn balance_of(&self, addr: &Address) -> u128 {
        self.balances.get(addr).copied().unwrap_or(0)
    }

    /// Current exchange rate: assets per 1e18 shares.
    pub fn price_per_share(&self) -> u128 {
        if self.total_shares == 0 { return 1_000_000_000_000_000_000; }
        self.total_assets * 1_000_000_000_000_000_000 / self.total_shares
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(v: u8) -> Address { Address([v; 20]) }
    fn mid(s: &str) -> MarketId { MarketId(s.to_string()) }

    #[test]
    fn deposit_and_redeem_roundtrip() {
        let mut v = YieldVault::new(mid("ZBX"), addr(99));
        let shares = v.deposit(addr(1), 1_000_000).unwrap();
        assert_eq!(shares, 1_000_000); // 1:1 on first deposit
        let assets = v.redeem(addr(1), shares).unwrap();
        assert_eq!(assets, 1_000_000);
        assert_eq!(v.total_shares, 0);
        assert_eq!(v.total_assets, 0);
    }

    #[test]
    fn shares_appreciate_after_harvest() {
        let mut v = YieldVault::new(mid("ZBX"), addr(99));
        v.deposit(addr(1), 1_000_000).unwrap();
        v.harvest(addr(99), 100_000).unwrap(); // 100 000 yield (100 bps fee = 10)
        // share price should be > 1:1
        assert!(v.price_per_share() > 1_000_000_000_000_000_000);
        // Two depositors should get proportional share
        v.deposit(addr(2), 1_000_000).unwrap();
        // addr(1) has proportionally more assets
        let a1 = v.convert_to_assets(v.balance_of(&addr(1)));
        let a2 = v.convert_to_assets(v.balance_of(&addr(2)));
        assert!(a1 > a2);
    }

    #[test]
    fn only_manager_can_harvest() {
        let mut v = YieldVault::new(mid("ZBX"), addr(99));
        v.deposit(addr(1), 1_000_000).unwrap();
        assert!(matches!(v.harvest(addr(1), 100), Err(VaultError::NotManager)));
    }

    #[test]
    fn paused_vault_blocks_deposit_withdraw() {
        let mut v = YieldVault::new(mid("ZBX"), addr(99));
        v.deposit(addr(1), 500_000).unwrap();
        v.set_paused(addr(99), true).unwrap();
        assert!(matches!(v.deposit(addr(1), 100), Err(VaultError::Paused)));
        assert!(matches!(v.withdraw(addr(1), 100), Err(VaultError::Paused)));
    }

    #[test]
    fn insufficient_shares_rejected() {
        let mut v = YieldVault::new(mid("ZBX"), addr(99));
        v.deposit(addr(1), 1_000).unwrap();
        assert!(matches!(v.redeem(addr(1), 10_000), Err(VaultError::InsufficientShares { .. })));
    }

    #[test]
    fn management_fee_collected() {
        let mut v = YieldVault::new(mid("ZBX"), addr(99));
        v.management_fee_bps = 100; // 1%
        v.deposit(addr(1), 1_000_000).unwrap();
        let net = v.harvest(addr(99), 10_000).unwrap();
        assert_eq!(net, 9_900);       // 1% fee = 100
        assert_eq!(v.fee_reserve, 100);
    }
}
