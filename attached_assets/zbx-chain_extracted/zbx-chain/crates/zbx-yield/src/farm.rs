//! YieldFarm — LP token staking with per-block ZBX emission.
//!
//! Modelled after Masterchef v2 (SushiSwap).
//! Each farm has an allocation point; ZBX emission per block is split
//! proportionally across all farms by allocation points.
//!
//! Security:
//! * `emergencyWithdraw()` skips reward accrual — no reward drain on emergency exit
//! * Reward debt prevents double-claiming across deposits/withdrawals
//! * Zero allocation farms earn nothing (emission = 0)
//! * Total alloc tracked to prevent divide-by-zero

use std::collections::HashMap;
use zbx_types::address::Address;

/// Farm error type.
#[derive(Debug, thiserror::Error)]
pub enum FarmError {
    #[error("farm {0} not found")]
    FarmNotFound(u32),
    #[error("insufficient staked balance: have {have}, want {want}")]
    InsufficientBalance { have: u128, want: u128 },
    #[error("zero amount not allowed")]
    ZeroAmount,
    #[error("farm already exists with id {0}")]
    Duplicate(u32),
}

/// Global per-block ZBX emission (default: 10 ZBX/block, 18 decimals).
pub const DEFAULT_ZBX_PER_BLOCK: u128 = 10 * 1_000_000_000_000_000_000;
/// Accumulator precision (1e12 to avoid integer division loss).
pub const ACC_PRECISION: u128 = 1_000_000_000_000;

/// One LP staking farm.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Farm {
    pub id:             u32,
    /// Description / LP token symbol.
    pub name:           String,
    /// Allocation points — proportional share of ZBX emission.
    pub alloc_points:   u64,
    /// Total LP tokens staked in this farm.
    pub total_staked:   u128,
    /// Accumulated rewards per staked token (× ACC_PRECISION).
    pub acc_zbx_per_share: u128,
    /// Block at which acc_zbx_per_share was last updated.
    pub last_reward_block: u64,
}

/// Per-(user, farm) staking position.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct UserInfo {
    /// LP tokens the user has staked.
    pub amount:       u128,
    /// Reward debt — subtracted when computing pending rewards.
    pub reward_debt:  u128,
}

/// Master yield farm controller.
#[derive(Debug)]
pub struct YieldFarm {
    farms:             HashMap<u32, Farm>,
    user_info:         HashMap<(Address, u32), UserInfo>,
    pub total_alloc:   u64,
    pub zbx_per_block: u128,
    /// Pending harvested rewards per user (not yet claimed).
    pending_rewards:   HashMap<Address, u128>,
}

impl YieldFarm {
    pub fn new(zbx_per_block: u128) -> Self {
        Self {
            farms:           HashMap::new(),
            user_info:       HashMap::new(),
            total_alloc:     0,
            zbx_per_block,
            pending_rewards: HashMap::new(),
        }
    }

    pub fn default_emission() -> Self {
        Self::new(DEFAULT_ZBX_PER_BLOCK)
    }

    // ── Farm management ───────────────────────────────────────────────────

    /// Add a new farm. Returns its id.
    pub fn add_farm(
        &mut self,
        id:          u32,
        name:        String,
        alloc_points: u64,
        current_block: u64,
    ) -> Result<u32, FarmError> {
        if self.farms.contains_key(&id) {
            return Err(FarmError::Duplicate(id));
        }
        self.total_alloc += alloc_points;
        self.farms.insert(id, Farm {
            id,
            name,
            alloc_points,
            total_staked: 0,
            acc_zbx_per_share: 0,
            last_reward_block: current_block,
        });
        Ok(id)
    }

    /// Update a farm's allocation points (governance action).
    pub fn set_alloc_points(
        &mut self,
        farm_id:     u32,
        new_alloc:   u64,
        current_block: u64,
    ) -> Result<(), FarmError> {
        self.update_farm(farm_id, current_block)?;
        let farm = self.farms.get_mut(&farm_id).ok_or(FarmError::FarmNotFound(farm_id))?;
        // DEFI-03 fix: use saturating_add instead of unchecked `+` to prevent
        // u64 overflow when new_alloc is large.  Pre-fix:
        //   `self.total_alloc.saturating_sub(...) + new_alloc`
        // could overflow u64 and panic (debug) / wrap (release) for very high
        // governance-supplied alloc values.
        self.total_alloc = self.total_alloc
            .saturating_sub(farm.alloc_points)
            .saturating_add(new_alloc);
        farm.alloc_points = new_alloc;
        Ok(())
    }

    // ── Reward accrual ─────────────────────────────────────────────────────

    /// Accrue rewards for one farm up to `current_block`.
    pub fn update_farm(&mut self, farm_id: u32, current_block: u64) -> Result<(), FarmError> {
        let farm = self.farms.get_mut(&farm_id).ok_or(FarmError::FarmNotFound(farm_id))?;
        if current_block <= farm.last_reward_block { return Ok(()); }
        if farm.total_staked == 0 || farm.alloc_points == 0 || self.total_alloc == 0 {
            farm.last_reward_block = current_block;
            return Ok(());
        }
        let blocks = (current_block - farm.last_reward_block) as u128;
        let farm_share = (farm.alloc_points as u128) * blocks * self.zbx_per_block
            / (self.total_alloc as u128);
        farm.acc_zbx_per_share += farm_share * ACC_PRECISION / farm.total_staked;
        farm.last_reward_block = current_block;
        Ok(())
    }

    /// Pending (unharvested) rewards for `user` in `farm_id`.
    pub fn pending_reward(&self, farm_id: u32, user: Address) -> u128 {
        let farm = match self.farms.get(&farm_id) { Some(f) => f, None => return 0 };
        let info = self.user_info.get(&(user, farm_id)).cloned().unwrap_or_default();
        let acc = farm.acc_zbx_per_share;
        if info.amount == 0 { return 0; }
        (info.amount * acc / ACC_PRECISION).saturating_sub(info.reward_debt)
    }

    // ── User operations ────────────────────────────────────────────────────

    /// Stake `amount` LP tokens in `farm_id`. Harvests any pending rewards.
    pub fn deposit(
        &mut self,
        farm_id:       u32,
        user:          Address,
        amount:        u128,
        current_block: u64,
    ) -> Result<u128, FarmError> {
        if amount == 0 { return Err(FarmError::ZeroAmount); }
        self.update_farm(farm_id, current_block)?;
        let acc = self.farms.get(&farm_id).ok_or(FarmError::FarmNotFound(farm_id))?.acc_zbx_per_share;
        let info = self.user_info.entry((user, farm_id)).or_default();

        // Harvest pending before deposit
        let pending = (info.amount * acc / ACC_PRECISION).saturating_sub(info.reward_debt);
        if pending > 0 {
            *self.pending_rewards.entry(user).or_default() += pending;
        }

        // DEFI-03 fix: saturating_add prevents u128 panic/wrap on astronomically
        // large deposits (theoretical — but safer than unchecked `+=`).
        info.amount = info.amount.saturating_add(amount);
        info.reward_debt = info.amount * acc / ACC_PRECISION;
        self.farms.get_mut(&farm_id).unwrap().total_staked += amount;
        Ok(pending)
    }

    /// Unstake `amount` LP tokens from `farm_id`. Harvests any pending rewards.
    pub fn withdraw(
        &mut self,
        farm_id:       u32,
        user:          Address,
        amount:        u128,
        current_block: u64,
    ) -> Result<u128, FarmError> {
        if amount == 0 { return Err(FarmError::ZeroAmount); }
        self.update_farm(farm_id, current_block)?;
        let acc = self.farms.get(&farm_id).ok_or(FarmError::FarmNotFound(farm_id))?.acc_zbx_per_share;
        let info = self.user_info.get_mut(&(user, farm_id))
            .ok_or(FarmError::InsufficientBalance { have: 0, want: amount })?;

        if info.amount < amount {
            return Err(FarmError::InsufficientBalance { have: info.amount, want: amount });
        }

        let pending = (info.amount * acc / ACC_PRECISION).saturating_sub(info.reward_debt);
        if pending > 0 {
            *self.pending_rewards.entry(user).or_default() += pending;
        }

        info.amount -= amount;
        info.reward_debt = info.amount * acc / ACC_PRECISION;
        self.farms.get_mut(&farm_id).unwrap().total_staked =
            self.farms[&farm_id].total_staked.saturating_sub(amount);
        Ok(pending)
    }

    /// Emergency withdraw — returns staked LP tokens with NO reward accrual.
    /// Use only when the contract is compromised or urgently migrating.
    pub fn emergency_withdraw(
        &mut self,
        farm_id: u32,
        user:    Address,
    ) -> Result<u128, FarmError> {
        let info = self.user_info.get_mut(&(user, farm_id))
            .ok_or(FarmError::InsufficientBalance { have: 0, want: 0 })?;
        let staked = info.amount;
        info.amount      = 0;
        info.reward_debt = 0;
        if let Some(farm) = self.farms.get_mut(&farm_id) {
            farm.total_staked = farm.total_staked.saturating_sub(staked);
        }
        Ok(staked)
    }

    /// Claim all pending rewards for `user` across all farms.
    pub fn claim(&mut self, user: Address) -> u128 {
        self.pending_rewards.remove(&user).unwrap_or(0)
    }

    pub fn farm(&self, id: u32) -> Option<&Farm> { self.farms.get(&id) }
    pub fn user_info(&self, farm_id: u32, user: &Address) -> UserInfo {
        self.user_info.get(&(*user, farm_id)).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(v: u8) -> Address { Address([v; 20]) }

    #[test]
    fn deposit_and_withdraw_accrues_rewards() {
        let mut yf = YieldFarm::new(10_000); // 10_000 per block
        yf.add_farm(0, "ZBX/ZUSD".into(), 100, 0).unwrap();
        yf.deposit(0, addr(1), 1_000, 0).unwrap();
        // 10 blocks later
        yf.update_farm(0, 10).unwrap();
        let pending = yf.pending_reward(0, addr(1));
        assert!(pending > 0, "should have pending rewards after 10 blocks");
        let harvested = yf.withdraw(0, addr(1), 1_000, 10).unwrap();
        assert_eq!(harvested, pending);
    }

    #[test]
    fn two_stakers_proportional_rewards() {
        let mut yf = YieldFarm::new(1_000_000);
        yf.add_farm(0, "LP".into(), 100, 0).unwrap();
        yf.deposit(0, addr(1), 1_000, 0).unwrap();
        yf.deposit(0, addr(2), 3_000, 0).unwrap(); // 3× stake
        yf.update_farm(0, 100).unwrap();
        let p1 = yf.pending_reward(0, addr(1));
        let p2 = yf.pending_reward(0, addr(2));
        // addr(2) should have ~3× the rewards of addr(1)
        assert!(p2 > p1 * 2, "addr(2) should earn proportionally more");
    }

    #[test]
    fn emergency_withdraw_no_reward() {
        let mut yf = YieldFarm::new(10_000);
        yf.add_farm(0, "LP".into(), 100, 0).unwrap();
        yf.deposit(0, addr(1), 500, 0).unwrap();
        yf.update_farm(0, 50).unwrap();
        let returned = yf.emergency_withdraw(0, addr(1)).unwrap();
        assert_eq!(returned, 500);
        // No rewards should be claimable
        assert_eq!(yf.claim(addr(1)), 0);
    }

    #[test]
    fn zero_alloc_farm_earns_nothing() {
        let mut yf = YieldFarm::new(10_000);
        yf.add_farm(0, "LP".into(), 0, 0).unwrap();
        yf.deposit(0, addr(1), 1_000, 0).unwrap();
        yf.update_farm(0, 100).unwrap();
        assert_eq!(yf.pending_reward(0, addr(1)), 0);
    }

    #[test]
    fn duplicate_farm_rejected() {
        let mut yf = YieldFarm::new(10_000);
        yf.add_farm(0, "LP".into(), 100, 0).unwrap();
        assert!(matches!(yf.add_farm(0, "LP2".into(), 50, 0), Err(FarmError::Duplicate(0))));
    }

    #[test]
    fn zero_deposit_rejected() {
        let mut yf = YieldFarm::new(10_000);
        yf.add_farm(0, "LP".into(), 100, 0).unwrap();
        assert!(matches!(yf.deposit(0, addr(1), 0, 0), Err(FarmError::ZeroAmount)));
    }
}
