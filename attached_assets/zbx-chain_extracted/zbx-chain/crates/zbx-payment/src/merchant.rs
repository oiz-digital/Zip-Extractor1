//! Merchant registry — off-chain cache of ZbxPaymentGateway merchant records.

use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use zbx_types::address::Address;

/// Off-chain merchant record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Merchant {
    pub merchant_id:    [u8; 32],
    pub owner:          Address,
    pub name:           String,
    pub payout_address: Address,
    pub active:         bool,
    /// Total lifetime payments received, per token (for analytics).
    pub lifetime_volume: HashMap<Address, u128>,
}

/// In-memory merchant registry.
#[derive(Debug, Default)]
pub struct MerchantRegistry {
    merchants: HashMap<[u8; 32], Merchant>,
    /// owner address → list of merchantIds
    by_owner:  HashMap<Address, Vec<[u8; 32]>>,
}

impl MerchantRegistry {
    pub fn new() -> Self { Self::default() }

    pub fn insert(&mut self, m: Merchant) {
        self.by_owner.entry(m.owner).or_default().push(m.merchant_id);
        self.merchants.insert(m.merchant_id, m);
    }

    pub fn get(&self, id: &[u8; 32]) -> Option<&Merchant> {
        self.merchants.get(id)
    }

    pub fn get_mut(&mut self, id: &[u8; 32]) -> Option<&mut Merchant> {
        self.merchants.get_mut(id)
    }

    /// All merchants owned by an address.
    pub fn by_owner(&self, owner: &Address) -> Vec<&Merchant> {
        self.by_owner.get(owner)
            .map(|ids| ids.iter().filter_map(|id| self.merchants.get(id)).collect())
            .unwrap_or_default()
    }

    /// Record a payment against a merchant's volume stats.
    pub fn record_payment(&mut self, id: &[u8; 32], token: Address, amount: u128) {
        if let Some(m) = self.merchants.get_mut(id) {
            *m.lifetime_volume.entry(token).or_insert(0) += amount;
        }
    }
}
