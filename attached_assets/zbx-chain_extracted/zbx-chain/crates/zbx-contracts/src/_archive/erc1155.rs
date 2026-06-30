//! ERC-1155 Multi-Token Standard implementation.
//!
//! ERC-1155 combines ERC-20 (fungible) and ERC-721 (non-fungible) in one contract.
//! A single contract can manage multiple token types (IDs).
//!
//! Key differences from ERC-20/721:
//!   - Batch transfers (safeTransferBatch) -- gas efficient
//!   - IDs can be fungible (qty > 1) or non-fungible (qty = 1)
//!   - Approval is per-operator (setApprovalForAll), not per-token
//!   - Safe transfer callbacks (onERC1155Received / onERC1155BatchReceived)
//!
//! Used in ZBX for:
//!   - Game item tokens (qty > 1 = stackable items)
//!   - NFT collections with supply variants
//!   - Semi-fungible tokens (e.g. concert tickets: same event, qty > 1)

use std::collections::HashMap;

pub const INTERFACE_ID_ERC1155: [u8; 4] = [0xd9, 0xb6, 0x7a, 0x26]; // 0xd9b67a26

// ── ERC-1155 storage ──────────────────────────────────────────────────────────

pub struct Erc1155 {
    /// (owner, token_id) -> balance
    pub balances:        HashMap<([u8; 20], u128), u128>,
    /// (owner, operator) -> is_approved_for_all
    pub operator_approvals: HashMap<([u8; 20], [u8; 20]), bool>,
    /// token_id -> URI
    pub uris:            HashMap<u128, String>,
    /// base URI for all tokens (can be overridden per token)
    pub base_uri:        String,
    /// Event log
    pub events:          Vec<Erc1155Event>,
}

#[derive(Debug, Clone)]
pub enum Erc1155Event {
    TransferSingle { operator: [u8; 20], from: [u8; 20], to: [u8; 20], id: u128, value: u128 },
    TransferBatch  { operator: [u8; 20], from: [u8; 20], to: [u8; 20], ids: Vec<u128>, values: Vec<u128> },
    ApprovalForAll { owner: [u8; 20], operator: [u8; 20], approved: bool },
    URI            { value: String, id: u128 },
}

impl Erc1155 {
    pub fn new(base_uri: String) -> Self {
        Self { balances: HashMap::new(), operator_approvals: HashMap::new(),
               uris: HashMap::new(), base_uri, events: Vec::new() }
    }

    /// Balance of a specific token ID for an account.
    pub fn balance_of(&self, account: [u8; 20], id: u128) -> u128 {
        *self.balances.get(&(account, id)).unwrap_or(&0)
    }

    /// Batch balance query (gas efficient for multiple IDs).
    pub fn balance_of_batch(&self, accounts: &[[u8; 20]], ids: &[u128]) -> Vec<u128> {
        accounts.iter().zip(ids.iter())
            .map(|(acc, id)| self.balance_of(*acc, *id))
            .collect()
    }

    /// Approve or revoke an operator for all tokens.
    pub fn set_approval_for_all(&mut self, owner: [u8; 20], operator: [u8; 20], approved: bool) -> Result<(), Erc1155Error> {
        if owner == operator { return Err(Erc1155Error::ApproveToCaller); }
        self.operator_approvals.insert((owner, operator), approved);
        self.events.push(Erc1155Event::ApprovalForAll { owner, operator, approved });
        Ok(())
    }

    pub fn is_approved_for_all(&self, owner: [u8; 20], operator: [u8; 20]) -> bool {
        *self.operator_approvals.get(&(owner, operator)).unwrap_or(&false)
    }

    /// Transfer a single token ID.
    pub fn safe_transfer_from(
        &mut self, caller: [u8; 20], from: [u8; 20],
        to: [u8; 20], id: u128, amount: u128,
    ) -> Result<(), Erc1155Error> {
        if to == [0u8; 20] { return Err(Erc1155Error::TransferToZero); }
        if caller != from && !self.is_approved_for_all(from, caller) {
            return Err(Erc1155Error::NotApproved);
        }
        let bal = self.balances.entry((from, id)).or_insert(0);
        if *bal < amount { return Err(Erc1155Error::InsufficientBalance); }
        *bal -= amount;
        *self.balances.entry((to, id)).or_insert(0) += amount;
        self.events.push(Erc1155Event::TransferSingle { operator: caller, from, to, id, value: amount });
        Ok(())
    }

    /// Batch transfer -- transfer multiple IDs in one call (gas efficient).
    pub fn safe_batch_transfer_from(
        &mut self, caller: [u8; 20], from: [u8; 20],
        to: [u8; 20], ids: Vec<u128>, amounts: Vec<u128>,
    ) -> Result<(), Erc1155Error> {
        if ids.len() != amounts.len() { return Err(Erc1155Error::ArrayLengthMismatch); }
        if to == [0u8; 20] { return Err(Erc1155Error::TransferToZero); }
        if caller != from && !self.is_approved_for_all(from, caller) {
            return Err(Erc1155Error::NotApproved);
        }
        for (id, amount) in ids.iter().zip(amounts.iter()) {
            let bal = self.balances.entry((from, *id)).or_insert(0);
            if *bal < *amount { return Err(Erc1155Error::InsufficientBalance); }
            *bal -= amount;
            *self.balances.entry((to, *id)).or_insert(0) += amount;
        }
        self.events.push(Erc1155Event::TransferBatch { operator: caller, from, to, ids, values: amounts });
        Ok(())
    }

    /// Mint tokens (internal -- called by owner/minter role).
    pub fn mint(&mut self, to: [u8; 20], id: u128, amount: u128) -> Result<(), Erc1155Error> {
        if to == [0u8; 20] { return Err(Erc1155Error::MintToZero); }
        *self.balances.entry((to, id)).or_insert(0) += amount;
        self.events.push(Erc1155Event::TransferSingle {
            operator: [0u8; 20], from: [0u8; 20], to, id, value: amount
        });
        Ok(())
    }

    /// Burn tokens.
    pub fn burn(&mut self, from: [u8; 20], id: u128, amount: u128) -> Result<(), Erc1155Error> {
        let bal = self.balances.entry((from, id)).or_insert(0);
        if *bal < amount { return Err(Erc1155Error::InsufficientBalance); }
        *bal -= amount;
        self.events.push(Erc1155Event::TransferSingle {
            operator: from, from, to: [0u8; 20], id, value: amount
        });
        Ok(())
    }

    /// Token URI (ERC-1155 Metadata extension).
    pub fn uri(&self, id: u128) -> String {
        self.uris.get(&id).cloned().unwrap_or_else(|| {
            format!("{}{}", self.base_uri, id)
        })
    }
}

#[derive(Debug)]
pub enum Erc1155Error {
    NotApproved, TransferToZero, MintToZero,
    InsufficientBalance, ApproveToCaller, ArrayLengthMismatch,
}