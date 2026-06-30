//! ZBX Launchpad -- IDO (Initial DEX Offering) / token sale platform.
//!
//! Allows ZBX ecosystem projects to conduct fair token launches with:
//!   - Whitelist / KYC gating (optional)
//!   - Hard cap and soft cap
//!   - FCFS (first come first served) OR equal allocation (lottery)
//!   - Vesting schedule for purchased tokens
//!   - Automatic liquidity provision post-IDO (locks LP tokens)
//!   - Refund if soft cap not met
//!
//! ## IDO lifecycle
//!   1. Project registers IDO (governance must approve)
//!   2. Whitelist period: wallets register interest
//!   3. Sale period: whitelisted wallets buy tokens (FCFS or lottery)
//!   4a. If soft cap met: tokens distributed, liquidity added
//!   4b. If soft cap not met: all contributions refunded
//!
//! ## ZBX Launchpad tiers (based on staked ZBX)
//!   Bronze:   500 ZBX staked  -- guaranteed 1x allocation
//!   Silver:   2000 ZBX staked -- guaranteed 3x allocation
//!   Gold:     5000 ZBX staked -- guaranteed 7x allocation
//!   Diamond: 20000 ZBX staked -- guaranteed 15x + private round access

/// Launchpad allocation tiers based on staked ZBX.
pub const TIER_BRONZE_STAKE: u128  =     500 * 1_000_000_000_000_000_000;
pub const TIER_SILVER_STAKE: u128  =   2_000 * 1_000_000_000_000_000_000;
pub const TIER_GOLD_STAKE: u128    =   5_000 * 1_000_000_000_000_000_000;
pub const TIER_DIAMOND_STAKE: u128 =  20_000 * 1_000_000_000_000_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tier { None, Bronze, Silver, Gold, Diamond }

impl Tier {
    pub fn from_staked(staked_zbx: u128) -> Self {
        if staked_zbx >= TIER_DIAMOND_STAKE { Self::Diamond }
        else if staked_zbx >= TIER_GOLD_STAKE { Self::Gold }
        else if staked_zbx >= TIER_SILVER_STAKE { Self::Silver }
        else if staked_zbx >= TIER_BRONZE_STAKE { Self::Bronze }
        else { Self::None }
    }

    pub fn allocation_multiplier(&self) -> u32 {
        match self { Self::Bronze => 1, Self::Silver => 3, Self::Gold => 7, Self::Diamond => 15, Self::None => 0 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdoStatus { Draft, Whitelisting, Sale, Ended, Cancelled, Refunding }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaleType { Fcfs, EqualAllocation }

/// An IDO project registration.
#[derive(Debug, Clone)]
pub struct Ido {
    pub id:              u64,
    pub project_name:    String,
    pub token_contract:  [u8; 20],
    pub token_price:     u128,      // price per token in ZBX (8 decimals)
    pub tokens_for_sale: u128,      // total tokens available
    pub soft_cap:        u128,      // minimum ZBX raise (if not met: refund)
    pub hard_cap:        u128,      // maximum ZBX raise
    pub min_buy:         u128,      // minimum contribution per wallet
    pub max_buy:         u128,      // maximum contribution per wallet
    pub sale_type:       SaleType,
    pub whitelist_start: u64,
    pub sale_start:      u64,
    pub sale_end:        u64,
    pub vesting_months:  u8,        // 0 = no vesting, N = linear vest over N months
    pub status:          IdoStatus,
    pub total_raised:    u128,
    pub participants:    u32,
}

impl Ido {
    /// Can a wallet participate given their staked ZBX?
    pub fn check_eligibility(&self, staked_zbx: u128, is_whitelisted: bool) -> Result<Tier, LaunchpadError> {
        let tier = Tier::from_staked(staked_zbx);
        if tier == Tier::None && !is_whitelisted { return Err(LaunchpadError::NotEligible); }
        if self.status != IdoStatus::Sale { return Err(LaunchpadError::SaleNotActive); }
        Ok(tier)
    }

    /// Compute a wallet's max allocation based on tier.
    pub fn allocation_for_tier(&self, tier: &Tier) -> u128 {
        let base = self.min_buy;
        (base * tier.allocation_multiplier() as u128).min(self.max_buy)
    }

    /// Has the soft cap been met?
    pub fn soft_cap_met(&self) -> bool { self.total_raised >= self.soft_cap }

    /// Is the IDO fully subscribed (hard cap hit)?
    pub fn hard_cap_met(&self) -> bool { self.total_raised >= self.hard_cap }
}

/// Launchpad contribution record.
#[derive(Debug, Clone)]
pub struct Contribution {
    pub ido_id:          u64,
    pub contributor:     [u8; 20],
    pub zbx_contributed: u128,
    pub tokens_allocated: u128,
    pub claimed:         bool,
    pub refunded:        bool,
}

/// The ZBX Launchpad IDO platform.
pub struct ZbxLaunchpad {
    pub idos:          std::collections::HashMap<u64, Ido>,
    pub contributions: std::collections::HashMap<(u64, [u8; 20]), Contribution>,
    pub next_ido_id:   u64,
    pub treasury:      [u8; 20],
}

impl ZbxLaunchpad {
    pub fn new(treasury: [u8; 20]) -> Self {
        Self { idos: Default::default(), contributions: Default::default(), next_ido_id: 1, treasury }
    }

    /// Register a new IDO (governance approved).
    pub fn register_ido(&mut self, ido: Ido) -> u64 {
        let id = self.next_ido_id;
        self.next_ido_id += 1;
        self.idos.insert(id, ido);
        id
    }

    /// Contribute ZBX to an IDO during the sale period.
    pub fn contribute(
        &mut self,
        ido_id:      u64,
        contributor: [u8; 20],
        zbx_amount:  u128,
        staked_zbx:  u128,
        whitelisted: bool,
    ) -> Result<Contribution, LaunchpadError> {
        let ido = self.idos.get_mut(&ido_id).ok_or(LaunchpadError::IdoNotFound)?;
        let tier = ido.check_eligibility(staked_zbx, whitelisted)?;
        let max_alloc = ido.allocation_for_tier(&tier);

        let existing = self.contributions.get(&(ido_id, contributor)).map(|c| c.zbx_contributed).unwrap_or(0);
        if existing + zbx_amount > max_alloc { return Err(LaunchpadError::ExceedsAllocation); }
        if zbx_amount < ido.min_buy { return Err(LaunchpadError::BelowMinimum); }
        if ido.hard_cap_met() { return Err(LaunchpadError::HardCapReached); }

        let remaining_cap = ido.hard_cap - ido.total_raised;
        let actual_contribution = zbx_amount.min(remaining_cap);
        let tokens = actual_contribution * 1_000_000_000_000_000_000 / ido.token_price;

        ido.total_raised  += actual_contribution;
        ido.participants  += 1;

        let c = Contribution {
            ido_id, contributor,
            zbx_contributed: actual_contribution,
            tokens_allocated: tokens,
            claimed: false, refunded: false,
        };
        self.contributions.insert((ido_id, contributor), c.clone());
        Ok(c)
    }
}

#[derive(Debug)]
pub enum LaunchpadError {
    IdoNotFound, SaleNotActive, NotEligible, ExceedsAllocation,
    BelowMinimum, HardCapReached, AlreadyClaimed, AlreadyRefunded,
}