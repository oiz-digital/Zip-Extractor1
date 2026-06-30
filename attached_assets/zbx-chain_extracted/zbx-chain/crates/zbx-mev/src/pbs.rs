//! PBS — Proposer-Builder Separation relay.
//!
//! Separates the roles of block *proposer* (validator) and block *builder*.
//! Prevents validators from directly extracting MEV.
//!
//! Flow:
//!   1. Builder submits bid + block header commitment to relay.
//!   2. Relay ranks bids and picks highest bidder.
//!   3. Validator requests the winning block from relay.
//!   4. Relay reveals full block body only after validator signs the header.
//!   5. If validator reveals bid amount without using it → builder slashed.

use crate::{builder::BuilderBid, error::MevError};
use std::collections::HashMap;

/// PBS relay: collects builder bids and coordinates with validators.
pub struct PbsRelay {
    /// All bids for each block slot: block_number → list of bids.
    bids: HashMap<u64, Vec<BuilderBid>>,
    /// Minimum bid (prevents spam).
    min_bid: u128,
}

/// A slot auction result.
#[derive(Debug, Clone)]
pub struct SlotAuction {
    pub block_number: u64,
    pub winner:       [u8; 20],
    pub bid_amount:   u128,
    pub block_root:   [u8; 32],
}

impl PbsRelay {
    pub fn new(min_bid: u128) -> Self {
        Self { bids: HashMap::new(), min_bid }
    }

    /// Submit a builder bid for a slot.
    pub fn submit_bid(&mut self, bid: BuilderBid) -> Result<(), MevError> {
        if bid.bid_amount < self.min_bid {
            return Err(MevError::BidTooLow { min: self.min_bid, bid: bid.bid_amount });
        }
        self.bids.entry(bid.block_number).or_default().push(bid);
        Ok(())
    }

    /// Get the winning bid for a block slot.
    pub fn winning_bid(&self, block_number: u64) -> Option<&BuilderBid> {
        self.bids.get(&block_number)
            .and_then(|bids| bids.iter().max_by_key(|b| b.bid_amount))
    }

    /// Run the auction and return the winner.
    pub fn run_auction(&self, block_number: u64) -> Option<SlotAuction> {
        let winner = self.winning_bid(block_number)?;
        Some(SlotAuction {
            block_number,
            winner:      winner.builder,
            bid_amount:  winner.bid_amount,
            block_root:  winner.block_root,
        })
    }

    /// Clear expired bids (older than current block).
    pub fn clear_expired(&mut self, current_block: u64) {
        self.bids.retain(|&bn, _| bn >= current_block);
    }
}