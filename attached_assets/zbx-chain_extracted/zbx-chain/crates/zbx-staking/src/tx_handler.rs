//! On-chain staking-transaction dispatcher.
//!
//! The block executor routes any `SignedTransaction` whose `to` is
//! `STAKING_PRECOMPILE_ADDR` here instead of running EVM bytecode.

use crate::delta::StakingDelta;
use crate::error::StakingError;
use crate::governance::{
    cast_and_maybe_finalize, create_proposal, load_proposal_registry, persist_proposal_registry,
};
use crate::persistence::{BondEntry, BondKind};
use crate::pipeline::SlashingPipeline;
use crate::validator::ValidatorSet;
use zbx_storage::ZbxDb;
use zbx_types::address::Address;
use zbx_types::staking_tx::{
    StakingTx, STAKING_PRECOMPILE_ADDR, UNBONDING_PERIOD_BLOCKS, APPEAL_BOND_WEI,
};
use zbx_types::H256;
use zbx_crypto::bls::{BlsPubKey, BlsSignature};
use tracing::{info, warn};

pub const STAKING_GAS_REGISTER: u64          = 200_000;
pub const STAKING_GAS_DELEGATE: u64          =  60_000;
pub const STAKING_GAS_UNDELEGATE: u64        =  80_000;
pub const STAKING_GAS_WITHDRAW: u64          =  50_000;
pub const STAKING_GAS_CLAIM: u64             =  40_000;
pub const STAKING_GAS_CLAIM_DELEGATOR: u64   =  50_000;
/// `FileAppeal` is more expensive than a Delegate because it touches
/// the slashing registry + records (≥2 disk writes: record + bond).
pub const STAKING_GAS_FILE_APPEAL: u64       = 150_000;
/// `ProposeUpgrade` writes a new `UpgradeProposal` to the governance registry.
pub const STAKING_GAS_PROPOSE_UPGRADE: u64   = 120_000;
/// `CastVote` reads + updates an existing `UpgradeProposal` vote tally.
pub const STAKING_GAS_CAST_VOTE: u64         =  80_000;

/// Trait for reading and mutating account balances. Implemented by
/// `zbx-execution::StateView`; tests use a HashMap shim.
pub trait BalanceAccess {
    fn get_balance(&self, addr: &Address) -> u128;
    fn set_balance(&mut self, addr: &Address, wei: u128);
}

pub fn decode_staking_call(data: &[u8]) -> Result<StakingTx, StakingError> {
    StakingTx::decode(data).map_err(|e| StakingError::BadPayload(e.to_string()))
}

/// Apply a `StakingTx` against the validator set and storage.
///
/// ## ZBX flow through the staking escrow
///
/// All staked and rewarded ZBX is held at `STAKING_PRECOMPILE_ADDR` (the
/// virtual staking-precompile address that acts as the on-chain escrow).
///
/// ```text
/// ┌─────────────────────────────────────────────────────────────────┐
/// │  USER ACTION              ZBX FLOW                             │
/// ├─────────────────────────────────────────────────────────────────┤
/// │  RegisterValidator        sender → STAKING_PRECOMPILE (lock)   │
/// │  Delegate                 sender → STAKING_PRECOMPILE (lock)   │
/// │  Undelegate               (queues unbonding, no balance move)  │
/// │  Withdraw                 STAKING_PRECOMPILE → sender (unlock) │
/// │  ClaimRewards (validator) STAKING_PRECOMPILE → validator       │
/// │  ClaimDelegatorRewards    STAKING_PRECOMPILE → delegator       │
/// ├─────────────────────────────────────────────────────────────────┤
/// │  EXECUTOR (every REWARD_INTERVAL blocks)                       │
/// │    interval_escrow_mint() → mint ZBX into STAKING_PRECOMPILE  │
/// │    distribute_block_reward() → update ValidatorSet accounting  │
/// └─────────────────────────────────────────────────────────────────┘
/// ```
///
/// The executor **must** call `RewardDistributor::interval_escrow_mint` and
/// credit `STAKING_PRECOMPILE_ADDR` BEFORE processing any claim transactions
/// in the same block, otherwise claims will fail with `EscrowUnderflow`.
///
/// ## Value-flow invariants per transaction type
/// - `RegisterValidator`: `sent_value_wei` MUST equal `self_stake`.
/// - `Delegate`: `sent_value_wei` MUST equal `amount`.
/// - `Undelegate` / `Withdraw` / `ClaimRewards` / `ClaimDelegatorRewards`:
///   `sent_value_wei` MUST be 0.
pub fn dispatch_staking_tx<B: BalanceAccess>(
    call: &StakingTx,
    sender: Address,
    sent_value_wei: u128,
    current_height: u64,
    vs: &mut ValidatorSet,
    db: &ZbxDb,
    delta: &mut StakingDelta,
    balances: &mut B,
) -> Result<u64, StakingError> {
    match call {
        StakingTx::RegisterValidator { pubkey: _, bls_pubkey, bls_pop, self_stake, commission_bps } => {
            if sent_value_wei != *self_stake {
                return Err(StakingError::UnexpectedValue {
                    got: sent_value_wei,
                    expected: *self_stake,
                });
            }
            let pk = BlsPubKey::from_bytes(bls_pubkey)
                .map_err(|e| StakingError::InvalidEvidence(format!("bls pubkey: {e}")))?;
            let pop = BlsSignature::from_bytes(bls_pop)
                .map_err(|e| StakingError::InvalidEvidence(format!("bls pop: {e}")))?;
            // Pass-18: production registration MUST go through register_with_pop.
            vs.register_with_pop(sender, pk, pop, *self_stake, *commission_bps)?;
            info!(?sender, self_stake = *self_stake, "validator registered via on-chain tx");
            Ok(STAKING_GAS_REGISTER)
        }

        StakingTx::Delegate { validator, amount } => {
            if sent_value_wei != *amount {
                return Err(StakingError::UnexpectedValue {
                    got: sent_value_wei,
                    expected: *amount,
                });
            }
            if *amount == 0 {
                return Err(StakingError::DelegationTooSmall(1));
            }
            // Pre-check status before any mutation.
            // STK-DEL-01: Unbonding validators must not receive new delegations —
            // their stake is exiting the system and they will leave consensus
            // before the delegator's 21-day unbonding window expires.
            {
                let v = vs.get(validator).ok_or(StakingError::NotFound(*validator))?;
                if matches!(
                    v.status,
                    crate::validator::ValidatorStatus::Jailed
                    | crate::validator::ValidatorStatus::Inactive
                    | crate::validator::ValidatorStatus::Unbonding { .. }
                ) {
                    return Err(StakingError::Jailed);
                }
            }
            let prior = delta.get_delegation(db, validator, &sender)?;
            let new = prior.saturating_add(*amount);
            delta.put_delegation(*validator, sender, new);
            vs.delegate(validator, *amount)?;
            info!(?validator, ?sender, amount = *amount, "delegation recorded");
            Ok(STAKING_GAS_DELEGATE)
        }

        StakingTx::Undelegate { validator, amount } => {
            if sent_value_wei != 0 {
                return Err(StakingError::UnexpectedValue { got: sent_value_wei, expected: 0 });
            }
            // Pre-check both the aggregate and per-delegator ledger.
            {
                let v = vs.get(validator).ok_or(StakingError::NotFound(*validator))?;
                if v.delegated_stake < *amount {
                    return Err(StakingError::InsufficientDelegation {
                        have: v.delegated_stake,
                        requested: *amount,
                    });
                }
            }
            let prior = delta.get_delegation(db, validator, &sender)?;
            if prior < *amount {
                return Err(StakingError::InsufficientDelegation {
                    have: prior,
                    requested: *amount,
                });
            }
            // Same-block repeat undelegations from the same (delegator, validator)
            // pair must accumulate, not overwrite.
            let unlock = current_height.saturating_add(UNBONDING_PERIOD_BLOCKS);
            let existing = delta.get_unbonding_entry(db, unlock, &sender, validator)?;
            let combined = existing.saturating_add(*amount);

            delta.put_delegation(*validator, sender, prior - *amount);
            delta.put_unbonding_entry(unlock, sender, *validator, combined);
            vs.undelegate(validator, *amount)?;
            info!(?validator, ?sender, amount = *amount, unlock, "undelegation queued");
            Ok(STAKING_GAS_UNDELEGATE)
        }

        StakingTx::Withdraw { validator } => {
            if sent_value_wei != 0 {
                return Err(StakingError::UnexpectedValue { got: sent_value_wei, expected: 0 });
            }
            let matured_all = delta.iter_matured_unbondings_for(db, &sender, current_height)?;
            // Filter to only the requested validator.
            let matured: Vec<(u64, Address, u128)> = matured_all
                .into_iter()
                .filter(|(_, v, _)| v == validator)
                .collect();
            if matured.is_empty() {
                return Err(StakingError::NothingToWithdraw);
            }
            let total: u128 = matured.iter().map(|(_, _, a)| *a).sum();
            let escrow = balances.get_balance(&STAKING_PRECOMPILE_ADDR);
            if escrow < total {
                warn!(escrow, total, "STAKING_PRECOMPILE escrow underflow on withdraw");
                return Err(StakingError::EscrowUnderflow { have: escrow, need: total });
            }
            let to_delete: Vec<(u64, Address)> = matured
                .iter()
                .map(|(h, v, _)| (*h, *v))
                .collect();
            delta.delete_unbonding_entries(sender, &to_delete);
            balances.set_balance(&STAKING_PRECOMPILE_ADDR, escrow - total);
            let bal = balances.get_balance(&sender);
            balances.set_balance(&sender, bal.saturating_add(total));
            info!(?sender, ?validator, total, entries = matured.len(),
                  "matured unbondings withdrawn");
            Ok(STAKING_GAS_WITHDRAW)
        }

        StakingTx::ClaimRewards { validator } => {
            if sent_value_wei != 0 {
                return Err(StakingError::UnexpectedValue { got: sent_value_wei, expected: 0 });
            }
            // Validators claim their own rewards: sender must equal validator.
            if &sender != validator {
                return Err(StakingError::NotAValidator(sender));
            }
            if vs.get(validator).is_none() {
                return Err(StakingError::NotAValidator(*validator));
            }
            let amt = vs.claim_rewards(validator)?;
            if amt > 0 {
                // RWD-ESCROW-01: reward ZBX is held in the staking-precompile
                // escrow (minted there by the executor at every REWARD_INTERVAL
                // boundary).  Debit escrow before crediting the validator so
                // total supply is conserved and no ZBX is created twice.
                let escrow = balances.get_balance(&STAKING_PRECOMPILE_ADDR);
                if escrow < amt {
                    warn!(escrow, amt, "STAKING_PRECOMPILE escrow underflow on validator reward claim");
                    return Err(StakingError::EscrowUnderflow { have: escrow, need: amt });
                }
                balances.set_balance(&STAKING_PRECOMPILE_ADDR, escrow - amt);
                let bal = balances.get_balance(validator);
                balances.set_balance(validator, bal.saturating_add(amt));
            }
            info!(?validator, amt, "rewards claimed");
            Ok(STAKING_GAS_CLAIM)
        }

        StakingTx::FileAppeal { .. } => {
            // FileAppeal needs access to the slashing pipeline
            // (registry + EvidenceStore), which is not part of this
            // dispatcher's signature. The block executor MUST route
            // FileAppeal to `dispatch_file_appeal_tx` BEFORE calling
            // `dispatch_staking_tx`. Reaching this arm is a routing
            // bug, never a user-input bug.
            Err(StakingError::BadPayload(
                "FileAppeal must be dispatched via dispatch_file_appeal_tx".into()))
        }

        StakingTx::ClaimDelegatorRewards { validator } => {
            // STK-RWD-06: delegators claim their proportional share of the
            // validator's `delegator_reward_pool`.  The carrying tx value MUST
            // be 0 — no stake is locked; only accrued rewards are transferred.
            if sent_value_wei != 0 {
                return Err(StakingError::UnexpectedValue { got: sent_value_wei, expected: 0 });
            }
            // Sender must have an active delegation to this validator.
            let delegator_stake = delta.get_delegation(db, validator, &sender)?;
            if delegator_stake == 0 {
                return Err(StakingError::NoPendingRewards(sender));
            }
            let amt = crate::rewards::RewardDistributor::claim_delegator_reward(
                vs, validator, &sender, delegator_stake,
            )?;
            if amt > 0 {
                // Rewards flow from the staking-precompile escrow to the delegator.
                let escrow = balances.get_balance(&STAKING_PRECOMPILE_ADDR);
                if escrow < amt {
                    warn!(escrow, amt, "STAKING_PRECOMPILE escrow underflow on delegator reward claim");
                    return Err(StakingError::EscrowUnderflow { have: escrow, need: amt });
                }
                balances.set_balance(&STAKING_PRECOMPILE_ADDR, escrow - amt);
                let bal = balances.get_balance(&sender);
                balances.set_balance(&sender, bal.saturating_add(amt));
            }
            info!(?validator, delegator = ?sender, amt, "delegator rewards claimed");
            Ok(STAKING_GAS_CLAIM_DELEGATOR)
        }

        // ── Governance transactions (C3 — ZEP-add: ProposeUpgrade / CastVote) ──────

        StakingTx::ProposeUpgrade { module_name, new_version, activation_height } => {
            // Only registered, active validators may submit governance proposals.
            if sent_value_wei != 0 {
                return Err(StakingError::UnexpectedValue { got: sent_value_wei, expected: 0 });
            }
            if !vs.is_active_validator(&sender) {
                return Err(StakingError::NotAValidator(sender));
            }
            let mod_name = String::from_utf8(module_name.clone())
                .map_err(|_| StakingError::BadPayload(
                    "ProposeUpgrade: module_name is not valid UTF-8".into(),
                ))?;
            if mod_name.is_empty() || mod_name.len() > 64 {
                return Err(StakingError::BadPayload(format!(
                    "ProposeUpgrade: module_name length {} is out of range [1, 64]",
                    mod_name.len()
                )));
            }
            if *activation_height <= current_height {
                return Err(StakingError::BadPayload(format!(
                    "ProposeUpgrade: activation_height {} must be > current_height {}",
                    activation_height, current_height
                )));
            }
            // Allocate a monotonically-incrementing proposal ID and write the
            // new `UpgradeProposal` into the canonical ProposalRegistry so that
            // the block-producer's `apply_ready_governance` hook can pick it up.
            let proposal_id = delta.next_proposal_id(db)?;
            let mut preg = load_proposal_registry(db)?;
            let id = create_proposal(
                &mut preg,
                proposal_id,
                mod_name.clone(),
                *new_version,
                *activation_height,
                sender,
            )?;
            persist_proposal_registry(db, &preg)?;
            info!(
                proposer = ?sender,
                module   = %mod_name,
                new_version,
                activation_height,
                proposal_id = %id,
                "governance upgrade proposal submitted"
            );
            Ok(STAKING_GAS_PROPOSE_UPGRADE)
        }

        StakingTx::CastVote { proposal_id, approve } => {
            // Only registered, active validators may vote.
            if sent_value_wei != 0 {
                return Err(StakingError::UnexpectedValue { got: sent_value_wei, expected: 0 });
            }
            if !vs.is_active_validator(&sender) {
                return Err(StakingError::NotAValidator(sender));
            }
            // Load the canonical ProposalRegistry, cast the vote, and immediately
            // attempt to finalize (try_finalize) the proposal.  If the vote
            // tips the tally over 50 % of the active-validator set the proposal
            // transitions Pending → Scheduled here; the block-producer's
            // `apply_ready_governance` hook will execute it at `activation_block`.
            let mut preg = load_proposal_registry(db)?;
            let new_status = cast_and_maybe_finalize(
                &mut preg,
                *proposal_id,
                sender,
                *approve,
                vs,
                current_height,
            )?;
            persist_proposal_registry(db, &preg)?;
            info!(
                voter       = ?sender,
                proposal_id,
                approve,
                status      = %new_status,
                "governance vote cast"
            );
            Ok(STAKING_GAS_CAST_VOTE)
        }
    }
}

#[inline]
pub fn is_staking_destination(to: Option<&Address>) -> bool {
    matches!(to, Some(a) if *a == STAKING_PRECOMPILE_ADDR)
}

/// Dispatch an on-chain `FileAppeal` staking transaction.
///
/// The slashed validator (and ONLY they) may appeal a Pending slash
/// record by sending a `FileAppeal` tx to `STAKING_PRECOMPILE_ADDR`
/// with `value == APPEAL_BOND_WEI`. The bond is escrowed at the
/// precompile (transparently, since the tx machinery already moves
/// `sent_value_wei` into the staking-precompile balance before this
/// handler runs); we record the bond in the on-disk ledger so that
/// `SlashingPipeline::overturn_and_refund` can refund it on a
/// successful overturn and so a process crash between filing and
/// finalize does not lose the deposit.
///
/// Validation:
///   * `sent_value_wei == APPEAL_BOND_WEI`
///   * `sender == record.offender`           (only the slashed validator may appeal)
///   * `record.status == Pending`            (enforced inside `file_appeal`)
///   * `current_height <= record.appeal_deadline` (enforced inside `file_appeal`)
pub fn dispatch_file_appeal_tx(
    evidence_id:    H256,
    sender:         Address,
    sent_value_wei: u128,
    current_height: u64,
    pipeline:       &SlashingPipeline,
) -> Result<u64, StakingError> {
    if sent_value_wei != APPEAL_BOND_WEI {
        return Err(StakingError::AppealBondMismatch {
            got:  sent_value_wei,
            need: APPEAL_BOND_WEI,
        });
    }
    // Flip status in the registry (also enforces sender == offender,
    // status == Pending, and current_block <= appeal_deadline).
    let updated_record = {
        let mut reg = pipeline.registry().lock();
        reg.file_appeal_for_tx(evidence_id, sender, current_height)?
    };
    // Persist the appeal bond ledger entry FIRST, then the record.
    // Rationale (architect-review follow-up):
    //   * If `put_bond` succeeds and `put_record` then fails, the
    //     registry's in-memory `Appealed` flip is lost on restart
    //     (registry rehydrates from disk = `Pending`). On retry,
    //     `file_appeal_for_tx` flips again and `put_bond` overwrites
    //     idempotently (same key, same value).
    //   * The reverse order (`put_record` first) was unsafe: a crash
    //     between record-persist and bond-persist would leave an
    //     Appealed record with NO bond on disk — `overturn_and_refund`
    //     would then credit `appeal_bond_refunded = 0`, silently
    //     stealing the offender's appeal bond.
    // Bond is keyed by (record_id, sender) — distinct from any
    // whistleblower bonds on the same record (those are keyed by
    // their respective reporter addresses).
    pipeline.store().put_bond(&evidence_id, &sender, &BondEntry {
        wei:  APPEAL_BOND_WEI,
        kind: BondKind::Appeal,
    })?;
    pipeline.store().put_record(&updated_record)?;
    info!(
        record_id = ?evidence_id,
        offender  = ?sender,
        bond_wei  = APPEAL_BOND_WEI,
        "on-chain appeal filed — bond escrowed at staking precompile"
    );
    Ok(STAKING_GAS_FILE_APPEAL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validator::{Validator, ValidatorStatus};
    use zbx_crypto::bls::BlsPrivKey;
    use std::collections::HashMap;
    use tempfile::TempDir;

    #[derive(Default)]
    struct FakeBalances(HashMap<Address, u128>);
    impl BalanceAccess for FakeBalances {
        fn get_balance(&self, addr: &Address) -> u128 { *self.0.get(addr).unwrap_or(&0) }
        fn set_balance(&mut self, addr: &Address, wei: u128) { self.0.insert(*addr, wei); }
    }

    fn fresh_db() -> (TempDir, std::sync::Arc<ZbxDb>) {
        let tmp = TempDir::new().unwrap();
        let db = std::sync::Arc::new(ZbxDb::open(tmp.path()).unwrap());
        (tmp, db)
    }

    fn fresh_pop_for_seed(addr: Address, seed: u8) -> ([u8; 48], [u8; 96]) {
        let sk = BlsPrivKey::from_bytes(&[seed; 32]).unwrap();
        let pk = sk.to_pubkey();
        let mut preimg = Vec::with_capacity(20 + 14);
        preimg.extend_from_slice(addr.as_bytes());
        preimg.extend_from_slice(b"zbx-bls-pop-v1");
        let msg = zbx_crypto::keccak::keccak256(&preimg);
        let pop = sk.sign(&msg);
        (*pk.as_bytes(), *pop.as_bytes())
    }
    fn fresh_pop(addr: Address) -> ([u8; 48], [u8; 96]) {
        fresh_pop_for_seed(addr, 7)
    }

    #[test]
    fn register_with_pop_succeeds() {
        let (_tmp, db) = fresh_db();
        let mut vs = ValidatorSet::new();
        let mut bal = FakeBalances::default();
        let mut delta = crate::StakingDelta::new();
        let me = Address([0xa1; 20]);
        let (pk_b, pop_b) = fresh_pop(me);
        let stake = crate::MIN_SELF_STAKE;

        let call = StakingTx::RegisterValidator {
            pubkey: [0u8; 33],
            bls_pubkey: pk_b,
            bls_pop: pop_b,
            self_stake: stake,
            commission_bps: 500,
        };
        let gas = dispatch_staking_tx(&call, me, stake, 1, &mut vs, &db, &mut delta, &mut bal).unwrap();
        assert_eq!(gas, STAKING_GAS_REGISTER);
        assert!(vs.get(&me).is_some());
        assert_eq!(vs.get(&me).unwrap().self_stake, stake);
    }

    #[test]
    fn register_with_bad_pop_rejected() {
        let (_tmp, db) = fresh_db();
        let mut vs = ValidatorSet::new();
        let mut bal = FakeBalances::default();
        let mut delta = crate::StakingDelta::new();
        let me = Address([0xa2; 20]);
        let (pk_b, _real_pop) = fresh_pop(me);
        let other = Address([0xa3; 20]);
        let (_, bad_pop) = fresh_pop_for_seed(other, 7);

        let call = StakingTx::RegisterValidator {
            pubkey: [0u8; 33],
            bls_pubkey: pk_b,
            bls_pop: bad_pop,
            self_stake: crate::MIN_SELF_STAKE,
            commission_bps: 500,
        };
        let err = dispatch_staking_tx(&call, me, crate::MIN_SELF_STAKE, 1,
                                       &mut vs, &db, &mut delta, &mut bal).unwrap_err();
        assert!(matches!(err, StakingError::InvalidEvidence(_)));
        assert!(vs.get(&me).is_none());
    }

    #[test]
    fn delegate_then_undelegate_then_withdraw() {
        let (_tmp, db) = fresh_db();
        let mut vs = ValidatorSet::new();
        let mut bal = FakeBalances::default();
        let mut delta = crate::StakingDelta::new();

        let validator = Address([0xb0; 20]);
        let sk = BlsPrivKey::from_bytes(&[33u8; 32]).unwrap();
        vs.validators.insert(validator, Validator {
            address: validator, bls_pubkey: sk.to_pubkey(),
            self_stake: crate::MIN_SELF_STAKE, delegated_stake: 0,
            commission_bps: 500, status: ValidatorStatus::Active,
            last_signed_block: 0, pending_rewards: 0,
            delegator_reward_pool: 0, pool_denominator: 0, registered_epoch: 0,
        });

        let delegator = Address([0xc1; 20]);
        bal.set_balance(&STAKING_PRECOMPILE_ADDR, 0);

        let amount = 10 * 10u128.pow(18);
        let g = dispatch_staking_tx(
            &StakingTx::Delegate { validator, amount },
            delegator, amount, 100, &mut vs, &db, &mut delta, &mut bal,
        ).unwrap();
        assert_eq!(g, STAKING_GAS_DELEGATE);
        bal.set_balance(&STAKING_PRECOMPILE_ADDR, amount);
        assert_eq!(vs.get(&validator).unwrap().delegated_stake, amount);
        assert_eq!(delta.get_delegation(&db, &validator, &delegator).unwrap(), amount);

        let undelegated = 4 * 10u128.pow(18);
        dispatch_staking_tx(
            &StakingTx::Undelegate { validator, amount: undelegated },
            delegator, 0, 200, &mut vs, &db, &mut delta, &mut bal,
        ).unwrap();
        assert_eq!(vs.get(&validator).unwrap().delegated_stake, amount - undelegated);
        assert_eq!(delta.get_delegation(&db, &validator, &delegator).unwrap(), amount - undelegated);

        let err = dispatch_staking_tx(
            &StakingTx::Withdraw { validator },
            delegator, 0, 200 + 100, &mut vs, &db, &mut delta, &mut bal,
        ).unwrap_err();
        assert!(matches!(err, StakingError::NothingToWithdraw));

        let unlock = 200 + UNBONDING_PERIOD_BLOCKS;
        let prior_delegator_bal = bal.get_balance(&delegator);
        let prior_escrow_bal = bal.get_balance(&STAKING_PRECOMPILE_ADDR);
        dispatch_staking_tx(
            &StakingTx::Withdraw { validator },
            delegator, 0, unlock + 1, &mut vs, &db, &mut delta, &mut bal,
        ).unwrap();
        assert_eq!(bal.get_balance(&delegator), prior_delegator_bal + undelegated);
        assert_eq!(bal.get_balance(&STAKING_PRECOMPILE_ADDR),
                   prior_escrow_bal - undelegated);
        let err2 = dispatch_staking_tx(
            &StakingTx::Withdraw { validator },
            delegator, 0, unlock + 1, &mut vs, &db, &mut delta, &mut bal,
        ).unwrap_err();
        assert!(matches!(err2, StakingError::NothingToWithdraw));
    }

    #[test]
    fn claim_rewards_credits_validator() {
        let (_tmp, db) = fresh_db();
        let mut vs = ValidatorSet::new();
        let mut bal = FakeBalances::default();
        let mut delta = crate::StakingDelta::new();
        let validator = Address([0xd1; 20]);
        let reward = 7 * 10u128.pow(18);
        let sk = BlsPrivKey::from_bytes(&[55u8; 32]).unwrap();
        vs.validators.insert(validator, Validator {
            address: validator, bls_pubkey: sk.to_pubkey(),
            self_stake: crate::MIN_SELF_STAKE, delegated_stake: 0,
            commission_bps: 500, status: ValidatorStatus::Active,
            last_signed_block: 0, pending_rewards: reward,
            delegator_reward_pool: 0, pool_denominator: 0, registered_epoch: 0,
        });
        // RWD-ESCROW-01: executor mints reward ZBX into escrow at each
        // REWARD_INTERVAL boundary; claim must debit escrow, not create ZBX.
        bal.set_balance(&STAKING_PRECOMPILE_ADDR, reward);

        dispatch_staking_tx(
            &StakingTx::ClaimRewards { validator },
            validator, 0, 1, &mut vs, &db, &mut delta, &mut bal,
        ).unwrap();
        assert_eq!(bal.get_balance(&validator), reward,
            "validator balance must increase by reward amount");
        assert_eq!(bal.get_balance(&STAKING_PRECOMPILE_ADDR), 0,
            "escrow must be debited by reward amount");
        assert_eq!(vs.get(&validator).unwrap().pending_rewards, 0,
            "pending_rewards must be zeroed after claim");
    }

    /// Claim fails when escrow does not hold enough ZBX (executor minting lag).
    #[test]
    fn claim_rewards_fails_on_escrow_underflow() {
        let (_tmp, db) = fresh_db();
        let mut vs = ValidatorSet::new();
        let mut bal = FakeBalances::default();
        let mut delta = crate::StakingDelta::new();
        let validator = Address([0xd2; 20]);
        let reward = 5 * 10u128.pow(18);
        let sk = BlsPrivKey::from_bytes(&[56u8; 32]).unwrap();
        vs.validators.insert(validator, Validator {
            address: validator, bls_pubkey: sk.to_pubkey(),
            self_stake: crate::MIN_SELF_STAKE, delegated_stake: 0,
            commission_bps: 500, status: ValidatorStatus::Active,
            last_signed_block: 0, pending_rewards: reward,
            delegator_reward_pool: 0, pool_denominator: 0, registered_epoch: 0,
        });
        // Escrow holds less than owed — executor minting bug / invariant violation.
        bal.set_balance(&STAKING_PRECOMPILE_ADDR, reward / 2);

        let err = dispatch_staking_tx(
            &StakingTx::ClaimRewards { validator },
            validator, 0, 1, &mut vs, &db, &mut delta, &mut bal,
        ).unwrap_err();
        assert!(matches!(err, StakingError::EscrowUnderflow { .. }),
            "must surface escrow underflow, not silently mint ZBX");
    }

    #[test]
    fn claim_rewards_by_non_validator_rejected() {
        let (_tmp, db) = fresh_db();
        let mut vs = ValidatorSet::new();
        let mut bal = FakeBalances::default();
        let mut delta = crate::StakingDelta::new();
        let stranger = Address([0xee; 20]);
        let err = dispatch_staking_tx(
            &StakingTx::ClaimRewards { validator: stranger },
            stranger, 0, 1, &mut vs, &db, &mut delta, &mut bal,
        ).unwrap_err();
        assert!(matches!(err, StakingError::NotAValidator(_)));
    }

    #[test]
    fn nonzero_value_on_withdraw_rejected() {
        let (_tmp, db) = fresh_db();
        let mut vs = ValidatorSet::new();
        let mut bal = FakeBalances::default();
        let mut delta = crate::StakingDelta::new();
        let me = Address([0xff; 20]);
        let validator = Address([0xb0; 20]);
        let err = dispatch_staking_tx(
            &StakingTx::Withdraw { validator },
            me, 1, 1, &mut vs, &db, &mut delta, &mut bal,
        ).unwrap_err();
        assert!(matches!(err, StakingError::UnexpectedValue { got: 1, expected: 0 }));
    }
}
