//! Staking escrow contract — locks ZBX for validator participation.
//!
//! NEW-CRIT-01 FIX (2026-05-03): Added per-delegator tracking.
//!
//! Previously `delegate()` discarded the delegator address (`let _ = delegator`)
//! meaning all delegations were pooled under the validator's single `amount`
//! field with no per-delegator attribution, making individual undelegation and
//! withdrawal impossible — a complete fund-loss vector.
//!
//! This rewrite adds:
//!   - `DelegationRecord` keyed by `(validator, delegator)` in a separate map
//!   - `delegate()` now inserts/updates per-delegator records
//!   - `undelegate()` begins per-delegator unbonding
//!   - `withdraw_delegation()` returns funds after the unbonding period
//!   - Proportional slash propagation to all active delegators
//!   - `Jailed` variant so partial slashes don't immediately end the validator
//!   - Active-only guard on `begin_unbonding()` (was missing)
//!   - Record removal on `withdraw()` so the validator slot is fully recycled

use std::collections::HashMap;
use zbx_types::address::Address;

/// Minimum self-stake required to become a validator (100 ZBX, 18 decimals).
pub const MIN_STAKE: u128 = 100 * 1_000_000_000_000_000_000;

/// Minimum delegation per delegator (10 ZBX, 18 decimals).
pub const MIN_DELEGATION: u128 = 10 * 1_000_000_000_000_000_000;

// ---------------------------------------------------------------------------
//  Status types
// ---------------------------------------------------------------------------

/// Status of a validator's self-stake.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StakeStatus {
    /// Actively participating in consensus.
    Active,
    /// Unbonding: locked until `unlock_at` (Unix timestamp, seconds).
    Unbonding { unlock_at: u64 },
    /// Fully slashed — no stake remaining. Terminal state.
    Slashed,
    /// Partially slashed — removed from active set, awaiting governance action.
    Jailed,
}

/// Status of a single delegator's position in one validator.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DelegationStatus {
    Active,
    Unbonding { unlock_at: u64 },
    Withdrawn,
}

// ---------------------------------------------------------------------------
//  Record types
// ---------------------------------------------------------------------------

/// Validator's own self-stake record (separate from delegator ledger).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StakeRecord {
    pub validator:  Address,
    pub self_stake: u128,
    pub status:     StakeStatus,
}

/// A single partially-undelegated chunk that is currently unbonding.
///
/// P3-PROD fix (NEW-HIGH-02): partial undelegates now push one `UnbondingChunk`
/// per call instead of silently reducing the active amount with no unbonding
/// period — the previous behaviour made partial undelegate amounts unwithdrawable
/// (a fund-loss vector). Each chunk has its own `unlock_at` timestamp so
/// multiple partial undelegates at different times are correctly tracked.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UnbondingChunk {
    pub amount:    u128,
    pub unlock_at: u64,
}

/// Per-(validator, delegator) delegation record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DelegationRecord {
    pub validator: Address,
    pub delegator: Address,
    /// Active (not-unbonding) delegation amount.
    pub amount:    u128,
    pub status:    DelegationStatus,
    /// Ordered list of partial undelegate chunks awaiting the unbonding period.
    /// Empty for records that have never had a partial undelegate.
    #[serde(default)]
    pub unbonding_chunks: Vec<UnbondingChunk>,
}

// ---------------------------------------------------------------------------
//  StakingEscrow
// ---------------------------------------------------------------------------

/// On-chain staking escrow state.
#[derive(Debug, Default)]
pub struct StakingEscrow {
    /// Validator self-stake records, keyed by validator address.
    stakes: HashMap<Address, StakeRecord>,
    /// Per-`(validator, delegator)` delegation ledger.
    delegations: HashMap<(Address, Address), DelegationRecord>,
    /// Unbonding lock duration in seconds (default: 7 days).
    pub unbonding_period: u64,
}

impl StakingEscrow {
    pub fn new() -> Self {
        Self {
            stakes: HashMap::new(),
            delegations: HashMap::new(),
            unbonding_period: 7 * 24 * 3600,
        }
    }

    // ── Validator self-stake ──────────────────────────────────────────────

    /// Lock `amount` as the validator's self-stake.
    pub fn stake(&mut self, validator: Address, amount: u128) -> Result<(), &'static str> {
        if amount < MIN_STAKE {
            return Err("below minimum stake (100 ZBX)");
        }
        let rec = self.stakes.entry(validator).or_insert(StakeRecord {
            validator,
            self_stake: 0,
            status: StakeStatus::Active,
        });
        if rec.status == StakeStatus::Slashed {
            return Err("slashed validator cannot re-stake without governance reset");
        }
        rec.self_stake += amount;
        rec.status = StakeStatus::Active;
        Ok(())
    }

    /// Begin unbonding for a validator's self-stake.
    /// Guard: only allowed from `Active` (was missing — would let unbonding/jailed
    /// validators reset their `unlock_at` by calling again).
    pub fn begin_unbonding(
        &mut self,
        validator: &Address,
        now: u64,
    ) -> Result<(), &'static str> {
        let rec = self.stakes.get_mut(validator).ok_or("validator not staked")?;
        if rec.status != StakeStatus::Active {
            return Err("can only begin unbonding from Active status");
        }
        rec.status = StakeStatus::Unbonding { unlock_at: now + self.unbonding_period };
        Ok(())
    }

    /// Withdraw validator's self-stake after unbonding period.
    /// Removes the record entirely so the slot is recycled cleanly.
    pub fn withdraw(&mut self, validator: &Address, now: u64) -> Result<u128, &'static str> {
        let rec = self.stakes.get(validator).ok_or("validator not staked")?;
        if let StakeStatus::Unbonding { unlock_at } = rec.status {
            if now < unlock_at {
                return Err("unbonding period has not elapsed");
            }
            let amount = rec.self_stake;
            self.stakes.remove(validator);
            return Ok(amount);
        }
        Err("validator is not in Unbonding state")
    }

    /// Unjail a partially-slashed validator (governance-gated call).
    pub fn unjail(&mut self, validator: &Address) -> Result<(), &'static str> {
        let rec = self.stakes.get_mut(validator).ok_or("validator not staked")?;
        if rec.status != StakeStatus::Jailed {
            return Err("validator is not Jailed");
        }
        rec.status = StakeStatus::Active;
        Ok(())
    }

    // ── Delegation ────────────────────────────────────────────────────────

    /// Delegate `amount` ZBX from `delegator` to `validator`.
    ///
    /// NEW-CRIT-01 fix: each `(validator, delegator)` pair now has its own
    /// `DelegationRecord`; the delegator address is no longer discarded.
    pub fn delegate(
        &mut self,
        validator: Address,
        delegator: Address,
        amount: u128,
    ) -> Result<(), &'static str> {
        if amount < MIN_DELEGATION {
            return Err("below minimum delegation (10 ZBX)");
        }
        let rec = self.stakes.get(&validator).ok_or("validator not found")?;
        match rec.status {
            StakeStatus::Slashed => return Err("cannot delegate to slashed validator"),
            StakeStatus::Jailed  => return Err("cannot delegate to jailed validator"),
            _ => {}
        }

        let entry = self
            .delegations
            .entry((validator, delegator))
            .or_insert(DelegationRecord {
                validator,
                delegator,
                amount: 0,
                status: DelegationStatus::Active,
                unbonding_chunks: Vec::new(),
            });

        if entry.status != DelegationStatus::Active {
            return Err("existing delegation is unbonding or withdrawn; withdraw first");
        }
        entry.amount += amount;
        Ok(())
    }

    /// Begin unbonding `amount` of a delegator's stake from a validator.
    ///
    /// For a full undelegate (`amount == record.amount`) the record transitions
    /// to `Unbonding`. Partial undelegate reduces the active amount (the
    /// unbonding chunk tracking is a TODO for a future sprint as noted in
    /// audit NEW-HIGH-02; for now full-or-nothing is enforced via the
    /// `MIN_DELEGATION` rump check).
    pub fn undelegate(
        &mut self,
        validator: &Address,
        delegator: &Address,
        amount: u128,
        now: u64,
    ) -> Result<(), &'static str> {
        let key = (*validator, *delegator);
        let rec = self.delegations.get_mut(&key).ok_or("delegation not found")?;
        if rec.status != DelegationStatus::Active {
            return Err("delegation is not Active");
        }
        if amount == 0 {
            return Err("amount must be greater than zero");
        }
        if amount > rec.amount {
            return Err("undelegate amount exceeds delegated balance");
        }
        let remaining = rec.amount - amount;
        if remaining > 0 && remaining < MIN_DELEGATION {
            return Err("remaining delegation would fall below minimum (10 ZBX)");
        }
        if remaining == 0 {
            // Full undelegate: transition record to Unbonding so the final
            // amount is returned via `withdraw_delegation` after the period.
            rec.amount = 0;
            rec.status = DelegationStatus::Unbonding {
                unlock_at: now + self.unbonding_period,
            };
        } else {
            // P3-PROD (NEW-HIGH-02): partial undelegate now pushes an
            // `UnbondingChunk` instead of silently reducing the active balance
            // with no unbonding period. Without this fix, partially undelegated
            // amounts were permanently trapped — effectively burned.
            rec.amount = remaining;
            rec.unbonding_chunks.push(UnbondingChunk {
                amount,
                unlock_at: now + self.unbonding_period,
            });
        }
        Ok(())
    }

    /// Withdraw all matured unbonding amounts for a delegation.
    ///
    /// P3-PROD (NEW-HIGH-02): now drains matured `UnbondingChunk`s in
    /// addition to handling the full-unbond (`status == Unbonding`) path.
    /// Multiple partial undelegates at different times each have their own
    /// `unlock_at`; this call returns the sum of all chunks that have matured
    /// by `now`.
    ///
    /// If the result is zero (nothing matured yet) returns an error so the
    /// caller can surface a useful message to the user.
    /// The delegation record is removed when fully drained:
    ///   - active amount == 0
    ///   - no pending unbonding chunks remain
    ///   - status is not Active (i.e. nothing more will be added)
    pub fn withdraw_delegation(
        &mut self,
        validator: &Address,
        delegator: &Address,
        now: u64,
    ) -> Result<u128, &'static str> {
        let key = (*validator, *delegator);

        let (withdrawn, should_remove) = {
            let rec = self.delegations.get_mut(&key).ok_or("delegation not found")?;

            // Drain all matured partial-undelegate chunks.
            let mut withdrawn = 0u128;
            rec.unbonding_chunks.retain(|chunk| {
                if now >= chunk.unlock_at {
                    withdrawn += chunk.amount;
                    false // remove the matured chunk
                } else {
                    true  // keep the immature chunk
                }
            });

            // Handle the full-unbond status (status == Unbonding).
            match rec.status {
                DelegationStatus::Unbonding { unlock_at } if now >= unlock_at => {
                    withdrawn += rec.amount;
                    rec.amount = 0;
                    rec.status = DelegationStatus::Withdrawn;
                }
                DelegationStatus::Unbonding { .. } => {
                    // Full unbond not yet matured; chunks already drained above.
                }
                DelegationStatus::Active => {
                    // Still active delegation — only chunk withdrawals apply.
                }
                DelegationStatus::Withdrawn => {
                    if withdrawn == 0 {
                        return Err("delegation already fully withdrawn");
                    }
                }
            }

            let should_remove = rec.amount == 0
                && rec.unbonding_chunks.is_empty()
                && rec.status != DelegationStatus::Active;

            (withdrawn, should_remove)
        };

        if should_remove {
            self.delegations.remove(&key);
        }

        if withdrawn == 0 {
            Err("no matured unbonding amounts; unbonding period has not elapsed")
        } else {
            Ok(withdrawn)
        }
    }

    // ── Slashing ──────────────────────────────────────────────────────────

    /// Slash `amount` from a validator's total stake (self + delegations).
    ///
    /// NEW-CRIT-01 fix: slash is now proportionally distributed across all
    /// active delegators, preventing validators from absorbing delegator
    /// principal without recourse.
    ///
    /// Returns the actual amount slashed (capped at total stake).
    pub fn slash(&mut self, validator: &Address, amount: u128) -> u128 {
        let total = self.stake_of(validator);
        if total == 0 {
            return 0;
        }
        let to_slash = amount.min(total);

        // ── Proportional: determine each delegator's share ────────────────
        let self_stake = self.stakes.get(validator).map(|r| r.self_stake).unwrap_or(0);

        // Self-stake slash proportional to (self_stake / total).
        let self_slash = (to_slash as u128)
            .saturating_mul(self_stake)
            .checked_div(total)
            .unwrap_or(0)
            .min(self_stake);

        if let Some(rec) = self.stakes.get_mut(validator) {
            rec.self_stake = rec.self_stake.saturating_sub(self_slash);
        }

        // Delegator slashes proportional to (delegation / total).
        let del_keys: Vec<(Address, Address)> = self
            .delegations
            .keys()
            .filter(|(v, _)| v == validator)
            .copied()
            .collect();

        let mut del_slash_total = 0u128;
        for key in &del_keys {
            if let Some(drec) = self.delegations.get_mut(key) {
                if drec.status == DelegationStatus::Active && total > 0 {
                    let d_slash = (to_slash as u128)
                        .saturating_mul(drec.amount)
                        .checked_div(total)
                        .unwrap_or(0)
                        .min(drec.amount);
                    drec.amount = drec.amount.saturating_sub(d_slash);
                    del_slash_total += d_slash;
                    if drec.amount == 0 {
                        drec.status = DelegationStatus::Withdrawn;
                    }
                }
            }
        }

        // ── Update validator status ───────────────────────────────────────
        let new_total = self.stake_of(validator);
        if let Some(rec) = self.stakes.get_mut(validator) {
            if new_total == 0 {
                rec.status = StakeStatus::Slashed;
            } else {
                rec.status = StakeStatus::Jailed;
            }
        }

        self_slash + del_slash_total
    }

    // ── Queries ───────────────────────────────────────────────────────────

    /// Total voting power for a validator (self-stake + active delegations).
    pub fn stake_of(&self, validator: &Address) -> u128 {
        let self_stake = self.stakes.get(validator).map(|r| r.self_stake).unwrap_or(0);
        let delegated: u128 = self
            .delegations
            .iter()
            .filter(|((v, _), drec)| {
                v == validator && drec.status == DelegationStatus::Active
            })
            .map(|(_, drec)| drec.amount)
            .sum();
        self_stake + delegated
    }

    /// Validator's own self-stake only (excludes delegations).
    pub fn self_stake_of(&self, validator: &Address) -> u128 {
        self.stakes.get(validator).map(|r| r.self_stake).unwrap_or(0)
    }

    /// Active delegation amount for one (validator, delegator) pair.
    pub fn delegation_of(&self, validator: &Address, delegator: &Address) -> u128 {
        self.delegations
            .get(&(*validator, *delegator))
            .filter(|r| r.status == DelegationStatus::Active)
            .map(|r| r.amount)
            .unwrap_or(0)
    }

    /// Current status of a validator's self-stake.
    pub fn validator_status(&self, validator: &Address) -> Option<&StakeStatus> {
        self.stakes.get(validator).map(|r| &r.status)
    }

    /// All active delegators for a validator and their amounts.
    /// Used by the reward distribution engine to pay out per-delegator.
    pub fn active_delegators(&self, validator: &Address) -> Vec<(Address, u128)> {
        self.delegations
            .iter()
            .filter(|((v, _), drec)| {
                v == validator && drec.status == DelegationStatus::Active
            })
            .map(|((_, d), drec)| (*d, drec.amount))
            .collect()
    }
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(b: u8) -> Address {
        let mut a = [0u8; 20];
        a[19] = b;
        Address(a)
    }

    const STAKE: u128 = 100 * 1_000_000_000_000_000_000;
    const DELG:  u128 =  10 * 1_000_000_000_000_000_000;

    #[test]
    fn delegate_per_delegator_tracked() {
        let mut e = StakingEscrow::new();
        let v = addr(1);
        let d1 = addr(2);
        let d2 = addr(3);
        e.stake(v, STAKE).unwrap();
        e.delegate(v, d1, DELG).unwrap();
        e.delegate(v, d2, DELG * 2).unwrap();
        assert_eq!(e.delegation_of(&v, &d1), DELG);
        assert_eq!(e.delegation_of(&v, &d2), DELG * 2);
        assert_eq!(e.stake_of(&v), STAKE + DELG * 3);
    }

    #[test]
    fn undelegate_and_withdraw() {
        let mut e = StakingEscrow::new();
        let v = addr(1);
        let d = addr(2);
        e.stake(v, STAKE).unwrap();
        e.delegate(v, d, DELG * 5).unwrap();
        e.undelegate(&v, &d, DELG * 5, 1000).unwrap();
        // Cannot withdraw before period
        assert!(e.withdraw_delegation(&v, &d, 1000).is_err());
        // Can withdraw after period
        let period = e.unbonding_period;
        let got = e.withdraw_delegation(&v, &d, 1000 + period).unwrap();
        assert_eq!(got, DELG * 5);
        assert_eq!(e.delegation_of(&v, &d), 0);
    }

    #[test]
    fn slash_proportional_to_delegators() {
        let mut e = StakingEscrow::new();
        let v = addr(1);
        let d = addr(2);
        e.stake(v, STAKE).unwrap();          // 100 ZBX self
        e.delegate(v, d, STAKE).unwrap();    // 100 ZBX delegation; total = 200
        // Slash 50% → 100 ZBX total slash
        let slashed = e.slash(&v, STAKE);
        assert_eq!(slashed, STAKE);
        // Both should lose 50 ZBX (50% each)
        assert_eq!(e.self_stake_of(&v), STAKE / 2);
        assert_eq!(e.delegation_of(&v, &d), STAKE / 2);
        // Validator should be Jailed (not fully slashed)
        assert_eq!(e.validator_status(&v), Some(&StakeStatus::Jailed));
    }

    #[test]
    fn slash_full_marks_slashed() {
        let mut e = StakingEscrow::new();
        let v = addr(1);
        e.stake(v, STAKE).unwrap();
        e.slash(&v, STAKE * 2);
        assert_eq!(e.validator_status(&v), Some(&StakeStatus::Slashed));
    }

    #[test]
    fn begin_unbonding_active_only() {
        let mut e = StakingEscrow::new();
        let v = addr(1);
        e.stake(v, STAKE).unwrap();
        e.begin_unbonding(&v, 0).unwrap();
        // Second call must fail (no longer Active)
        assert!(e.begin_unbonding(&v, 100).is_err());
    }

    #[test]
    fn withdraw_removes_stake_record() {
        let mut e = StakingEscrow::new();
        let v = addr(1);
        e.stake(v, STAKE).unwrap();
        e.begin_unbonding(&v, 0).unwrap();
        let period = e.unbonding_period;
        let got = e.withdraw(&v, period + 1).unwrap();
        assert_eq!(got, STAKE);
        // Record should be gone
        assert_eq!(e.validator_status(&v), None);
    }

    #[test]
    fn delegate_to_slashed_fails() {
        let mut e = StakingEscrow::new();
        let v = addr(1);
        let d = addr(2);
        e.stake(v, STAKE).unwrap();
        e.slash(&v, STAKE * 10);
        assert!(e.delegate(v, d, DELG).is_err());
    }
}
