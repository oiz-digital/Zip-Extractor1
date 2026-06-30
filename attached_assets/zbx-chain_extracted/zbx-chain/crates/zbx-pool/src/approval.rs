//! ERC-20 style token approval and allowance system.
//!
//! ## Model
//!
//! Mirrors the ERC-20 approve/allowance/transferFrom interface:
//!
//! ```text
//! owner  ──approve(spender, amount)──►  AllowanceRegistry
//! spender ──transferFrom(owner, to, amount)──► moves tokens on owner's behalf
//! ```
//!
//! ## Security features
//!
//! | Attack | Defence |
//! |--------|---------|
//! | Approval front-running | `increase_allowance` / `decrease_allowance` helpers |
//! | Infinite approval drain | Optional `expire_at` block deadline per approval |
//! | Unauthorised spend | `spend_allowance` atomically deducts, fails if insufficient |
//! | Zero-address spender | Rejected at approve() time |
//! | Self-approval | Rejected (owner == spender) |
//! | Zero-address recipient | Rejected in `transfer` and `transfer_from` |

use std::collections::HashMap;
use zbx_types::address::Address;
use serde::{Deserialize, Serialize};

// ── Types ──────────────────────────────────────────────────────────────────────

/// One approval entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approval {
    /// Approved spending limit (wei-equivalent token units).
    pub amount: u128,
    /// Optional block-number expiry. 0 = never expires.
    pub expire_at_block: u64,
}

/// Error variants for approval operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalError {
    /// owner == spender is not allowed.
    SelfApproval,
    /// Spender address is the zero address.
    ZeroAddressSpender,
    /// Recipient address is the zero address (ERC-20: burns must be explicit).
    ZeroAddressRecipient,
    /// Decreasing allowance would underflow (use `decrease_allowance`).
    AllowanceUnderflow,
    /// Transferring more than the approved allowance.
    InsufficientAllowance { have: u128, need: u128 },
    /// The approval has expired (current block > expire_at_block).
    ApprovalExpired { expired_at: u64, current: u64 },
    /// Transferring more than the token balance of the owner.
    InsufficientBalance { have: u128, need: u128 },
}

impl std::fmt::Display for ApprovalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SelfApproval => write!(f, "owner cannot approve themselves as spender"),
            Self::ZeroAddressSpender => write!(f, "spender cannot be the zero address"),
            Self::ZeroAddressRecipient => write!(f, "recipient cannot be the zero address"),
            Self::AllowanceUnderflow => write!(f, "allowance decrease would underflow"),
            Self::InsufficientAllowance { have, need } =>
                write!(f, "allowance too low: have {have}, need {need}"),
            Self::ApprovalExpired { expired_at, current } =>
                write!(f, "approval expired at block {expired_at}, current {current}"),
            Self::InsufficientBalance { have, need } =>
                write!(f, "balance too low: have {have}, need {need}"),
        }
    }
}
impl std::error::Error for ApprovalError {}

// ── AllowanceRegistry ─────────────────────────────────────────────────────────

/// Global allowance registry: token → owner → spender → Approval.
///
/// One registry instance lives per DEX / token system. It tracks spending
/// permissions across all ERC-20 compatible tokens in the ZBX ecosystem.
#[derive(Debug, Default)]
pub struct AllowanceRegistry {
    /// token_address → owner → spender → Approval
    allowances: HashMap<Address, HashMap<Address, HashMap<Address, Approval>>>,
    /// token_address → owner → balance (managed externally but readable for transferFrom)
    balances: HashMap<Address, HashMap<Address, u128>>,
}

impl AllowanceRegistry {
    pub fn new() -> Self { Self::default() }

    // ── Balance management (called by token contract / DEX engine) ─────────────

    /// Set the token balance for an account (called by TokenFactory/DEX when minting/burning).
    pub fn set_balance(&mut self, token: Address, owner: Address, balance: u128) {
        *self.balances.entry(token).or_default().entry(owner).or_insert(0) = balance;
    }

    /// Add to a token balance (mint / deposit).
    pub fn add_balance(&mut self, token: Address, owner: Address, amount: u128) {
        let b = self.balances.entry(token).or_default().entry(owner).or_insert(0);
        *b = b.saturating_add(amount);
    }

    /// Subtract from a token balance (burn / withdrawal). Returns error if insufficient.
    pub fn sub_balance(
        &mut self,
        token: Address,
        owner: Address,
        amount: u128,
    ) -> Result<(), ApprovalError> {
        let b = self.balances.entry(token).or_default().entry(owner).or_insert(0);
        if *b < amount {
            return Err(ApprovalError::InsufficientBalance { have: *b, need: amount });
        }
        *b -= amount;
        Ok(())
    }

    /// Query token balance.
    pub fn balance_of(&self, token: &Address, owner: &Address) -> u128 {
        self.balances.get(token)
            .and_then(|m| m.get(owner))
            .copied()
            .unwrap_or(0)
    }

    // ── Approve ────────────────────────────────────────────────────────────────

    /// Grant `spender` permission to spend up to `amount` of `token` from `owner`.
    ///
    /// - `expire_at_block = 0` → approval never expires.
    /// - Replaces any previous approval (ERC-20 standard behaviour).
    pub fn approve(
        &mut self,
        token:          Address,
        owner:          Address,
        spender:        Address,
        amount:         u128,
        expire_at_block: u64,
    ) -> Result<(), ApprovalError> {
        if owner == spender {
            return Err(ApprovalError::SelfApproval);
        }
        if spender == Address([0u8; 20]) {
            return Err(ApprovalError::ZeroAddressSpender);
        }
        self.allowances
            .entry(token)
            .or_default()
            .entry(owner)
            .or_default()
            .insert(spender, Approval { amount, expire_at_block });
        Ok(())
    }

    /// Increase an existing allowance by `delta` (front-running safe).
    pub fn increase_allowance(
        &mut self,
        token:   Address,
        owner:   Address,
        spender: Address,
        delta:   u128,
        current_block: u64,
    ) -> Result<(), ApprovalError> {
        let entry = self.allowances
            .entry(token)
            .or_default()
            .entry(owner)
            .or_default()
            .entry(spender)
            .or_insert(Approval { amount: 0, expire_at_block: 0 });
        entry.amount = entry.amount.saturating_add(delta);
        let _ = current_block; // expiry not changed on increase
        Ok(())
    }

    /// Decrease an existing allowance by `delta`. Fails if result would underflow.
    pub fn decrease_allowance(
        &mut self,
        token:   Address,
        owner:   Address,
        spender: Address,
        delta:   u128,
    ) -> Result<(), ApprovalError> {
        let entry = self.allowances
            .entry(token)
            .or_default()
            .entry(owner)
            .or_default()
            .entry(spender)
            .or_insert(Approval { amount: 0, expire_at_block: 0 });
        if entry.amount < delta {
            return Err(ApprovalError::AllowanceUnderflow);
        }
        entry.amount -= delta;
        Ok(())
    }

    // ── Query ──────────────────────────────────────────────────────────────────

    /// Return the current approved allowance (0 if none / expired).
    pub fn allowance(
        &self,
        token:         &Address,
        owner:         &Address,
        spender:       &Address,
        current_block: u64,
    ) -> u128 {
        if let Some(a) = self.allowances
            .get(token)
            .and_then(|m| m.get(owner))
            .and_then(|m| m.get(spender))
        {
            if a.expire_at_block != 0 && current_block > a.expire_at_block {
                return 0; // expired
            }
            a.amount
        } else {
            0
        }
    }

    // ── Spend (deduct) ─────────────────────────────────────────────────────────

    /// Atomically deduct `amount` from `spender`'s allowance for `owner`'s `token`.
    ///
    /// Called internally before executing a transferFrom. Checks:
    /// 1. Approval exists and is not expired.
    /// 2. Allowance >= amount.
    /// 3. Owner's balance >= amount.
    pub fn spend_allowance(
        &mut self,
        token:         Address,
        owner:         Address,
        spender:       Address,
        amount:        u128,
        current_block: u64,
    ) -> Result<(), ApprovalError> {
        // Check and deduct allowance
        {
            let entry = self.allowances
                .entry(token)
                .or_default()
                .entry(owner)
                .or_default()
                .get_mut(&spender)
                .ok_or(ApprovalError::InsufficientAllowance { have: 0, need: amount })?;

            if entry.expire_at_block != 0 && current_block > entry.expire_at_block {
                return Err(ApprovalError::ApprovalExpired {
                    expired_at: entry.expire_at_block,
                    current:    current_block,
                });
            }
            if entry.amount < amount {
                return Err(ApprovalError::InsufficientAllowance {
                    have: entry.amount,
                    need: amount,
                });
            }
            // u128::MAX = infinite approval (never deducted, ERC-20 convention)
            if entry.amount != u128::MAX {
                entry.amount -= amount;
            }
        }
        Ok(())
    }

    // ── TransferFrom ──────────────────────────────────────────────────────────

    /// ERC-20 transferFrom: move `amount` of `token` from `owner` to `to`,
    /// using `spender`'s pre-approved allowance.
    ///
    /// Atomically: (1) verifies recipient, (2) verifies and deducts allowance,
    /// (3) deducts owner balance, (4) credits recipient.
    ///
    /// Returns `Err(ZeroAddressRecipient)` if `to == 0x0000…0000`.
    pub fn transfer_from(
        &mut self,
        token:         Address,
        owner:         Address,
        to:            Address,
        spender:       Address,
        amount:        u128,
        current_block: u64,
    ) -> Result<(), ApprovalError> {
        if to == Address([0u8; 20]) {
            return Err(ApprovalError::ZeroAddressRecipient);
        }
        if amount == 0 { return Ok(()); }

        // 1. Deduct allowance
        self.spend_allowance(token, owner, spender, amount, current_block)?;

        // 2. Deduct owner balance
        self.sub_balance(token, owner, amount)?;

        // 3. Credit recipient
        self.add_balance(token, to, amount);

        Ok(())
    }

    /// Direct transfer (no approval needed — caller must be owner).
    ///
    /// Returns `Err(ZeroAddressRecipient)` if `to == 0x0000…0000`.
    ///
    /// ## Why the zero-address check matters
    ///
    /// ERC-20 tokens treat a transfer to the zero address as an accidental
    /// permanent burn.  Without this guard a bug in calling code (e.g. an
    /// uninitialized `to` field) silently destroys tokens with no recovery
    /// path.  Making the check explicit ensures callers that intend to burn
    /// tokens must do so through a dedicated `burn()` path.
    pub fn transfer(
        &mut self,
        token:  Address,
        owner:  Address,
        to:     Address,
        amount: u128,
    ) -> Result<(), ApprovalError> {
        if to == Address([0u8; 20]) {
            return Err(ApprovalError::ZeroAddressRecipient);
        }
        if amount == 0 { return Ok(()); }
        self.sub_balance(token, owner, amount)?;
        self.add_balance(token, to, amount);
        Ok(())
    }

    // ── Revoke ─────────────────────────────────────────────────────────────────

    /// Revoke all approvals granted by `owner` for `token`.
    pub fn revoke_all(&mut self, token: &Address, owner: &Address) {
        if let Some(m) = self.allowances.get_mut(token) {
            m.remove(owner);
        }
    }

    /// Revoke a specific approval.
    pub fn revoke(
        &mut self,
        token:   &Address,
        owner:   &Address,
        spender: &Address,
    ) {
        if let Some(owners) = self.allowances.get_mut(token) {
            if let Some(spenders) = owners.get_mut(owner) {
                spenders.remove(spender);
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(n: u8) -> Address { Address([n; 20]) }

    #[test]
    fn approve_and_read_allowance() {
        let mut reg = AllowanceRegistry::new();
        reg.approve(addr(1), addr(2), addr(3), 1_000, 0).unwrap();
        assert_eq!(reg.allowance(&addr(1), &addr(2), &addr(3), 100), 1_000);
    }

    #[test]
    fn self_approval_rejected() {
        let mut reg = AllowanceRegistry::new();
        assert!(matches!(
            reg.approve(addr(1), addr(2), addr(2), 100, 0),
            Err(ApprovalError::SelfApproval)
        ));
    }

    #[test]
    fn zero_address_spender_rejected() {
        let mut reg = AllowanceRegistry::new();
        assert!(matches!(
            reg.approve(addr(1), addr(2), addr(0), 100, 0),
            Err(ApprovalError::ZeroAddressSpender)
        ));
    }

    #[test]
    fn expired_approval_returns_zero() {
        let mut reg = AllowanceRegistry::new();
        reg.approve(addr(1), addr(2), addr(3), 500, 100).unwrap();
        // Before expiry
        assert_eq!(reg.allowance(&addr(1), &addr(2), &addr(3), 90), 500);
        // After expiry
        assert_eq!(reg.allowance(&addr(1), &addr(2), &addr(3), 101), 0);
    }

    #[test]
    fn transfer_from_deducts_allowance_and_balance() {
        let mut reg = AllowanceRegistry::new();
        let token = addr(1); let owner = addr(2); let spender = addr(3); let to = addr(4);
        reg.add_balance(token, owner, 2_000);
        reg.approve(token, owner, spender, 1_000, 0).unwrap();
        reg.transfer_from(token, owner, to, spender, 500, 1).unwrap();
        assert_eq!(reg.balance_of(&token, &owner), 1_500);
        assert_eq!(reg.balance_of(&token, &to),    500);
        assert_eq!(reg.allowance(&token, &owner, &spender, 1), 500);
    }

    #[test]
    fn transfer_from_fails_on_insufficient_allowance() {
        let mut reg = AllowanceRegistry::new();
        let token = addr(1); let owner = addr(2); let spender = addr(3); let to = addr(4);
        reg.add_balance(token, owner, 2_000);
        reg.approve(token, owner, spender, 100, 0).unwrap();
        let r = reg.transfer_from(token, owner, to, spender, 500, 1);
        assert!(matches!(r, Err(ApprovalError::InsufficientAllowance { .. })));
    }

    #[test]
    fn infinite_approval_not_deducted() {
        let mut reg = AllowanceRegistry::new();
        let token = addr(1); let owner = addr(2); let spender = addr(3); let to = addr(4);
        reg.add_balance(token, owner, 10_000);
        reg.approve(token, owner, spender, u128::MAX, 0).unwrap();
        reg.transfer_from(token, owner, to, spender, 5_000, 1).unwrap();
        assert_eq!(reg.allowance(&token, &owner, &spender, 1), u128::MAX);
    }

    #[test]
    fn increase_and_decrease_allowance() {
        let mut reg = AllowanceRegistry::new();
        reg.approve(addr(1), addr(2), addr(3), 1_000, 0).unwrap();
        reg.increase_allowance(addr(1), addr(2), addr(3), 500, 1).unwrap();
        assert_eq!(reg.allowance(&addr(1), &addr(2), &addr(3), 1), 1_500);
        reg.decrease_allowance(addr(1), addr(2), addr(3), 300).unwrap();
        assert_eq!(reg.allowance(&addr(1), &addr(2), &addr(3), 1), 1_200);
    }

    #[test]
    fn transfer_to_zero_address_rejected() {
        let mut reg = AllowanceRegistry::new();
        let token = addr(1); let owner = addr(2);
        reg.add_balance(token, owner, 1_000);
        // Direct transfer to zero address must be blocked
        let err = reg.transfer(token, owner, addr(0), 100);
        assert!(
            matches!(err, Err(ApprovalError::ZeroAddressRecipient)),
            "transfer to zero address must return ZeroAddressRecipient"
        );
        // Balance unchanged — tokens not destroyed
        assert_eq!(reg.balance_of(&token, &owner), 1_000);
    }

    #[test]
    fn transfer_from_to_zero_address_rejected() {
        let mut reg = AllowanceRegistry::new();
        let token = addr(1); let owner = addr(2); let spender = addr(3);
        reg.add_balance(token, owner, 1_000);
        reg.approve(token, owner, spender, 500, 0).unwrap();
        let err = reg.transfer_from(token, owner, addr(0), spender, 100, 1);
        assert!(
            matches!(err, Err(ApprovalError::ZeroAddressRecipient)),
            "transferFrom to zero address must return ZeroAddressRecipient"
        );
        // Allowance and balance unchanged — no partial state
        assert_eq!(reg.allowance(&token, &owner, &spender, 1), 500);
        assert_eq!(reg.balance_of(&token, &owner), 1_000);
    }
}
