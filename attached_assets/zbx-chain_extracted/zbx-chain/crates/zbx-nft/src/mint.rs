//! NFT minting logic (ZEP-721 / ZEP-1155).

use std::collections::HashMap;
use zbx_types::address::Address;

pub type TokenId = u128;

/// Tracks ownership and supply for a single NFT collection.
#[derive(Debug, Default)]
pub struct NftMinter {
    /// token_id → owner
    owners: HashMap<TokenId, Address>,
    /// owner → count
    balances: HashMap<Address, u64>,
    next_id: TokenId,
    /// Maximum supply (0 = unlimited).
    pub max_supply: u128,
    pub minter: Option<Address>,
}

impl NftMinter {
    pub fn new(minter: Address, max_supply: u128) -> Self {
        Self { minter: Some(minter), max_supply, ..Default::default() }
    }

    pub fn mint(&mut self, caller: &Address, to: Address) -> Result<TokenId, &'static str> {
        if Some(*caller) != self.minter { return Err("not minter"); }
        if self.max_supply > 0 && self.next_id >= self.max_supply {
            return Err("max supply reached");
        }
        let token_id = self.next_id;
        self.next_id += 1;
        self.owners.insert(token_id, to);
        *self.balances.entry(to).or_insert(0) += 1;
        Ok(token_id)
    }

    pub fn owner_of(&self, token_id: TokenId) -> Option<Address> {
        self.owners.get(&token_id).copied()
    }

    pub fn balance_of(&self, owner: &Address) -> u64 {
        self.balances.get(owner).copied().unwrap_or(0)
    }

    pub fn total_supply(&self) -> u128 { self.next_id }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(b: u8) -> Address { [b; 20] }

    #[test]
    fn mint_increments_token_id_and_balance() {
        let minter = addr(1);
        let mut m = NftMinter::new(minter, 0);
        let id0 = m.mint(&minter, addr(2)).unwrap();
        let id1 = m.mint(&minter, addr(2)).unwrap();
        assert_eq!(id0, 0);
        assert_eq!(id1, 1);
        assert_eq!(m.balance_of(&addr(2)), 2);
        assert_eq!(m.total_supply(), 2);
    }

    #[test]
    fn owner_of_returns_correct_owner() {
        let minter = addr(1);
        let mut m = NftMinter::new(minter, 0);
        let id = m.mint(&minter, addr(3)).unwrap();
        assert_eq!(m.owner_of(id), Some(addr(3)));
        assert_eq!(m.owner_of(99), None);
    }

    #[test]
    fn non_minter_cannot_mint() {
        let minter = addr(1);
        let mut m = NftMinter::new(minter, 0);
        let err = m.mint(&addr(9), addr(2)).unwrap_err();
        assert_eq!(err, "not minter");
    }

    #[test]
    fn max_supply_enforced() {
        let minter = addr(1);
        let mut m = NftMinter::new(minter, 2);
        m.mint(&minter, addr(2)).unwrap();
        m.mint(&minter, addr(2)).unwrap();
        let err = m.mint(&minter, addr(2)).unwrap_err();
        assert_eq!(err, "max supply reached");
    }

    #[test]
    fn balance_of_unknown_address_is_zero() {
        let minter = addr(1);
        let m = NftMinter::new(minter, 0);
        assert_eq!(m.balance_of(&addr(99)), 0);
    }

    #[test]
    fn unlimited_supply_allows_many_mints() {
        let minter = addr(1);
        let mut m = NftMinter::new(minter, 0);
        for i in 0u8..50 {
            m.mint(&minter, addr(i % 10)).unwrap();
        }
        assert_eq!(m.total_supply(), 50);
    }
}
