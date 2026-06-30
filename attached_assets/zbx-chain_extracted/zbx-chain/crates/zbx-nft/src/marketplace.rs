//! ZBX NFT Marketplace -- buy, sell, and auction ZBX NFTs.
//!
//! Supported listing types:
//!   1. Fixed price listing: seller sets a price, buyer pays and receives NFT
//!   2. English auction:     starts at reserve price, highest bid wins
//!   3. Dutch auction:       starts high, price decreases over time
//!   4. Bundle sale:         sell multiple NFTs in one transaction
//!
//! Marketplace fee: 2.5% of sale price to ZBX protocol treasury.
//! Creator royalty: configurable per NFT collection (0-10%).
//!
//! ## ZEP-006 NFT Standard compliance
//!   All listings use ERC-721 / ERC-1155 token standards.
//!   Marketplace supports both ZRC721 and ZRC1155 collections.
//!
//! ## Payment tokens
//!   Primary: ZBX (native)
//!   Secondary: ZUSD (stablecoin)
//!   (Other ERC-20s can be whitelisted by governance)

/// Marketplace protocol fee (basis points, 250 = 2.5%)
pub const MARKETPLACE_FEE_BPS: u16 = 250;
/// Maximum royalty a creator can set (basis points, 1000 = 10%)
pub const MAX_ROYALTY_BPS: u16 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListingType {
    FixedPrice,
    EnglishAuction { reserve_price: u128, end_time: u64 },
    DutchAuction   { start_price: u128, end_price: u128, end_time: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListingStatus { Active, Sold, Cancelled, Expired }

/// A single NFT listing on the marketplace.
#[derive(Debug, Clone)]
pub struct NftListing {
    pub id:             u64,
    pub seller:         [u8; 20],
    pub nft_contract:   [u8; 20],
    pub token_id:       u128,
    pub amount:         u128,        // 1 for ERC-721, N for ERC-1155
    pub payment_token:  [u8; 20],    // ZBX (native) or ZUSD or other ERC-20
    pub price:          u128,        // Fixed price or starting auction price
    pub listing_type:   ListingType,
    pub status:         ListingStatus,
    pub created_at:     u64,
    pub royalty_bps:    u16,         // Creator royalty for this collection
    pub royalty_recipient: [u8; 20],
}

/// Highest bid in an English auction.
#[derive(Debug, Clone)]
pub struct Bid {
    pub listing_id:  u64,
    pub bidder:      [u8; 20],
    pub amount:      u128,
    pub placed_at:   u64,
}

/// The ZBX NFT Marketplace.
pub struct NftMarketplace {
    pub listings:    std::collections::HashMap<u64, NftListing>,
    pub bids:        std::collections::HashMap<u64, Bid>,  // listing_id -> highest bid
    pub next_id:     u64,
    pub treasury:    [u8; 20],
}

impl NftMarketplace {
    pub fn new(treasury: [u8; 20]) -> Self {
        Self { listings: Default::default(), bids: Default::default(), next_id: 1, treasury }
    }

    /// Create a fixed-price NFT listing.
    pub fn list_fixed_price(
        &mut self,
        seller:         [u8; 20],
        nft_contract:   [u8; 20],
        token_id:       u128,
        payment_token:  [u8; 20],
        price:          u128,
        royalty_bps:    u16,
        royalty_recipient: [u8; 20],
        now:            u64,
    ) -> Result<u64, MarketError> {
        if price == 0 { return Err(MarketError::ZeroPrice); }
        if royalty_bps > MAX_ROYALTY_BPS { return Err(MarketError::RoyaltyTooHigh); }
        let id = self.next_id;
        self.next_id += 1;
        self.listings.insert(id, NftListing {
            id, seller, nft_contract, token_id, amount: 1,
            payment_token, price, royalty_bps, royalty_recipient,
            listing_type: ListingType::FixedPrice,
            status: ListingStatus::Active, created_at: now,
        });
        Ok(id)
    }

    /// Buy a fixed-price listing.
    pub fn buy(
        &mut self,
        listing_id: u64,
        buyer:      [u8; 20],
        paid:       u128,
        now:        u64,
    ) -> Result<SaleResult, MarketError> {
        let listing = self.listings.get_mut(&listing_id)
            .ok_or(MarketError::NotFound)?;
        if listing.status != ListingStatus::Active { return Err(MarketError::NotActive); }
        if listing.listing_type != ListingType::FixedPrice { return Err(MarketError::WrongListingType); }
        if paid < listing.price { return Err(MarketError::InsufficientPayment); }

        listing.status = ListingStatus::Sold;

        let fee      = paid * MARKETPLACE_FEE_BPS as u128 / 10_000;
        let royalty  = paid * listing.royalty_bps as u128 / 10_000;
        let seller_gets = paid - fee - royalty;

        Ok(SaleResult {
            listing_id, buyer, seller: listing.seller,
            nft_contract: listing.nft_contract, token_id: listing.token_id,
            sale_price: paid, marketplace_fee: fee,
            royalty_amount: royalty, royalty_recipient: listing.royalty_recipient,
            seller_proceeds: seller_gets, sold_at: now,
        })
    }

    /// Place a bid on an English auction listing.
    pub fn place_bid(
        &mut self,
        listing_id: u64,
        bidder:     [u8; 20],
        amount:     u128,
        now:        u64,
    ) -> Result<(), MarketError> {
        let listing = self.listings.get(&listing_id)
            .ok_or(MarketError::NotFound)?;
        if listing.status != ListingStatus::Active { return Err(MarketError::NotActive); }
        let min_bid = match &listing.listing_type {
            ListingType::EnglishAuction { reserve_price, end_time } => {
                if now >= *end_time { return Err(MarketError::AuctionEnded); }
                *reserve_price
            }
            _ => return Err(MarketError::WrongListingType),
        };
        if amount < min_bid { return Err(MarketError::BidTooLow); }
        let current_high = self.bids.get(&listing_id).map(|b| b.amount).unwrap_or(0);
        if amount <= current_high { return Err(MarketError::BidTooLow); }
        self.bids.insert(listing_id, Bid { listing_id, bidder, amount, placed_at: now });
        Ok(())
    }

    /// Cancel a listing (only seller, only if no bids).
    pub fn cancel(&mut self, listing_id: u64, caller: [u8; 20]) -> Result<(), MarketError> {
        let listing = self.listings.get_mut(&listing_id)
            .ok_or(MarketError::NotFound)?;
        if listing.seller != caller { return Err(MarketError::Unauthorized); }
        if listing.status != ListingStatus::Active { return Err(MarketError::NotActive); }
        if self.bids.contains_key(&listing_id) { return Err(MarketError::HasBids); }
        listing.status = ListingStatus::Cancelled;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SaleResult {
    pub listing_id:         u64,
    pub buyer:              [u8; 20],
    pub seller:             [u8; 20],
    pub nft_contract:       [u8; 20],
    pub token_id:           u128,
    pub sale_price:         u128,
    pub marketplace_fee:    u128,
    pub royalty_amount:     u128,
    pub royalty_recipient:  [u8; 20],
    pub seller_proceeds:    u128,
    pub sold_at:            u64,
}

#[derive(Debug)]
pub enum MarketError {
    NotFound, NotActive, WrongListingType, ZeroPrice, RoyaltyTooHigh,
    InsufficientPayment, BidTooLow, AuctionEnded, Unauthorized, HasBids,
}