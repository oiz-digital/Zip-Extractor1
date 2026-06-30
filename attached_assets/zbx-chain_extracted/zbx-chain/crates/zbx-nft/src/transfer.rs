//! NFT transfer logic with approval tracking (ZEP-721 compatible).

use std::collections::{HashMap, HashSet};
use zbx_types::address::Address;
use crate::mint::TokenId;

/// Manages ownership transfers and approvals for an NFT collection.
#[derive(Debug, Default)]
pub struct NftTransfer {
    /// token_id → owner
    owners: HashMap<TokenId, Address>,
    /// token_id → approved address
    approvals: HashMap<TokenId, Address>,
    /// owner → set of operators approved for all tokens
    operators: HashMap<Address, HashSet<Address>>,
    /// owner → token_count
    balances: HashMap<Address, u64>,
}

impl NftTransfer {
    pub fn new() -> Self { Self::default() }

    /// Register a newly minted token.
    pub fn register(&mut self, token_id: TokenId, owner: Address) {
        self.owners.insert(token_id, owner);
        *self.balances.entry(owner).or_insert(0) += 1;
    }

    pub fn owner_of(&self, token_id: TokenId) -> Option<Address> {
        self.owners.get(&token_id).copied()
    }

    pub fn approve(&mut self, caller: &Address, to: Address, token_id: TokenId) -> Result<(), &'static str> {
        if self.owners.get(&token_id) != Some(caller) { return Err("not owner"); }
        self.approvals.insert(token_id, to);
        Ok(())
    }

    pub fn set_approval_for_all(&mut self, owner: Address, operator: Address, approved: bool) {
        if approved {
            self.operators.entry(owner).or_default().insert(operator);
        } else if let Some(ops) = self.operators.get_mut(&owner) {
            ops.remove(&operator);
        }
    }

    pub fn transfer_from(&mut self, caller: &Address, from: Address, to: Address, token_id: TokenId) -> Result<(), &'static str> {
        let owner = self.owners.get(&token_id).copied().ok_or("token not found")?;
        if owner != from { return Err("from is not owner"); }
        let approved = self.approvals.get(&token_id).copied() == Some(*caller);
        let is_operator = self.operators.get(&from).map(|ops| ops.contains(caller)).unwrap_or(false);
        if caller != &from && !approved && !is_operator {
            return Err("not authorised");
        }
        self.owners.insert(token_id, to);
        self.approvals.remove(&token_id);
        *self.balances.entry(from).or_insert(1) -= 1;
        *self.balances.entry(to).or_insert(0) += 1;
        Ok(())
    }

    pub fn balance_of(&self, owner: &Address) -> u64 {
        self.balances.get(owner).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(b: u8) -> Address { [b; 20] }

    fn setup() -> (NftTransfer, Address, Address, TokenId) {
        let mut t = NftTransfer::new();
        let owner = addr(1);
        let token_id = 42u128;
        t.register(token_id, owner);
        (t, owner, addr(2), token_id)
    }

    #[test]
    fn owner_can_transfer() {
        let (mut t, owner, recipient, tid) = setup();
        t.transfer_from(&owner, owner, recipient, tid).unwrap();
        assert_eq!(t.owner_of(tid), Some(recipient));
        assert_eq!(t.balance_of(&owner), 0);
        assert_eq!(t.balance_of(&recipient), 1);
    }

    #[test]
    fn non_owner_cannot_transfer_without_approval() {
        let (mut t, owner, _, tid) = setup();
        let attacker = addr(9);
        let err = t.transfer_from(&attacker, owner, attacker, tid).unwrap_err();
        assert_eq!(err, "not authorised");
    }

    #[test]
    fn approved_spender_can_transfer() {
        let (mut t, owner, spender, tid) = setup();
        t.approve(&owner, spender, tid).unwrap();
        t.transfer_from(&spender, owner, spender, tid).unwrap();
        assert_eq!(t.owner_of(tid), Some(spender));
    }

    #[test]
    fn approval_cleared_after_transfer() {
        let (mut t, owner, spender, tid) = setup();
        t.approve(&owner, spender, tid).unwrap();
        t.transfer_from(&spender, owner, spender, tid).unwrap();
        let err = t.transfer_from(&owner, spender, owner, tid).unwrap_err();
        assert_eq!(err, "not authorised");
    }

    #[test]
    fn operator_can_transfer_any_token() {
        let (mut t, owner, operator, tid) = setup();
        t.set_approval_for_all(owner, operator, true);
        t.transfer_from(&operator, owner, operator, tid).unwrap();
        assert_eq!(t.owner_of(tid), Some(operator));
    }

    #[test]
    fn revoked_operator_cannot_transfer() {
        let (mut t, owner, operator, tid) = setup();
        t.set_approval_for_all(owner, operator, true);
        t.set_approval_for_all(owner, operator, false);
        let err = t.transfer_from(&operator, owner, operator, tid).unwrap_err();
        assert_eq!(err, "not authorised");
    }

    #[test]
    fn transfer_nonexistent_token_returns_error() {
        let (mut t, owner, recipient, _) = setup();
        let err = t.transfer_from(&owner, owner, recipient, 999).unwrap_err();
        assert_eq!(err, "token not found");
    }

    #[test]
    fn wrong_from_address_returns_error() {
        let (mut t, owner, recipient, tid) = setup();
        let err = t.transfer_from(&owner, recipient, owner, tid).unwrap_err();
        assert_eq!(err, "from is not owner");
    }
}
