//! ERC-1155 game item state — off-chain mirror of ZbxGameItems.sol.
//!
//! Used by game servers to query item balances, attributes, and metadata
//! without on-chain RPC calls on every game tick.

use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use zbx_types::address::Address;

/// Item type definition (mirrors Solidity struct ItemType).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemType {
    pub type_id:          u64,
    pub name:             String,
    pub base_uri:         String,
    pub max_supply:       u64,   // 0 = unlimited
    pub total_minted:     u64,
    pub soulbound:        bool,
    pub royalty_bps:      u16,   // basis points (0-1000)
    pub royalty_recipient: Address,
    /// On-chain attributes (e.g. "damage" → "150").
    pub attributes:       HashMap<String, String>,
}

/// Player balance: type_id → quantity.
pub type ItemBalances = HashMap<u64, u64>;

/// Off-chain item state index.
#[derive(Debug, Default)]
pub struct ItemState {
    /// type_id → ItemType
    pub types:    HashMap<u64, ItemType>,
    /// player address → ItemBalances
    pub balances: HashMap<Address, ItemBalances>,
    next_type_id: u64,
}

impl ItemState {
    pub fn new() -> Self { Self::default() }

    /// Register a new item type.
    pub fn create_type(
        &mut self,
        name:             String,
        base_uri:         String,
        max_supply:       u64,
        soulbound:        bool,
        royalty_bps:      u16,
        royalty_recipient: Address,
    ) -> u64 {
        self.next_type_id += 1;
        let id = self.next_type_id;
        self.types.insert(id, ItemType {
            type_id:          id,
            name,
            base_uri,
            max_supply,
            total_minted:     0,
            soulbound,
            royalty_bps,
            royalty_recipient,
            attributes:       HashMap::new(),
        });
        id
    }

    /// Set an attribute on a type (e.g. "damage" = "150").
    pub fn set_attribute(&mut self, type_id: u64, key: String, value: String) {
        if let Some(t) = self.types.get_mut(&type_id) {
            t.attributes.insert(key, value);
        }
    }

    /// Mint items of a given type to a player.
    pub fn mint(&mut self, to: Address, type_id: u64, amount: u64) -> Result<(), &'static str> {
        let t = self.types.get_mut(&type_id)
            .ok_or("Items: type not found")?;

        if t.max_supply > 0 && t.total_minted + amount > t.max_supply {
            return Err("Items: max supply reached");
        }
        t.total_minted += amount;

        let bal = self.balances.entry(to).or_default()
            .entry(type_id).or_insert(0);
        *bal += amount;
        Ok(())
    }

    /// Burn items from a player (item consumed in game).
    pub fn burn(&mut self, from: &Address, type_id: u64, amount: u64) -> Result<(), &'static str> {
        let bal = self.balances
            .get_mut(from).ok_or("Items: player not found")?
            .get_mut(&type_id).ok_or("Items: item not found")?;
        if *bal < amount { return Err("Items: insufficient balance"); }
        *bal -= amount;
        Ok(())
    }

    /// Transfer items between players (respects soulbound flag).
    pub fn transfer(
        &mut self,
        from:    Address,
        to:      Address,
        type_id: u64,
        amount:  u64,
    ) -> Result<(), &'static str> {
        if let Some(t) = self.types.get(&type_id) {
            if t.soulbound { return Err("Items: soulbound"); }
        } else {
            return Err("Items: type not found");
        }
        self.burn(&from, type_id, amount)?;
        let bal = self.balances.entry(to).or_default()
            .entry(type_id).or_insert(0);
        *bal += amount;
        Ok(())
    }

    pub fn balance_of(&self, player: &Address, type_id: u64) -> u64 {
        self.balances.get(player)
            .and_then(|b| b.get(&type_id))
            .copied()
            .unwrap_or(0)
    }
}
