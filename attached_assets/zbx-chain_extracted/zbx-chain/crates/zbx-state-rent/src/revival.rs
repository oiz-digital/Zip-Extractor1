//! Account revival — restores hibernated accounts that pay back-rent.

use crate::error::RentError;

/// A hibernated account record (stripped from active trie, stored in revival DB).
#[derive(Debug, Clone)]
pub struct HibernatedAccount {
    pub address:          [u8; 20],
    pub state_root:       [u8; 32],  // Merkle root of the account's storage
    pub slot_count:       u64,
    pub hibernated_block: u64,
    pub back_rent_wei:    u128,      // Amount owed to revive
}

/// Manages the lifecycle of hibernated accounts.
pub struct RevivalManager {
    pub hibernated: Vec<HibernatedAccount>,
}

impl RevivalManager {
    pub fn new() -> Self {
        Self { hibernated: Vec::new() }
    }

    /// Hibernate an account (move it out of the active trie).
    pub fn hibernate(&mut self, account: HibernatedAccount) {
        self.hibernated.push(account);
    }

    /// Revive an account that has paid its back-rent.
    pub fn revive(
        &mut self,
        address: &[u8; 20],
        payment: u128,
    ) -> Result<HibernatedAccount, RentError> {
        let pos = self.hibernated.iter().position(|a| &a.address == address)
            .ok_or(RentError::AccountNotHibernated)?;
        let acc = &self.hibernated[pos];
        if payment < acc.back_rent_wei {
            return Err(RentError::InsufficientRevivalPayment {
                required: acc.back_rent_wei,
                provided: payment,
            });
        }
        Ok(self.hibernated.remove(pos))
    }

    /// Check whether an account has expired (dormant past EXPIRY_BLOCKS).
    pub fn is_expired(&self, address: &[u8; 20], current_block: u64, expiry_blocks: u64) -> bool {
        self.hibernated.iter().any(|a| {
            &a.address == address
                && current_block.saturating_sub(a.hibernated_block) >= expiry_blocks
        })
    }
}

impl Default for RevivalManager {
    fn default() -> Self { Self::new() }
}
