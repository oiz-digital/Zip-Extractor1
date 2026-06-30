//! Block reward and fee distribution to validators and delegators.
//!
//! ## Reward-split model (STK-RWD-06)
//!
//! Each active validator's committee share is split between two pools:
//!
//! * `pending_rewards` — the validator's own slice:
//!   self-stake proportional share + commission on the delegated portion.
//! * `delegator_reward_pool` — delegators' net share (post-commission),
//!   claimable individually via `ClaimDelegatorRewards`.
//!
//! The proposer bonus (10% of total block reward) always goes entirely to the
//! proposer's own `pending_rewards`; it is never split to the delegator pool.
//!
//! ## Previous bugs fixed
//!
//! * STK-RWD-01 (double-counted commission): the old code credited the proposer
//!   an extra commission amount *on top of* their full weight-proportional share.
//!   Fixed in a prior pass; the current split model is consistent with it.
//! * STK-RWD-06 (delegators had no claimable path): `distribute_block_reward`
//!   previously put every wei into `pending_rewards` on the validator, leaving
//!   delegators with no way to claim their share.  Fixed here by splitting at
//!   distribution time so the delegator pool is always funded on-chain.

use crate::validator::ValidatorSet;
use zbx_types::{address::Address, block_reward_at};
use tracing::debug;

/// Handles reward calculation and distribution after each block.
pub struct RewardDistributor;

impl RewardDistributor {
    /// Distribute the accumulated block reward + fees to the proposer and active committee.
    ///
    /// ## Reward interval (STK-INTERVAL-01)
    ///
    /// This function is a **no-op on every block except multiples of
    /// `REWARD_INTERVAL`** (default: 100).  At each boundary block the
    /// function:
    ///
    /// 1. Sums the protocol base subsidy for every block in the window
    ///    (`block_height - REWARD_INTERVAL + 1` … `block_height`), correctly
    ///    handling halvings that straddle the boundary.
    /// 2. Adds `accumulated_fees` — the caller (block executor) **must**
    ///    accumulate transaction fees across all blocks in the interval and
    ///    pass the total here at the boundary.
    ///
    /// ## Reward split
    ///
    /// 1. `proposer_bonus` (10% of interval total) → proposer's `pending_rewards`.
    /// 2. For each active validator proportionally to `total_stake()`:
    ///    * If no delegators: full share → `pending_rewards`.
    ///    * Otherwise:
    ///      * self-stake proportional fraction + commission on delegated fraction
    ///        → `pending_rewards`.
    ///      * delegated fraction minus commission → `delegator_reward_pool`.
    pub fn distribute_block_reward(
        validators: &mut ValidatorSet,
        block_height: u64,
        proposer: &Address,
        accumulated_fees: u128,
    ) {
        // STK-INTERVAL-01: only distribute at every REWARD_INTERVAL boundary.
        // All other blocks are a no-op — the executor accumulates fees.
        if block_height % crate::REWARD_INTERVAL != 0 {
            return;
        }

        // Sum the base subsidy for each block in the interval window.
        // Iterating 100 slots is negligible; handles halving boundaries correctly.
        let window_start = block_height.saturating_sub(crate::REWARD_INTERVAL - 1);
        let interval_base: u128 = (window_start..=block_height)
            .map(block_reward_at)
            .sum();

        let total = interval_base.saturating_add(accumulated_fees);
        if total == 0 { return; }

        let proposer_bonus  = total / 10;
        let committee_share = total - proposer_bonus;

        let total_active_stake: u128 = validators
            .active_set
            .iter()
            .filter_map(|a| validators.validators.get(a))
            .map(|v| v.total_stake())
            .sum();

        if total_active_stake == 0 { return; }

        // Proposer bonus → validator's own reward pool only.
        // STK-RWD-01: no additional commission credit — commission is already
        // baked into the per-block split loop below.
        if let Some(v) = validators.get_mut(proposer) {
            v.pending_rewards = v.pending_rewards.saturating_add(proposer_bonus);
        }

        let active_addrs: Vec<Address> = validators.active_set.clone();
        for addr in &active_addrs {
            if let Some(v) = validators.validators.get_mut(addr) {
                let weight = v.total_stake();
                let total_reward = committee_share * weight / total_active_stake;
                if total_reward == 0 { continue; }

                if v.delegated_stake == 0 {
                    // No delegators — all reward belongs to the validator.
                    v.pending_rewards = v.pending_rewards.saturating_add(total_reward);
                } else {
                    // Proportional split.
                    //
                    //   self_share  = total_reward × self_stake / weight
                    //   deleg_gross = total_reward − self_share
                    //   commission  = deleg_gross × commission_bps / 10_000
                    //   net_deleg   = deleg_gross − commission
                    //
                    //   pending_rewards       += self_share + commission
                    //   delegator_reward_pool += net_deleg
                    //
                    // Integer arithmetic: self_share is computed via checked_mul
                    // to guard against pathologically large reward values.
                    let self_share = total_reward
                        .checked_mul(v.self_stake)
                        .map(|p| p / weight)
                        .unwrap_or_else(|| (total_reward / weight) * v.self_stake);
                    let deleg_gross = total_reward.saturating_sub(self_share);
                    let commission  = v.commission_of(deleg_gross);
                    let net_deleg   = deleg_gross.saturating_sub(commission);

                    v.pending_rewards = v.pending_rewards
                        .saturating_add(self_share)
                        .saturating_add(commission);
                    // STK-DEL-01: snapshot delegated_stake before adding to the
                    // pool so claim_delegator_share can divide by the value that
                    // reflects *who was delegating when rewards were earned*,
                    // not who happens to be delegating at claim time.
                    v.pool_denominator = v.delegated_stake;
                    v.delegator_reward_pool = v.delegator_reward_pool
                        .saturating_add(net_deleg);
                }

                debug!(
                    validator = ?addr,
                    total_reward,
                    block = block_height,
                    interval = crate::REWARD_INTERVAL,
                    "interval reward distributed"
                );
            }
        }
    }

    /// Compute how much ZBX the executor must mint into `STAKING_PRECOMPILE_ADDR`
    /// at a reward-interval boundary so that all future `ClaimRewards` and
    /// `ClaimDelegatorRewards` calls can be fully satisfied from escrow.
    ///
    /// ## Executor contract (RWD-ESCROW-01)
    ///
    /// Every `REWARD_INTERVAL` blocks the executor **must**:
    ///
    /// ```text
    /// 1. Call  interval_escrow_mint(block_height, accumulated_fees)
    ///          → returns mint_amount
    /// 2. escrow_balance += mint_amount          (credit STAKING_PRECOMPILE_ADDR)
    /// 3. Call  distribute_block_reward(...)     (update accounting in ValidatorSet)
    /// ```
    ///
    /// Steps 1+2 must happen **before** any `ClaimRewards` / `ClaimDelegatorRewards`
    /// transaction in the same block is executed, otherwise those txs fail with
    /// `EscrowUnderflow`.
    ///
    /// Returns `0` on non-interval blocks (no minting required).
    pub fn interval_escrow_mint(block_height: u64, accumulated_fees: u128) -> u128 {
        if block_height % crate::REWARD_INTERVAL != 0 {
            return 0;
        }
        let window_start = block_height.saturating_sub(crate::REWARD_INTERVAL - 1);
        let interval_base: u128 = (window_start..=block_height)
            .map(block_reward_at)
            .sum();
        interval_base.saturating_add(accumulated_fees)
    }

    /// Claim a validator's accumulated `pending_rewards` (validator's own share).
    ///
    /// Delegators claiming their share use `claim_delegator_reward` instead.
    pub fn claim_rewards(
        validators: &mut ValidatorSet,
        validator: &Address,
    ) -> Result<u128, crate::StakingError> {
        let v = validators.get_mut(validator)
            .ok_or(crate::StakingError::NotFound(*validator))?;
        let rewards = v.pending_rewards;
        if rewards == 0 {
            return Err(crate::StakingError::NoPendingRewards(*validator));
        }
        v.pending_rewards = 0;
        Ok(rewards)
    }

    /// Claim a delegator's proportional share of a validator's delegator reward pool.
    ///
    /// STK-RWD-06: share = `delegator_reward_pool × delegator_stake / delegated_stake`.
    /// The claimed amount is deducted from the pool immediately to prevent double-claiming.
    ///
    /// `delegator_stake` MUST be the caller's on-chain delegation to `validator`,
    /// read from `ZbxDb` / `StakingDelta` before this call — the caller bears
    /// responsibility for supplying the correct current amount.
    ///
    /// Returns `Err(NoPendingRewards)` if the pool is empty or delegation is zero.
    pub fn claim_delegator_reward(
        validators: &mut ValidatorSet,
        validator: &Address,
        delegator: &Address,
        delegator_stake: u128,
    ) -> Result<u128, crate::StakingError> {
        if delegator_stake == 0 {
            return Err(crate::StakingError::NoPendingRewards(*delegator));
        }
        let v = validators
            .get_mut(validator)
            .ok_or(crate::StakingError::NotFound(*validator))?;
        let share = v.claim_delegator_share(delegator_stake);
        if share == 0 {
            return Err(crate::StakingError::NoPendingRewards(*delegator));
        }
        Ok(share)
    }

    /// Compute APY for a validator given current network conditions.
    ///
    /// Uses f64 — suitable for RPC/display only, not on-chain accounting.
    pub fn estimate_apy(
        validator_stake: u128,
        total_network_stake: u128,
        annual_emission: u128,
    ) -> f64 {
        if validator_stake == 0 || total_network_stake == 0 { return 0.0; }
        let share = validator_stake as f64 / total_network_stake as f64;
        let annual_reward = annual_emission as f64 * share;
        (annual_reward / validator_stake as f64) * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validator::{Validator, ValidatorSet, ValidatorStatus};
    use zbx_crypto::bls::BlsPrivKey;
    use zbx_types::address::Address;

    fn make_vs_with_two_validators(
        self_stake: u128,
        delegated: u128,
        commission_bps: u16,
    ) -> (ValidatorSet, Address) {
        let mut vs = ValidatorSet::new();
        let addr = Address([0xab; 20]);
        let sk = BlsPrivKey::from_bytes(&[11u8; 32]).unwrap();
        vs.validators.insert(addr, Validator {
            address: addr,
            bls_pubkey: sk.to_pubkey(),
            self_stake,
            delegated_stake: delegated,
            commission_bps,
            status: ValidatorStatus::Active,
            last_signed_block: 0,
            pending_rewards: 0,
            delegator_reward_pool: 0,
            pool_denominator: 0,
            registered_epoch: 0,
        });
        vs.active_set = vec![addr];
        (vs, addr)
    }

    #[test]
    fn no_delegation_all_to_pending_rewards() {
        let (mut vs, addr) = make_vs_with_two_validators(1_000_000, 0, 500);
        RewardDistributor::distribute_block_reward(&mut vs, 0, &addr, 0);
        let v = vs.get(&addr).unwrap();
        assert!(v.pending_rewards > 0, "validator should have pending rewards");
        assert_eq!(v.delegator_reward_pool, 0, "no delegator pool when no delegators");
    }

    #[test]
    fn with_delegation_splits_correctly() {
        // 50/50 self/delegated, 10% commission (1000 bps)
        let (mut vs, addr) = make_vs_with_two_validators(500_000, 500_000, 1000);
        let total_reward: u128 = 3_000_000_000_000_000_000; // 3 ZBX block reward
        RewardDistributor::distribute_block_reward(&mut vs, 0, &addr, 0);
        let v = vs.get(&addr).unwrap();

        // pending_rewards = proposer_bonus + self_share + commission
        // delegator_reward_pool = deleg_gross - commission
        assert!(v.pending_rewards > 0);
        assert!(v.delegator_reward_pool > 0);

        // Total must not exceed the original total (minus proposer bonus accounted separately)
        let grand_total = v.pending_rewards + v.delegator_reward_pool;
        // total_reward_from_engine = block_reward_at(0) = 3 ZBX
        // proposer_bonus = total / 10 (added to pending_rewards already)
        // remaining committee_share = 9/10 of total
        // So grand_total can be ≤ total_reward
        assert!(grand_total <= total_reward, "must not exceed total reward");
    }

    #[test]
    fn commission_goes_to_validator_not_delegator() {
        // 0% commission → all delegated share in delegator pool
        let (mut vs0, addr0) = make_vs_with_two_validators(500_000, 500_000, 0);
        // 100% commission (capped at 20%) → max commission in validator pool
        let (mut vs20, addr20) = make_vs_with_two_validators(500_000, 500_000, 2000);

        RewardDistributor::distribute_block_reward(&mut vs0, 0, &addr0, 0);
        RewardDistributor::distribute_block_reward(&mut vs20, 0, &addr20, 0);

        let v0  = vs0.get(&addr0).unwrap();
        let v20 = vs20.get(&addr20).unwrap();

        // Higher commission → higher validator reward, lower delegator pool
        assert!(
            v20.pending_rewards > v0.pending_rewards,
            "20% commission validator must earn more than 0% commission validator"
        );
        assert!(
            v20.delegator_reward_pool < v0.delegator_reward_pool,
            "20% commission validator's delegator pool must be smaller"
        );
    }

    #[test]
    fn claim_delegator_reward_proportional() {
        // Two delegators with 3:1 split
        let addr = Address([0xcd; 20]);
        let sk = BlsPrivKey::from_bytes(&[22u8; 32]).unwrap();
        let mut vs = ValidatorSet::new();
        vs.validators.insert(addr, Validator {
            address: addr,
            bls_pubkey: sk.to_pubkey(),
            self_stake:    1_000_000,
            delegated_stake: 4_000_000,  // 3M + 1M delegators
            commission_bps: 0,
            status: ValidatorStatus::Active,
            last_signed_block: 0,
            pending_rewards: 0,
            delegator_reward_pool: 1_200,  // pre-seeded
            pool_denominator: 4_000_000,   // snapshot = delegated_stake at distribution time
            registered_epoch: 0,
        });
        vs.active_set = vec![addr];

        let d1 = Address([1u8; 20]);
        let d2 = Address([2u8; 20]);

        let r1 = RewardDistributor::claim_delegator_reward(&mut vs, &addr, &d1, 3_000_000).unwrap();
        let r2 = RewardDistributor::claim_delegator_reward(&mut vs, &addr, &d2, 1_000_000).unwrap();

        // r1 should be ~3× r2 (3:1 delegation ratio)
        assert_eq!(r1, 900, "3/4 of pool");
        assert_eq!(r2, 300, "1/4 of pool");
        assert_eq!(vs.get(&addr).unwrap().delegator_reward_pool, 0, "pool drained");
    }

    // ── STK-INTERVAL-01 tests ─────────────────────────────────────────────

    /// Non-interval blocks must be no-ops: no rewards should be credited.
    #[test]
    fn mid_interval_block_is_noop() {
        let (mut vs, addr) = make_vs_with_two_validators(1_000_000, 0, 500);

        // Block 1, 50, 99 are not interval boundaries — nothing should change.
        for h in [1u64, 50, 99] {
            RewardDistributor::distribute_block_reward(&mut vs, h, &addr, 1_000_000);
            let v = vs.get(&addr).unwrap();
            assert_eq!(
                v.pending_rewards, 0,
                "block {h}: no reward should be credited on a non-interval block"
            );
            assert_eq!(v.delegator_reward_pool, 0);
        }
    }

    /// Block 0 fires (0 % 100 == 0) and distributes exactly one block's base
    /// subsidy (genesis edge case: window = [0..=0]).
    /// Block 100 fires and distributes 100 blocks worth of subsidy.
    #[test]
    fn interval_boundary_credits_cumulative_base_reward() {
        use zbx_types::block_reward_at;

        // ── Block 0 (genesis boundary) ───────────────────────────────────
        let (mut vs0, addr0) = make_vs_with_two_validators(1_000_000, 0, 0);
        RewardDistributor::distribute_block_reward(&mut vs0, 0, &addr0, 0);
        let genesis_reward = block_reward_at(0);
        let v0 = vs0.get(&addr0).unwrap();
        // All goes to pending_rewards (no delegators); genesis window = 1 block.
        assert_eq!(v0.pending_rewards, genesis_reward,
            "genesis boundary should credit exactly one block's reward");

        // ── Block 100 (first full interval) ─────────────────────────────
        let (mut vs100, addr100) = make_vs_with_two_validators(1_000_000, 0, 0);
        RewardDistributor::distribute_block_reward(&mut vs100, 100, &addr100, 0);

        // Expected: sum of block_reward_at(h) for h in 1..=100
        let expected_100: u128 = (1u64..=100).map(block_reward_at).sum();
        let v100 = vs100.get(&addr100).unwrap();
        assert_eq!(v100.pending_rewards, expected_100,
            "block 100 should credit 100 blocks of base reward");

        // Block 100 covers 100 blocks; genesis covers 1 → block-100 reward >> genesis.
        assert!(v100.pending_rewards > v0.pending_rewards,
            "full interval must exceed genesis single-block payout");
    }

    /// `interval_escrow_mint` returns 0 on non-interval blocks.
    #[test]
    fn escrow_mint_is_zero_on_mid_interval_blocks() {
        for h in [1u64, 50, 99, 101, 199] {
            assert_eq!(
                RewardDistributor::interval_escrow_mint(h, 999),
                0,
                "block {h} is not an interval boundary — must return 0"
            );
        }
    }

    /// `interval_escrow_mint` returns the exact same total as `distribute_block_reward`
    /// would credit at the boundary — the two must stay in sync.
    #[test]
    fn escrow_mint_equals_distribute_total() {
        use zbx_types::block_reward_at;

        // At block 200: window is blocks 101..=200 (100 blocks, no halving).
        let fees: u128 = 1_000_000_000_000_000_000; // 1 ZBX accumulated fees
        let mint = RewardDistributor::interval_escrow_mint(200, fees);

        let expected_base: u128 = (101u64..=200).map(block_reward_at).sum();
        assert_eq!(mint, expected_base + fees,
            "escrow mint must equal interval base + accumulated fees");

        // Verify distribute_block_reward would credit this exact total to validators.
        let (mut vs, addr) = make_vs_with_two_validators(1_000_000, 0, 0);
        RewardDistributor::distribute_block_reward(&mut vs, 200, &addr, fees);
        let v = vs.get(&addr).unwrap();
        // Single validator with no delegators gets everything.
        // pending_rewards = total (proposer_bonus already included since only 1 validator).
        assert_eq!(v.pending_rewards, mint,
            "single validator must receive the full escrow-mint amount");
    }

    /// Accumulated fees passed at the boundary are included in the distribution.
    #[test]
    fn accumulated_fees_included_at_boundary() {
        let fee_total: u128 = 5_000_000_000_000_000_000; // 5 ZBX accumulated fees

        let (mut vs_no_fees, addr_nf) = make_vs_with_two_validators(1_000_000, 0, 0);
        let (mut vs_with_fees, addr_wf) = make_vs_with_two_validators(1_000_000, 0, 0);

        RewardDistributor::distribute_block_reward(&mut vs_no_fees,  0, &addr_nf, 0);
        RewardDistributor::distribute_block_reward(&mut vs_with_fees, 0, &addr_wf, fee_total);

        let v_nf = vs_no_fees.get(&addr_nf).unwrap();
        let v_wf = vs_with_fees.get(&addr_wf).unwrap();

        assert!(
            v_wf.pending_rewards > v_nf.pending_rewards,
            "accumulated fees must increase validator's reward at the boundary"
        );
        // Difference should equal the fee contribution (minus proposer_bonus rounding).
        let extra = v_wf.pending_rewards - v_nf.pending_rewards;
        assert!(extra > 0, "fee contribution must be positive");
    }

    #[test]
    fn claim_delegator_reward_empty_pool_errors() {
        let addr = Address([0xef; 20]);
        let sk = BlsPrivKey::from_bytes(&[33u8; 32]).unwrap();
        let mut vs = ValidatorSet::new();
        vs.validators.insert(addr, Validator {
            address: addr,
            bls_pubkey: sk.to_pubkey(),
            self_stake: 1_000_000,
            delegated_stake: 1_000_000,
            commission_bps: 0,
            status: ValidatorStatus::Active,
            last_signed_block: 0,
            pending_rewards: 0,
            delegator_reward_pool: 0, // empty
            pool_denominator: 0,
            registered_epoch: 0,
        });
        vs.active_set = vec![addr];

        let delegator = Address([0x01; 20]);
        let err = RewardDistributor::claim_delegator_reward(
            &mut vs, &addr, &delegator, 500_000,
        ).unwrap_err();
        assert!(matches!(err, crate::StakingError::NoPendingRewards(_)));
    }
}
