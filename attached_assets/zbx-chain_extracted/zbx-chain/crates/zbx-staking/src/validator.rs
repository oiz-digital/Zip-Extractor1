//! Validator registry and active set management.

use crate::{error::StakingError, EPOCH_LENGTH, MAX_VALIDATORS, MIN_SELF_STAKE};
use zbx_types::address::Address;
use zbx_crypto::bls::{BlsPubKey, BlsSignature};
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Lifecycle state of a validator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValidatorStatus {
    /// Registered but not yet in the active set.
    Pending,
    /// Active participant in consensus.
    Active,
    /// Temporarily excluded from consensus (slashing or liveness fault).
    /// Can re-enter the active set after operator-driven unjail.
    Jailed,
    /// Voluntarily unbonding from the network.
    Unbonding { until_block: u64 },
    /// Fully withdrawn.
    Inactive,
    /// **Permanent** exclusion from consensus, set by the slashing pipeline
    /// on repeat or catastrophic offence (≥2 confirmed slashes lifetime,
    /// or any `InvalidBlock` evidence). Tombstoned validators:
    ///   * are never eligible for election (`is_eligible() == false`)
    ///   * cannot receive new delegations
    ///   * cannot be revived by operator status edits (every read path
    ///     checks Tombstoned explicitly)
    /// Existing delegators can still undelegate / withdraw matured
    /// unbondings normally — only the validator's consensus role ends.
    Tombstoned,
}

/// A single validator's on-chain state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Validator {
    /// EVM address — also the reward recipient.
    pub address: Address,
    /// BLS public key for consensus vote aggregation.
    pub bls_pubkey: BlsPubKey,
    /// Amount staked by the validator itself (wei).
    pub self_stake: u128,
    /// Total delegation from external delegators (wei).
    pub delegated_stake: u128,
    /// Commission rate (basis points, 0–2000 = 0–20%).
    pub commission_bps: u16,
    pub status: ValidatorStatus,
    /// Block number when this validator last signed a block.
    pub last_signed_block: u64,
    /// Accumulated unclaimed rewards for the validator itself (wei).
    /// Covers: self-stake proportional share + commission on delegated share + proposer bonus.
    pub pending_rewards: u128,
    /// Accumulated rewards designated for delegators — net of commission (wei).
    ///
    /// STK-RWD-06: at reward-distribution time each block's delegated-stake share is
    /// split: commission goes to `pending_rewards`, the remainder to this pool.
    /// Delegators claim their proportional slice via `ClaimDelegatorRewards`.
    /// Using `#[serde(default)]` means existing serialised `Validator` records
    /// that lack this field deserialize with `0`, preserving backward compatibility.
    #[serde(default)]
    pub delegator_reward_pool: u128,
    /// Snapshot of `delegated_stake` captured at the moment the current
    /// `delegator_reward_pool` was funded (i.e. at each reward-distribution
    /// boundary).
    ///
    /// ## STK-DEL-01 fix (2026-05-16)
    ///
    /// Previously `claim_delegator_share` divided by `self.delegated_stake`
    /// (the live value at claim time).  An attacker could:
    ///   1. Wait until the reward interval fires and fills `delegator_reward_pool`.
    ///   2. In the same block, delegate a large amount — inflating `delegated_stake`.
    ///   3. Immediately claim, receiving a disproportionately large share because
    ///      the denominator was now larger while the numerator (delegator_stake)
    ///      was also large, stealing from existing delegators.
    ///
    /// Fix: `distribute_block_reward` snapshots `delegated_stake → pool_denominator`
    /// at distribution time.  `claim_delegator_share` divides by `pool_denominator`
    /// so the proportional split reflects who was delegating *when the rewards were
    /// earned*, not who happens to be delegating at claim time.
    ///
    /// `#[serde(default)]` ensures backward-compatible deserialization from
    /// records written before this field existed (they deserialize as 0).
    #[serde(default)]
    pub pool_denominator: u128,
    /// Epoch when registered.
    pub registered_epoch: u64,
}

impl Validator {
    pub fn total_stake(&self) -> u128 {
        self.self_stake.saturating_add(self.delegated_stake)
    }

    pub fn is_active(&self) -> bool {
        self.status == ValidatorStatus::Active
    }

    pub fn is_eligible(&self) -> bool {
        // STK-VAL-04: Unbonding validators must NOT re-enter the active set.
        // A validator that has initiated withdrawal should not be eligible for
        // election — their stake is exiting the system and including them in
        // the active set would allow them to earn rewards while unbonding and
        // could leave the active set short-staffed when their stake finalizes.
        self.self_stake >= MIN_SELF_STAKE
            && !matches!(
                self.status,
                ValidatorStatus::Jailed
                    | ValidatorStatus::Inactive
                    | ValidatorStatus::Unbonding { .. }
                    | ValidatorStatus::Tombstoned
            )
    }

    /// Compute validator's commission on `reward` using integer BPS arithmetic.
    ///
    /// STK-COMM-01: replaces the imprecise f64 `commission_factor()`.
    /// `commission_bps ≤ 2000 < 2^11`; for any reward ≤ MAX_REWARD_PER_BLOCK
    /// (2^120) the product `reward × bps ≤ 2^131`, which overflows u128.
    /// We therefore use `checked_mul` with a safe fallback that avoids
    /// truncation for large values.
    pub fn commission_of(&self, reward: u128) -> u128 {
        reward
            .checked_mul(self.commission_bps as u128)
            .map(|p| p / 10_000)
            .unwrap_or_else(|| (reward / 10_000) * self.commission_bps as u128)
    }

    /// Claim a delegator's proportional share of `delegator_reward_pool`.
    ///
    /// STK-RWD-06: share = `pool × delegator_stake / pool_denominator`.
    ///
    /// ## STK-DEL-01 fix (2026-05-16)
    ///
    /// The denominator is now `pool_denominator` — the snapshot of
    /// `delegated_stake` taken at distribution time — rather than the current
    /// live `delegated_stake`.  This prevents an attacker from delegating after
    /// a reward distribution (inflating the denominator) and then claiming a
    /// proportionally large share of the pre-existing pool.
    ///
    /// If `pool_denominator == 0` (pool was funded before this field existed or
    /// all delegators have fully exited) the full residual pool is returned to
    /// the caller so dust never accumulates forever in a dead pool.
    /// Returns 0 when the pool is empty.
    pub fn claim_delegator_share(&mut self, delegator_stake: u128) -> u128 {
        if self.delegator_reward_pool == 0 {
            return 0;
        }
        // STK-DEL-01: use pool_denominator (distribution-time snapshot) not
        // the current delegated_stake to prevent post-distribution delegation
        // from diluting existing delegators' claims.
        //
        // ## Denominator decay (correct proportional split for sequential claims)
        //
        // After each claim we decrement pool_denominator by delegator_stake so
        // that the residual pool is split fairly among remaining claimants.
        //
        // Example: pool = 1200, denom = 4M, d1 = 3M, d2 = 1M.
        //   d1 claims: share = 1200 × 3M / 4M = 900; pool → 300, denom → 1M.
        //   d2 claims: share = 300  × 1M / 1M = 300; pool → 0.   ✓
        //
        // Without denominator decay, d2 would get 300 × 1M / 4M = 75 — an
        // undercount caused by d1's claim having already removed 900 from the
        // pool while the denominator stayed at 4M.
        let share = if self.pool_denominator > 0 {
            // Integer proportional split.
            self.delegator_reward_pool
                .checked_mul(delegator_stake)
                .map(|p| p / self.pool_denominator)
                .unwrap_or_else(|| {
                    // pool × stake overflows u128: compute via reciprocal.
                    (self.delegator_reward_pool / self.pool_denominator)
                        .saturating_mul(delegator_stake)
                })
        } else {
            // pool_denominator == 0: pool was created before this field existed
            // or all delegators have exited — return residual dust to the
            // claimant so it does not accumulate forever in a dead pool.
            self.delegator_reward_pool
        };
        let share = share.min(self.delegator_reward_pool);
        self.delegator_reward_pool = self.delegator_reward_pool.saturating_sub(share);
        // Decay the denominator so the residual pool is proportioned correctly
        // for remaining claimants.  Saturating to 0 handles the last claimant
        // and the legacy case where delegator_stake > pool_denominator.
        self.pool_denominator = self.pool_denominator.saturating_sub(delegator_stake);
        share
    }

    /// f64 commission factor kept for display/RPC code only.
    /// All on-chain accounting MUST use `commission_of()`.
    pub fn commission_factor(&self) -> f64 {
        self.commission_bps as f64 / 10_000.0
    }
}

/// Manages the full validator registry and active set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorSet {
    pub validators: HashMap<Address, Validator>,
    pub active_set: Vec<Address>,
    pub current_epoch: u64,
}

impl ValidatorSet {
    pub fn new() -> Self {
        ValidatorSet {
            validators: HashMap::new(),
            active_set: Vec::new(),
            current_epoch: 0,
        }
    }

    /// Register a new validator.
    ///
    /// # Pass-18 deprecation note
    ///
    /// SEC-2026-05-09 Pass-18: this entry point does **not** verify a BLS
    /// Proof-of-Possession and is therefore vulnerable to the rogue-key
    /// attack on aggregate BLS signatures (a malicious validator can publish
    /// `pk_attacker = pk_real - sum(pk_others)` and forge an aggregate-sig
    /// the verifier accepts as committee-signed). It is kept for backward
    /// compatibility with genesis loaders that hard-code key material from
    /// a trusted setup ceremony, but **all production / network registration
    /// flows must use [`register_with_pop`]**, which requires the validator
    /// to BLS-sign their own ECDSA address with the canonical `zbx-bls-pop-v1`
    /// domain separator before being added to the registry.
    pub fn register(
        &mut self,
        address: Address,
        bls_pubkey: BlsPubKey,
        self_stake: u128,
        commission_bps: u16,
    ) -> Result<(), StakingError> {
        if self.validators.contains_key(&address) {
            return Err(StakingError::AlreadyRegistered(address));
        }
        if self_stake < MIN_SELF_STAKE {
            return Err(StakingError::InsufficientSelfStake {
                have: self_stake,
                need: MIN_SELF_STAKE,
            });
        }
        let v = Validator {
            address,
            bls_pubkey,
            self_stake,
            delegated_stake: 0,
            commission_bps: commission_bps.min(2000),
            status: ValidatorStatus::Pending,
            last_signed_block: 0,
            pending_rewards: 0,
            delegator_reward_pool: 0,
            pool_denominator: 0,
            registered_epoch: self.current_epoch,
        };
        self.validators.insert(address, v);
        info!(validator = ?address, stake = self_stake, "validator registered");
        Ok(())
    }

    /// SEC-2026-05-09 Pass-18 — register with mandatory BLS Proof-of-Possession.
    ///
    /// `pop` MUST be `BlsSign(bls_sk, keccak256(address ‖ "zbx-bls-pop-v1"))`,
    /// which proves the validator actually possesses the secret key matching
    /// `bls_pubkey`. This is the canonical defense against rogue-key attacks
    /// on BLS aggregate signatures; without it, a malicious validator can
    /// derive a forged `bls_pubkey` whose secret key is `unknown` to anyone
    /// but which combines with the rest of the committee's keys to verify an
    /// arbitrary aggregate signature as "signed by everyone".
    ///
    /// All network registration RPCs and on-chain `Stake` flows MUST call
    /// this function; the legacy [`register`] entry point is kept only for
    /// genesis loaders sourcing keys from a trusted setup ceremony where
    /// possession is established out-of-band.
    pub fn register_with_pop(
        &mut self,
        address: Address,
        bls_pubkey: BlsPubKey,
        bls_pop: BlsSignature,
        self_stake: u128,
        commission_bps: u16,
    ) -> Result<(), StakingError> {
        if !bls_pubkey.verify_pop(&bls_pop, &address) {
            warn!(
                validator = ?address,
                "BLS PoP verification failed — rejecting registration \
                 (possible rogue-key attack)"
            );
            return Err(StakingError::InvalidEvidence(
                "BLS Proof-of-Possession does not verify under \
                 keccak256(address || \"zbx-bls-pop-v1\")".into(),
            ));
        }
        self.register(address, bls_pubkey, self_stake, commission_bps)
    }

    /// Delegate stake to a validator.
    ///
    /// STK-DEL-01: Unbonding validators are blocked in addition to Jailed/Inactive.
    /// A validator initiating withdrawal is exiting consensus; accepting new
    /// delegations would lock delegators' funds to a node that will stop
    /// participating before the 21-day unbonding window expires.
    pub fn delegate(&mut self, to: &Address, amount: u128) -> Result<(), StakingError> {
        let v = self.validators.get_mut(to).ok_or(StakingError::NotFound(*to))?;
        if matches!(
            v.status,
            ValidatorStatus::Jailed
                | ValidatorStatus::Inactive
                | ValidatorStatus::Unbonding { .. }
                | ValidatorStatus::Tombstoned
        ) {
            return Err(StakingError::Jailed);
        }
        v.delegated_stake = v.delegated_stake.saturating_add(amount);
        Ok(())
    }

    /// Undelegate `amount` from a validator's aggregate `delegated_stake`.
    /// Per-delegator ledger checks happen in `dispatch_staking_tx` against
    /// ZbxDb. Jailed/Inactive validators can still be undelegated from so
    /// delegators can recover stake from a misbehaving node.
    pub fn undelegate(&mut self, from: &Address, amount: u128) -> Result<(), StakingError> {
        let v = self.validators.get_mut(from).ok_or(StakingError::NotFound(*from))?;
        if v.delegated_stake < amount {
            return Err(StakingError::InsufficientDelegation {
                have: v.delegated_stake,
                requested: amount,
            });
        }
        v.delegated_stake -= amount;
        Ok(())
    }

    /// Claim a validator's accumulated `pending_rewards`. Returns the
    /// claimed amount and resets the counter; caller credits the balance.
    pub fn claim_rewards(&mut self, validator: &Address) -> Result<u128, StakingError> {
        let v = self.validators.get_mut(validator).ok_or(StakingError::NotAValidator(*validator))?;
        let amt = v.pending_rewards;
        v.pending_rewards = 0;
        Ok(amt)
    }

    /// Elect the top-MAX_VALIDATORS eligible validators for the next epoch.
    pub fn elect_active_set(&mut self) -> Vec<Address> {
        let mut eligible: Vec<_> = self.validators.values()
            .filter(|v| v.is_eligible())
            .map(|v| (v.address, v.total_stake()))
            .collect();
        // STK-ELT-01: secondary sort by address bytes (ascending) breaks ties in
        // total_stake deterministically across all nodes. Without a tiebreaker,
        // HashMap iteration order varies by OS/runtime so two nodes could elect
        // different active sets, producing a consensus fork.
        eligible.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0 .0.cmp(&b.0 .0)));
        eligible.truncate(MAX_VALIDATORS);

        let new_set: Vec<Address> = eligible.into_iter().map(|(a, _)| a).collect();
        // STK-VAL-01: Deactivate removed validators — but ONLY if they are
        // currently Active. Overriding Unbonding or Inactive with Pending would
        // reset in-progress withdrawals and break the unbonding state machine.
        // Tombstoned is also preserved — it is permanent and election must
        // never demote it back to Pending (which would allow re-eligibility
        // if stake later returned above MIN_SELF_STAKE).
        for addr in &self.active_set {
            if !new_set.contains(addr) {
                if let Some(v) = self.validators.get_mut(addr) {
                    if v.status == ValidatorStatus::Active {
                        v.status = ValidatorStatus::Pending;
                    }
                }
            }
        }
        // Activate newly elected validators — except Tombstoned validators,
        // which must never be reactivated. `is_eligible` already excludes
        // them from `eligible` above, so this is a defence-in-depth guard.
        for addr in &new_set {
            if let Some(v) = self.validators.get_mut(addr) {
                if v.status != ValidatorStatus::Tombstoned {
                    v.status = ValidatorStatus::Active;
                }
            }
        }
        self.active_set = new_set.clone();
        self.current_epoch += 1;
        info!(epoch = self.current_epoch, active = self.active_set.len(), "new epoch elected");
        new_set
    }

    pub fn get(&self, addr: &Address) -> Option<&Validator> {
        self.validators.get(addr)
    }

    pub fn get_mut(&mut self, addr: &Address) -> Option<&mut Validator> {
        self.validators.get_mut(addr)
    }

    pub fn active_bls_keys(&self) -> Vec<BlsPubKey> {
        self.active_set.iter()
            .filter_map(|a| self.validators.get(a).map(|v| v.bls_pubkey.clone()))
            .collect()
    }

    /// Returns `true` if `addr` is a registered, non-jailed, active-set validator.
    /// Used by the governance dispatcher to gate `ProposeUpgrade` and `CastVote`.
    pub fn is_active_validator(&self, addr: &Address) -> bool {
        self.active_set.contains(addr)
            && self.validators.get(addr).map_or(false, |v| {
                !matches!(
                    v.status,
                    ValidatorStatus::Jailed | ValidatorStatus::Tombstoned | ValidatorStatus::Inactive
                )
            })
    }

    /// Returns the total stake (own + delegated) of a registered validator,
    /// or `None` if the address is not registered.
    /// Used by the governance dispatcher to weight `CastVote`.
    pub fn total_stake_of(&self, addr: &Address) -> Option<u128> {
        self.validators.get(addr).map(|v| v.total_stake())
    }

}

impl Default for ValidatorSet { fn default() -> Self { Self::new() } }