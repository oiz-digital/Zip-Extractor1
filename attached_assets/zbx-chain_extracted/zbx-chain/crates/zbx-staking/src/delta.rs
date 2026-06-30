//! Staking write-set deferred until block commit.
//!
//! `dispatch_staking_tx` accumulates persistence operations into a
//! `StakingDelta` instead of writing directly to RocksDB. The executor
//! returns the delta as part of `ExecutionResult`. The block producer
//! flushes the delta via `ZbxDb::apply_staking_delta` only after the
//! reorg pre-commit check passes and the block has been persisted —
//! so a dropped candidate block can never leave staking-side state
//! drift on disk.
//!
//! Reads (`get_delegation`, `get_unbonding_entry`,
//! `iter_matured_unbondings_for`) are overlaid: pending ops within
//! the same block take precedence over the on-disk view, so a
//! `Delegate` followed by `Undelegate` in the same block sees the
//! right intermediate balance.

use crate::error::StakingError;
use std::collections::{HashMap, HashSet};
use zbx_storage::ZbxDb;
use zbx_types::address::Address;

#[derive(Debug, Default, Clone)]
pub struct StakingDelta {
    delegations: HashMap<(Address, Address), u128>,
    unbonding_puts: HashMap<(u64, Address, Address), u128>,
    unbonding_deletes: HashSet<(u64, Address, Address)>,
}

impl StakingDelta {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.delegations.is_empty()
            && self.unbonding_puts.is_empty()
            && self.unbonding_deletes.is_empty()
    }

    pub fn delegation_overrides(&self) -> &HashMap<(Address, Address), u128> {
        &self.delegations
    }
    pub fn unbonding_put_overrides(&self) -> &HashMap<(u64, Address, Address), u128> {
        &self.unbonding_puts
    }
    pub fn unbonding_delete_overrides(&self) -> &HashSet<(u64, Address, Address)> {
        &self.unbonding_deletes
    }

    pub fn get_delegation(
        &self,
        db: &ZbxDb,
        validator: &Address,
        delegator: &Address,
    ) -> Result<u128, StakingError> {
        if let Some(v) = self.delegations.get(&(*validator, *delegator)) {
            return Ok(*v);
        }
        db.get_delegation(validator, delegator)
            .map_err(|e| StakingError::Persistence(e.to_string()))
    }

    pub fn put_delegation(&mut self, validator: Address, delegator: Address, amount: u128) {
        self.delegations.insert((validator, delegator), amount);
    }

    pub fn get_unbonding_entry(
        &self,
        db: &ZbxDb,
        unlock: u64,
        delegator: &Address,
        validator: &Address,
    ) -> Result<u128, StakingError> {
        let key = (unlock, *delegator, *validator);
        if self.unbonding_deletes.contains(&key) {
            return Ok(self.unbonding_puts.get(&key).copied().unwrap_or(0));
        }
        if let Some(v) = self.unbonding_puts.get(&key) {
            return Ok(*v);
        }
        db.get_unbonding_entry(unlock, delegator, validator)
            .map_err(|e| StakingError::Persistence(e.to_string()))
    }

    pub fn put_unbonding_entry(
        &mut self,
        unlock: u64,
        delegator: Address,
        validator: Address,
        amount: u128,
    ) {
        let key = (unlock, delegator, validator);
        self.unbonding_deletes.remove(&key);
        self.unbonding_puts.insert(key, amount);
    }

    pub fn iter_matured_unbondings_for(
        &self,
        db: &ZbxDb,
        who: &Address,
        current_height: u64,
    ) -> Result<Vec<(u64, Address, u128)>, StakingError> {
        let mut on_disk = db
            .iter_matured_unbondings_for(who, current_height)
            .map_err(|e| StakingError::Persistence(e.to_string()))?;
        // Apply deletes + put-overrides on the on-disk view.
        on_disk.retain(|(h, v, _)| !self.unbonding_deletes.contains(&(*h, *who, *v)));
        for (h, v, amt) in on_disk.iter_mut() {
            if let Some(over) = self.unbonding_puts.get(&(*h, *who, *v)) {
                *amt = *over;
            }
        }
        // Add brand-new puts whose unlock <= current_height and that
        // were not already in the on-disk vec.
        let mut seen: HashSet<(u64, Address)> = on_disk
            .iter()
            .map(|(h, v, _)| (*h, *v))
            .collect();
        for ((unlock, delegator, validator), amt) in &self.unbonding_puts {
            if delegator != who {
                continue;
            }
            if *unlock > current_height {
                continue;
            }
            if seen.insert((*unlock, *validator)) {
                on_disk.push((*unlock, *validator, *amt));
            }
        }
        Ok(on_disk)
    }

    pub fn delete_unbonding_entry(
        &mut self,
        unlock: u64,
        delegator: Address,
        validator: Address,
    ) {
        let key = (unlock, delegator, validator);
        self.unbonding_puts.remove(&key);
        self.unbonding_deletes.insert(key);
    }

    pub fn delete_unbonding_entries(&mut self, delegator: Address, entries: &[(u64, Address)]) {
        for (h, v) in entries {
            self.delete_unbonding_entry(*h, delegator, *v);
        }
    }

    // ── Governance proposal helpers ──────────────────────────────────────────────
    //
    // `next_proposal_id` — retained as the sole canonical ID allocator.
    //
    // `record_proposal` and `record_vote` were interim raw-metadata helpers
    // written before `governance::ProposalRegistry` was wired into the dispatcher.
    // They are now SUPERSEDED: `dispatch_staking_tx` uses
    // `governance::{create_proposal, cast_and_maybe_finalize}` which write the
    // canonical `ProposalRegistry` blob under
    // `GOVERNANCE_META_PROPOSAL_REGISTRY` (= "governance/proposal_registry").
    // The raw per-proposal and per-vote keys written by the old helpers are
    // unused by any reader; they are kept below only so that existing test
    // fixtures that call them directly continue to compile.  New code must NOT
    // call `record_proposal` or `record_vote` — use `governance.rs` instead.

    /// Read + increment the governance proposal counter stored at the well-known
    /// metadata key `b"gov/proposal_counter"`.  Returns the NEW proposal ID (1-based).
    ///
    /// The counter is persisted to `db` immediately so that a crash between
    /// `next_proposal_id` and `create_proposal` does not re-use an ID.
    pub fn next_proposal_id(&mut self, db: &ZbxDb) -> Result<u64, StakingError> {
        const KEY: &[u8] = b"gov/proposal_counter";
        let current: u64 = db
            .get_metadata(KEY)
            .map_err(|e| StakingError::Persistence(e.to_string()))?
            .map(|v| {
                let arr: [u8; 8] = v.get(..8)
                    .and_then(|s| s.try_into().ok())
                    .unwrap_or([0u8; 8]);
                u64::from_be_bytes(arr)
            })
            .unwrap_or(0);
        let next = current + 1;
        db.put_metadata(KEY, next.to_be_bytes().to_vec())
            .map_err(|e| StakingError::Persistence(e.to_string()))?;
        Ok(next)
    }

    /// Persist an `UpgradeProposal` record to the metadata store.
    ///
    /// The key is `b"gov/proposal/<id>"` (big-endian u64 suffix).
    /// The value is RLP: `[module_name, new_version, activation_height, proposer_addr]`.
    pub fn record_proposal(
        &mut self,
        db: &ZbxDb,
        proposal_id: u64,
        module_name: String,
        new_version: u32,
        activation_height: u64,
        proposer: Address,
    ) -> Result<(), StakingError> {
        use rlp::RlpStream;
        let mut s = RlpStream::new_list(4);
        s.append(&module_name.as_bytes());
        s.append(&new_version);
        s.append(&activation_height);
        s.append(&&proposer.0[..]);
        db.put_metadata(&proposal_key(proposal_id), s.out().to_vec())
            .map_err(|e| StakingError::Persistence(e.to_string()))
    }

    /// Record a governance vote by `voter` on `proposal_id`.
    ///
    /// The vote record is stored at `b"gov/vote/<proposal_id>/<voter_20bytes>"`.
    /// Double-voting by the same address is rejected (second `CastVote` tx errors out).
    pub fn record_vote(
        &mut self,
        db: &ZbxDb,
        proposal_id: u64,
        voter: Address,
        approve: bool,
        weight: u128,
    ) -> Result<(), StakingError> {
        use rlp::RlpStream;
        let vkey = vote_key(proposal_id, voter);
        // Prevent double-voting.
        let already = db
            .get_metadata(&vkey)
            .map_err(|e| StakingError::Persistence(e.to_string()))?
            .map_or(false, |v| !v.is_empty());
        if already {
            return Err(StakingError::BadPayload(format!(
                "CastVote: address {:?} has already voted on proposal {}",
                voter, proposal_id
            )));
        }
        let mut s = RlpStream::new_list(2);
        s.append(&(approve as u8));
        s.append(&weight);
        db.put_metadata(&vkey, s.out().to_vec())
            .map_err(|e| StakingError::Persistence(e.to_string()))
    }
}

#[inline]
fn proposal_key(id: u64) -> Vec<u8> {
    let mut k = b"gov/proposal/".to_vec();
    k.extend_from_slice(&id.to_be_bytes());
    k
}

#[inline]
fn vote_key(proposal_id: u64, voter: Address) -> Vec<u8> {
    let mut k = b"gov/vote/".to_vec();
    k.extend_from_slice(&proposal_id.to_be_bytes());
    k.push(b'/');
    k.extend_from_slice(&voter.0);
    k
}
