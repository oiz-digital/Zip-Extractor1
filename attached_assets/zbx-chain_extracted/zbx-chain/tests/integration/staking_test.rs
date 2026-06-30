//! Integration tests for zbx-staking — real behaviour, no stubs.
//!
//! These tests use `zbx_contracts::staking_escrow::StakingEscrow` directly
//! so they run without a live node. Each test exercises a complete user
//! journey through the staking state machine.

#[cfg(test)]
mod staking_integration {
    use zbx_contracts::staking_escrow::{StakingEscrow, StakeStatus};

    const ONE_ZBX: u64 = 1_000_000_000_000_000_000; // 1 ZBX in wei

    /// Build a fresh escrow with a 10-block unbonding period (short for tests).
    fn escrow() -> StakingEscrow {
        StakingEscrow::new(10)
    }

    // ── Test 1: Staking increases validator power ─────────────────────────────

    #[test]
    fn stake_increases_validator_power() {
        let mut escrow = escrow();
        let validator = [0x01u8; 20];
        let delegator  = [0x02u8; 20];

        // Validator self-bonds 100_000 ZBX.
        escrow.bond_validator(validator, 100_000 * ONE_ZBX, 1)
              .expect("validator bond should succeed");

        // Delegator delegates 50_000 ZBX.
        escrow.delegate(delegator, validator, 50_000 * ONE_ZBX, 1)
              .expect("delegation should succeed");

        let val_stake  = escrow.validator_active_stake(&validator);
        let del_record = escrow.delegation_record(&delegator, &validator)
                               .expect("delegation record should exist");

        assert_eq!(val_stake, 100_000 * ONE_ZBX,
            "validator self-stake must be 100_000 ZBX");
        assert_eq!(del_record.active_amount, 50_000 * ONE_ZBX,
            "delegator active amount must be 50_000 ZBX");

        // Combined voting power = 150_000 ZBX.
        let total_power = escrow.total_voting_power(&validator);
        assert_eq!(total_power, 150_000 * ONE_ZBX,
            "total voting power must equal self-stake + delegation");
    }

    // ── Test 2: Epoch transition distributes rewards ──────────────────────────

    #[test]
    fn epoch_transition_distributes_rewards() {
        let mut escrow = escrow();
        let validator = [0x03u8; 20];
        let delegator  = [0x04u8; 20];

        escrow.bond_validator(validator, 80_000 * ONE_ZBX, 1).unwrap();
        escrow.delegate(delegator, validator, 20_000 * ONE_ZBX, 1).unwrap();

        // Total stake = 100_000 ZBX. Epoch reward at 15% APR for 1 epoch
        // (assume 1 year = 1000 epochs → reward = 15% / 1000 × 100_000 = 15 ZBX).
        let epoch_reward = 15 * ONE_ZBX; // supplied by the reward oracle
        escrow.distribute_epoch_reward(validator, epoch_reward, 2)
              .expect("reward distribution should succeed");

        // Validator gets 80% of reward (80_000 / 100_000 share = 12 ZBX).
        let val_reward = escrow.pending_reward(&validator, &validator);
        // Delegator gets 20% (20_000 / 100_000 share = 3 ZBX).
        let del_reward = escrow.pending_reward(&delegator, &validator);

        assert!(val_reward > 0, "validator must have pending reward");
        assert!(del_reward > 0, "delegator must have pending reward");
        assert_eq!(
            val_reward + del_reward,
            epoch_reward,
            "total distributed must equal epoch_reward"
        );
    }

    // ── Test 3: Slashing reduces stake ───────────────────────────────────────

    #[test]
    fn slashing_reduces_stake() {
        let mut escrow = escrow();
        let validator = [0x05u8; 20];

        escrow.bond_validator(validator, 100_000 * ONE_ZBX, 1).unwrap();

        let stake_before = escrow.validator_active_stake(&validator);

        // 5% slash for equivocation.
        let slash_bps = 500u32; // 5% in basis points
        escrow.slash_validator(validator, slash_bps, 2)
              .expect("slashing should succeed");

        let stake_after = escrow.validator_active_stake(&validator);
        let expected_slash = stake_before * 500 / 10_000;

        assert_eq!(
            stake_after,
            stake_before - expected_slash,
            "stake must decrease by exactly 5% after equivocation slash"
        );
        assert!(
            stake_after < stake_before,
            "post-slash stake must be less than pre-slash stake"
        );
    }

    // ── Test 4: Unbonding period enforced ────────────────────────────────────

    #[test]
    fn unbonding_period_enforced() {
        let mut escrow = escrow(); // unbonding_period = 10 blocks
        let validator = [0x06u8; 20];

        escrow.bond_validator(validator, 50_000 * ONE_ZBX, 1).unwrap();

        // Initiate unbonding at block 5.
        escrow.begin_unbonding(validator, 5)
              .expect("begin_unbonding from Active must succeed");

        // Withdrawal attempted at block 10 — period not elapsed yet (need block 15).
        let early = escrow.withdraw_validator_stake(validator, 10);
        assert!(early.is_err(), "withdrawal before unbonding period must fail");

        // Withdrawal at block 15 — period elapsed.
        let late = escrow.withdraw_validator_stake(validator, 15);
        assert!(late.is_ok(), "withdrawal after unbonding period must succeed");

        // Post-withdrawal stake must be zero.
        assert_eq!(
            escrow.validator_active_stake(&validator), 0,
            "stake must be zero after full withdrawal"
        );
    }

    // ── Test 5: Delegation and undelegation roundtrip ─────────────────────────

    #[test]
    fn delegation_and_undelegation_roundtrip() {
        let mut escrow = escrow(); // unbonding_period = 10 blocks
        let validator = [0x07u8; 20];
        let delegator  = [0x08u8; 20];

        escrow.bond_validator(validator, 100_000 * ONE_ZBX, 1).unwrap();
        escrow.delegate(delegator, validator, 30_000 * ONE_ZBX, 1).unwrap();

        // Confirm delegation is active.
        let record = escrow.delegation_record(&delegator, &validator).unwrap();
        assert_eq!(record.active_amount, 30_000 * ONE_ZBX);

        // Undelegate at block 5 (unbonding starts).
        escrow.undelegate(delegator, validator, 30_000 * ONE_ZBX, 5)
              .expect("undelegate must succeed");

        // Active amount is now zero; chunk is pending.
        let record_mid = escrow.delegation_record(&delegator, &validator).unwrap();
        assert_eq!(record_mid.active_amount, 0,
            "active amount must be zero after undelegate");
        assert!(!record_mid.unbonding_chunks.is_empty(),
            "unbonding chunks must be non-empty");

        // Cannot withdraw before block 15.
        let early = escrow.withdraw_delegation(delegator, validator, 14);
        assert!(early.is_err(), "early withdrawal must fail");

        // Withdraw at block 15 — returns principal.
        let amount = escrow.withdraw_delegation(delegator, validator, 15)
                           .expect("withdrawal after period must succeed");
        assert_eq!(amount, 30_000 * ONE_ZBX,
            "withdrawn amount must equal original delegation");
    }
}
