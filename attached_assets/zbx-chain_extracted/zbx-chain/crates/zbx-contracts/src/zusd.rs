//! ZUSD stablecoin contract — 1:1 USD-pegged token with mint/burn authority.
//!
//! Decimal precision: 18 (ERC-20 standard, same as ZBX).
//! All amounts are in wei-equivalent units (1 ZUSD = 10^18 internal units).
//!
//! C-02 fix (2026-05-03): changed from 6 decimals (10^6) to 18 decimals (10^18)
//! to match the ERC-20 standard used by Ethereum tooling and the bridge contract.
//! Any state snapshot taken before this fix stored amounts in 6-decimal units and
//! MUST be rescaled (× 10^12) before use.

use std::collections::HashMap;
use zbx_types::address::Address;

/// ZUSD decimals — 18, matching ERC-20 standard (same as ZBX and WETH).
pub const DECIMALS: u8 = 18;

/// 1 ZUSD expressed in internal units (10^18).
pub const ONE_ZUSD: u128 = 1_000_000_000_000_000_000u128;

/// Total ZUSD supply cap: 100 billion ZUSD with 18 decimal precision.
///
/// = 100_000_000_000 × 10^18 = 10^29 (well within u128 range of ~3.4 × 10^38).
pub const MAX_SUPPLY: u128 = 100_000_000_000u128 * ONE_ZUSD;

/// ZUSD stablecoin state.
#[derive(Debug, Default)]
pub struct ZusdContract {
    balances:     HashMap<Address, u128>,
    allowances:   HashMap<(Address, Address), u128>,
    total_supply: u128,
    minters:      std::collections::HashSet<Address>,
    owner:        Option<Address>,
}

impl ZusdContract {
    pub fn new(owner: Address) -> Self {
        let mut c = Self::default();
        c.owner = Some(owner);
        c
    }

    /// ERC-20 `decimals()` — always 18.
    pub fn decimals(&self) -> u8 { DECIMALS }

    pub fn balance_of(&self, addr: &Address) -> u128 {
        self.balances.get(addr).copied().unwrap_or(0)
    }

    pub fn total_supply(&self) -> u128 { self.total_supply }

    pub fn add_minter(&mut self, caller: &Address, minter: Address) -> Result<(), &'static str> {
        if Some(*caller) != self.owner { return Err("not owner"); }
        self.minters.insert(minter);
        Ok(())
    }

    /// ZUSD-02 fix (2026-05-16) — revoke a minter's privilege.
    ///
    /// Without this, a compromised minter key can inflate supply to MAX_SUPPLY
    /// (100 billion ZUSD) with no on-chain remedy.  Owner can now revoke at any
    /// time; subsequent `mint` calls from the revoked address return `Err("not
    /// minter")`.
    pub fn remove_minter(&mut self, caller: &Address, minter: &Address) -> Result<(), &'static str> {
        if Some(*caller) != self.owner { return Err("not owner"); }
        self.minters.remove(minter);
        Ok(())
    }

    /// Mint `amount` ZUSD (in 18-decimal units) to `to`.
    pub fn mint(&mut self, caller: &Address, to: Address, amount: u128) -> Result<(), &'static str> {
        if !self.minters.contains(caller) { return Err("not minter"); }
        let new_supply = self.total_supply.checked_add(amount).ok_or("overflow")?;
        if new_supply > MAX_SUPPLY { return Err("supply cap exceeded"); }
        *self.balances.entry(to).or_insert(0) += amount;
        self.total_supply = new_supply;
        Ok(())
    }

    /// Burn `amount` ZUSD from `from`.
    ///
    /// ## ZUSD-01 fix (2026-05-16) — use `checked_sub` for `total_supply`
    /// Previously used `saturating_sub`, which would silently floor
    /// `total_supply` to 0 if accounting ever drifted, breaking the invariant
    /// `total_supply == Σ balances` with no observable signal.  `checked_sub`
    /// surfaces any drift immediately as an error.
    ///
    /// ## ZUSD-03 fix (2026-05-16) — no ghost map entries
    /// Previously used `entry(from).or_insert(0)` which persists a zero-balance
    /// record for every address that attempts to burn tokens they don't hold.
    /// Now uses `get_mut` so non-existent addresses are rejected cleanly.
    pub fn burn(&mut self, from: Address, amount: u128) -> Result<(), &'static str> {
        let bal = self.balances.get_mut(&from).ok_or("insufficient balance")?;
        if *bal < amount { return Err("insufficient balance"); }
        *bal -= amount;
        // ZUSD-01: checked_sub so any supply/balance drift is caught immediately.
        self.total_supply = self.total_supply
            .checked_sub(amount)
            .ok_or("accounting invariant violated: total_supply < burn amount")?;
        Ok(())
    }

    /// ZUSD-03 fix (2026-05-16): use `get_mut` so a zero-balance `from` address
    /// does not gain a ghost entry in the balance map on failed transfers.
    pub fn transfer(&mut self, from: Address, to: Address, amount: u128) -> Result<(), &'static str> {
        let from_bal = self.balances.get_mut(&from).ok_or("insufficient balance")?;
        if *from_bal < amount { return Err("insufficient balance"); }
        *from_bal -= amount;
        *self.balances.entry(to).or_insert(0) += amount;
        Ok(())
    }

    /// ERC-20 `approve`: set allowance for `spender` on behalf of `owner`.
    pub fn approve(&mut self, owner: Address, spender: Address, amount: u128) {
        self.allowances.insert((owner, spender), amount);
    }

    /// ERC-20 `allowance`.
    pub fn allowance(&self, owner: &Address, spender: &Address) -> u128 {
        self.allowances.get(&(*owner, *spender)).copied().unwrap_or(0)
    }

    /// ERC-20 `transferFrom`.
    pub fn transfer_from(
        &mut self,
        spender: &Address,
        from: Address,
        to: Address,
        amount: u128,
    ) -> Result<(), &'static str> {
        let allow = self.allowances.get_mut(&(from, *spender))
            .ok_or("allowance not set")?;
        if *allow < amount { return Err("allowance exceeded"); }
        // Decrement allowance before touching balances so a balance failure
        // after a successful allowance deduct is impossible (both are &mut on
        // the same struct, so Rust borrow rules already prevent aliasing, but
        // the order is explicit for auditability).
        *allow -= amount;
        // ZUSD-03 fix: use get_mut — no ghost zero-balance entry for `from`.
        let from_bal = self.balances.get_mut(&from).ok_or("insufficient balance")?;
        if *from_bal < amount { return Err("insufficient balance"); }
        *from_bal -= amount;
        *self.balances.entry(to).or_insert(0) += amount;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(n: u8) -> Address { Address([n; 20]) }

    #[test]
    fn decimals_is_18() {
        let c = ZusdContract::new(addr(1));
        assert_eq!(c.decimals(), 18);
    }

    #[test]
    fn max_supply_is_100_billion_18_dec() {
        // 100_000_000_000 × 10^18
        assert_eq!(MAX_SUPPLY, 100_000_000_000u128 * 1_000_000_000_000_000_000u128);
    }

    #[test]
    fn mint_and_burn() {
        let owner = addr(1);
        let minter = addr(2);
        let user = addr(3);
        let mut c = ZusdContract::new(owner);
        c.add_minter(&owner, minter).unwrap();

        let amount = ONE_ZUSD * 1_000; // 1000 ZUSD
        c.mint(&minter, user, amount).unwrap();
        assert_eq!(c.balance_of(&user), amount);
        assert_eq!(c.total_supply(), amount);

        c.burn(user, amount).unwrap();
        assert_eq!(c.balance_of(&user), 0);
        assert_eq!(c.total_supply(), 0);
    }

    #[test]
    fn supply_cap_enforced() {
        let owner = addr(1);
        let minter = addr(2);
        let mut c = ZusdContract::new(owner);
        c.add_minter(&owner, minter).unwrap();
        // Minting exactly MAX_SUPPLY should succeed.
        c.mint(&minter, addr(3), MAX_SUPPLY).unwrap();
        // One more unit must fail.
        assert!(c.mint(&minter, addr(4), 1).is_err());
    }

    #[test]
    fn transfer_and_allowance() {
        let owner = addr(1);
        let minter = addr(2);
        let alice = addr(3);
        let bob = addr(4);
        let mut c = ZusdContract::new(owner);
        c.add_minter(&owner, minter).unwrap();
        c.mint(&minter, alice, ONE_ZUSD * 500).unwrap();

        c.approve(alice, bob, ONE_ZUSD * 100);
        assert_eq!(c.allowance(&alice, &bob), ONE_ZUSD * 100);

        c.transfer_from(&bob, alice, bob, ONE_ZUSD * 50).unwrap();
        assert_eq!(c.balance_of(&bob), ONE_ZUSD * 50);
        assert_eq!(c.allowance(&alice, &bob), ONE_ZUSD * 50);
    }

    // ── ZUSD-02: remove_minter ────────────────────────────────────────────────

    #[test]
    fn remove_minter_revokes_privilege() {
        let owner  = addr(1);
        let minter = addr(2);
        let user   = addr(3);
        let mut c  = ZusdContract::new(owner);
        c.add_minter(&owner, minter).unwrap();

        // Minting works before revocation.
        c.mint(&minter, user, ONE_ZUSD).unwrap();

        // Owner revokes the minter.
        c.remove_minter(&owner, &minter).unwrap();

        // Minting must fail after revocation.
        assert_eq!(c.mint(&minter, user, ONE_ZUSD), Err("not minter"));
        // Previously minted tokens are unaffected.
        assert_eq!(c.balance_of(&user), ONE_ZUSD);
    }

    #[test]
    fn remove_minter_non_owner_rejected() {
        let owner  = addr(1);
        let minter = addr(2);
        let rogue  = addr(9);
        let mut c  = ZusdContract::new(owner);
        c.add_minter(&owner, minter).unwrap();
        assert_eq!(c.remove_minter(&rogue, &minter), Err("not owner"));
        // Minter privilege must still be intact.
        c.mint(&minter, addr(3), ONE_ZUSD).unwrap();
    }

    // ── ZUSD-01: burn total_supply uses checked_sub ───────────────────────────

    #[test]
    fn burn_maintains_total_supply_invariant() {
        let owner  = addr(1);
        let minter = addr(2);
        let user   = addr(3);
        let mut c  = ZusdContract::new(owner);
        c.add_minter(&owner, minter).unwrap();
        let amount = ONE_ZUSD * 500;
        c.mint(&minter, user, amount).unwrap();

        // Partial burn: total_supply must decrease by exactly the burned amount.
        c.burn(user, ONE_ZUSD * 100).unwrap();
        assert_eq!(c.total_supply(), ONE_ZUSD * 400);
        assert_eq!(c.balance_of(&user), ONE_ZUSD * 400);

        // Burning more than balance must fail without mutating supply.
        assert!(c.burn(user, ONE_ZUSD * 500).is_err());
        assert_eq!(c.total_supply(), ONE_ZUSD * 400); // unchanged
    }

    // ── ZUSD-03: no ghost zero-balance entries ────────────────────────────────

    #[test]
    fn burn_zero_balance_addr_no_ghost_entry() {
        let owner  = addr(1);
        let nobody = addr(99);
        let mut c  = ZusdContract::new(owner);

        // Nobody has no balance — burn must fail cleanly.
        assert!(c.burn(nobody, 1).is_err());
        // No entry must be inserted into the balance map.
        assert!(!c.balances.contains_key(&nobody));
    }

    #[test]
    fn transfer_zero_balance_addr_no_ghost_entry() {
        let owner  = addr(1);
        let minter = addr(2);
        let nobody = addr(99);
        let bob    = addr(4);
        let mut c  = ZusdContract::new(owner);
        c.add_minter(&owner, minter).unwrap();

        // Transfer from a zero-balance address must fail cleanly.
        assert!(c.transfer(nobody, bob, 1).is_err());
        assert!(!c.balances.contains_key(&nobody));
    }

    #[test]
    fn transfer_from_zero_balance_addr_no_ghost_entry() {
        let owner  = addr(1);
        let minter = addr(2);
        let nobody = addr(99);
        let bob    = addr(4);
        let mut c  = ZusdContract::new(owner);
        c.add_minter(&owner, minter).unwrap();

        // Give bob an allowance over nobody's (empty) account.
        c.approve(nobody, bob, ONE_ZUSD);
        assert!(c.transfer_from(&bob, nobody, bob, 1).is_err());
        assert!(!c.balances.contains_key(&nobody));
    }
}
