//! SEC-2026-05-09 Pass-11 — end-to-end slashing execution pipeline.
//!
//! This module closes the second HARD mainnet blocker from
//! `docs/SUBSYSTEM-MATURITY-AUDIT-2026-05-09.md`: the consensus
//! remote-equivocation detector verified evidence and emitted a
//! `tracing::error!("SLASHABLE")` log, but no on-chain consequence
//! followed. There was no submission to `SlashingRegistryV2`, no
//! finalization tick, no stake-burn against `ValidatorSet`.
//!
//! `SlashingPipeline` ties the pieces together:
//!
//! ```text
//!   Detector                 Pipeline                 State
//!   ─────────                 ────────                 ─────
//!  HotStuff::on_vote
//!     │ RemoteEquivocation
//!     ▼
//!  EvidenceStore.put_evidence ──┐
//!                                ▼
//!                       Registry::submit_evidence ──► EvidenceStore.put_record
//!                                                       (status = Pending,
//!                                                        appeal_deadline)
//!
//!  per-block tick:
//!     Pipeline::tick_finalize(current_block)
//!     ├─ load_records_ready_to_finalize     (status==Pending && now>deadline)
//!     ├─ Registry::finalize_slash           (status → Confirmed)
//!     ├─ EvidenceStore.put_record           (persist transition)
//!     └─ ValidatorSet.apply_slash_burn      (debit self_stake, jail)
//! ```
//!
//! # Honest scope (Pass-11)
//!
//! - **In-scope**: pipeline orchestration, persistence wiring, the
//!   actual stake debit + jail on `ValidatorSet`, idempotency,
//!   restart-safety, deterministic in-memory E2E tests covering
//!   detection → submission → finalization → state mutation.
//! - **Deferred**: on-chain governance appeal flow, cross-validator
//!   correlated slashing on the SAME equivocation by multiple reporters
//!   (current path supports correlated *epochs*, not co-witness).
//!   These are explicitly documented and do NOT silently degrade
//!   security — they are conservative omissions.
//!
//! MB-5 (2026-06-27): whistleblower-bond escrow is now RocksDB-backed via
//! `EvidenceStore::put_bond` / `get_bond` / `list_bonds_for_record` (the
//! `SlashingBonds` column family). The legacy in-memory
//! `SlashingRegistryV2.pending_bonds` mirror has been removed — bond state
//! is durable across node restarts.

use crate::error::StakingError;
use crate::persistence::{EvidenceStore, evidence_to_double_sign, BondEntry, BondKind};
use crate::slashing_v2::{SlashingRegistryV2, SubmitOutcome, EvidenceType, EvidenceStatus};
use crate::validator::{ValidatorSet, ValidatorStatus};
use zbx_consensus::vote::EquivocationEvidence;
use zbx_types::{address::Address, H256};
use parking_lot::{Mutex, RwLock};
use std::sync::Arc;
use tracing::{info, warn, error};

/// Outcome of `tick_finalize` per record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedSlash {
    pub record_id:        H256,
    pub offender:         Address,
    pub burn_wei:         u128,
    /// Total whistleblower reward across ALL reporters (sum of per-
    /// reporter splits, modulo ≤ N-1 wei integer-division remainder).
    pub whistleblower_wei: u128,
    /// First reporter (kept for back-compat with consumers that only
    /// looked at a single submitter). Use `reporters` for the full
    /// co-witness list and `splits` for per-reporter shares.
    pub whistleblower:    Address,
    /// All reporters (first reporter + co-witnesses) that shared the
    /// whistleblower reward at finalization.
    pub reporters:        Vec<Address>,
    /// Per-reporter reward shares in the same order as `reporters`.
    pub splits:           Vec<u128>,
    pub jailed:           bool,
    /// Whether the slash promoted the offender all the way to
    /// `Tombstoned` (≥ 2 lifetime Confirmed slashes OR catastrophic
    /// `InvalidBlock` evidence). Tombstoned is permanent — operator
    /// status edits cannot reverse it.
    pub tombstoned:       bool,
}

/// Outcome of `SlashingPipeline::overturn_and_refund` — what was
/// actually un-done by a successful governance overturn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedOverturn {
    pub record_id:       H256,
    pub offender:        Address,
    /// ZBX re-credited to the offender's `self_stake`.
    pub refunded_wei:    u128,
    /// Appeal bond (APPEAL_BOND_WEI) returned to the offender.
    pub appeal_bond_refunded_wei: u128,
    /// Total whistleblower bonds forfeited (burnt). Anti-spam.
    pub whistleblower_bonds_forfeited_wei: u128,
    /// True if the overturn also un-jailed the offender. Only true
    /// when `record.jailed_by_slash` was set — we never undo a jail
    /// caused by an unrelated reason (liveness, operator).
    pub unjailed:        bool,
}

/// End-to-end slashing pipeline.
///
/// Holds references to the durable evidence store, the in-memory
/// (but persistence-backed) registry, and the live validator set.
/// All three are `Arc` because consensus driver, RPC, and
/// pipeline-tick task share them.
#[derive(Clone)]
pub struct SlashingPipeline {
    store:      EvidenceStore,
    registry:   Arc<Mutex<SlashingRegistryV2>>,
    /// Validator set is shared with the RPC layer (`rpc_state.validator_set`)
    /// so a stake-burn here is visible to `eth_getBalance` / staking views
    /// without a refresh tick. parking_lot RwLock matches the node's choice.
    validators: Arc<RwLock<ValidatorSet>>,
}

impl SlashingPipeline {
    pub fn new(
        store:      EvidenceStore,
        registry:   Arc<Mutex<SlashingRegistryV2>>,
        validators: Arc<RwLock<ValidatorSet>>,
    ) -> Self {
        Self { store, registry, validators }
    }

    /// Bootstrap the in-memory `SlashingRegistryV2` from persisted
    /// records. Called at node startup so a crash mid-window does
    /// not lose pending slashes.
    ///
    /// Returns the count of records rehydrated (informational).
    pub fn rehydrate_from_disk(&self) -> Result<usize, StakingError> {
        let records = self.store.load_all_records()?;
        let n = records.len();
        if n == 0 {
            return Ok(0);
        }
        let mut reg = self.registry.lock();
        for rec in records {
            reg.insert_rehydrated_record(rec);
        }
        info!(rehydrated = n, "slashing pipeline: records rehydrated from disk");
        Ok(n)
    }

    /// Ingest one freshly-detected `EquivocationEvidence`. The full
    /// happy path:
    ///
    /// 1. Re-verify (defence-in-depth — the detector also verified,
    ///    but a corrupted in-memory copy or test injection might not
    ///    have).
    /// 2. Persist evidence (durable, idempotent on content hash).
    /// 3. Convert to `SlashEvidenceV2::DoubleSign`.
    /// 4. Submit to `SlashingRegistryV2` (computes correlated slash,
    ///    sets `appeal_deadline = current_block + APPEAL_WINDOW`).
    /// 5. Persist the resulting `SlashEvidenceRecord`.
    ///
    /// Returns the registry record ID. If the evidence was already
    /// submitted (deduplication via offender/block/type tuple in
    /// `SlashEvidenceRecord::compute_id`), returns `Ok(existing_id)`
    /// rather than an error — the caller treats re-detection as
    /// idempotent.
    pub fn ingest_equivocation(
        &self,
        ev:               &EquivocationEvidence,
        submitter:        Address,
        current_block:    u64,
        current_epoch:    u64,
        offender_stake:   u128,
    ) -> Result<H256, StakingError> {
        if !ev.verify() {
            error!(validator = ?ev.validator,
                   "rejecting equivocation evidence — verify() returned false");
            return Err(StakingError::InvalidEvidence(
                "evidence failed BLS / structural re-verification".into()));
        }

        // 1. Persist raw evidence (idempotent).
        let _evidence_id = self.store.put_evidence(ev)?;

        // 2. Convert + submit. The registry computes the slash amount
        //    and appeal deadline. Co-witness path: a second honest
        //    reporter of the SAME equivocation is added to the
        //    record's `reporters` list (was silently dropped pre-
        //    upgrade) and will share the whistleblower reward.
        let evidence_v2 = evidence_to_double_sign(ev);
        let mut reg = self.registry.lock();
        let outcome = reg.submit_evidence(
            evidence_v2,
            submitter,
            current_block,
            current_epoch,
            offender_stake,
        )?;
        let record_id = outcome.record_id();

        // Snapshot the record (post-mutation), drop the registry lock
        // before any fsync.
        let record = reg.get_record(&record_id)
            .cloned()
            .ok_or_else(|| StakingError::Persistence(
                "submitted record vanished from registry".into()))?;
        drop(reg);

        // Always persist — for NewRecord it is the initial write,
        // for CoWitnessAdded it captures the appended reporter so a
        // crash before the next put_record doesn't lose the co-
        // witness.
        match outcome {
            SubmitOutcome::NewRecord(_) => {
                self.store.put_record(&record)?;
                // Whistleblower bond — the consensus auto-detection
                // path is the chain itself (not a user-spammable RPC),
                // so we record a 0-wei `Whistleblower` bond as a
                // ledger marker. A future operator-submitted evidence
                // tx will write the real EVIDENCE_BOND_WEI here.
                self.store.put_bond(&record_id, &submitter, &BondEntry {
                    wei: 0, kind: BondKind::Whistleblower,
                })?;
                info!(
                    record_id = ?record_id, offender = ?ev.validator,
                    slash_wei = record.final_slash_wei,
                    appeal_deadline = record.appeal_deadline,
                    "equivocation evidence submitted to slashing pipeline"
                );
            }
            SubmitOutcome::CoWitnessAdded(_) => {
                self.store.put_record(&record)?;
                self.store.put_bond(&record_id, &submitter, &BondEntry {
                    wei: 0, kind: BondKind::Whistleblower,
                })?;
                info!(
                    record_id = ?record_id, offender = ?ev.validator,
                    co_witness = ?submitter,
                    total_reporters = 1 + record.reporters.len(),
                    "co-witness reporter added — will share reward"
                );
            }
            SubmitOutcome::AlreadyRecorded(_) => {
                // Pure no-op idempotent re-detection by the same
                // reporter — no disk write needed.
                warn!(record_id = ?record_id, validator = ?ev.validator,
                      "equivocation re-detected by same reporter — idempotent");
            }
        }
        Ok(record_id)
    }

    /// Per-block tick — finalize any records whose appeal window
    /// has closed and apply the stake burn.
    ///
    /// The state mutation order is:
    /// 1. `Registry::finalize_slash` flips status → `Confirmed`.
    /// 2. We persist the updated record (commit-before-burn so a
    ///    crash between burn and persist cannot un-slash).
    /// 3. `ValidatorSet::apply_slash_burn` debits `self_stake`,
    ///    transitions `status` to `Jailed`, and credits the
    ///    whistleblower reward to the submitter's pending_rewards.
    ///
    /// Idempotent: replaying the tick on the same `current_block`
    /// is a no-op because finalize moves status away from `Pending`.
    pub fn tick_finalize(
        &self,
        current_block: u64,
    ) -> Result<Vec<AppliedSlash>, StakingError> {
        let mut applied = Vec::new();

        // ── Phase 0: crash-consistency replay ──────────────────────
        //
        // Pick up any Confirmed records whose burn never ran. This
        // happens when a previous tick (possibly in a previous
        // process) crashed/erred between `finalize_slash` (status
        // → Confirmed, persisted) and `apply_slash_burn_v2`. Replay
        // is byte-equivalent: splits are recomputed deterministically
        // from `(submitted_by + reporters + final_slash_wei)`.
        //
        // **Replay safety guard** (architect-review #3 hardening).
        //
        // Catches the "burn applied but post-burn put_record failed"
        // crash window: at that point validator state has been
        // mutated (stake debited, status flipped to Jailed/
        // Tombstoned) but the on-disk record still says
        // `burn_applied=false` AND may have `jailed_by_slash=false`
        // (the flag is set in the same failed put_record call).
        //
        // Three independent OR-conditions, ANY one of which causes
        // the replay loop to SKIP the record (declaring it
        // already-handled, or unsafe-to-replay):
        //   1. `self_stake < final_slash_wei` — strong proof the
        //      burn already ran. Weak/false-negative for small
        //      (e.g. 5%) slashes where stake remains well above
        //      the burn amount.
        //   2. `jailed_by_slash && status in {Jailed,Tombstoned}` —
        //      strong proof; only fires if the post-burn put_record
        //      reached disk.
        //   3. `status != Active` — CONSERVATIVE catch-all that
        //      covers the post-burn-persist-failure crash window
        //      regardless of the offender's pre-burn state
        //      (apply_slash_burn_v2 only transitions Active→Jailed;
        //      Pending/Unbonding/Inactive/Jailed/Tombstoned starts
        //      keep their status, so condition 2 would miss them).
        //      Trade-off: this MAY under-burn — if a validator was
        //      independently jailed for downtime or operator-paused
        //      between the original submit and the replay tick, we
        //      skip a legitimate burn. Under-burn is recoverable
        //      via a separate operator recovery tx (or a future
        //      durable applied-slash ledger keyed by record_id);
        //      double-burn permanently destroys stake. We accept
        //      the under-burn risk explicitly until that ledger
        //      lands as a follow-up.
        let to_replay = self.store.load_records_with_unapplied_burn()?;
        for rec in to_replay {
            // Skip records whose appeal window hasn't yet closed — a
            // future appeal could still flip them away from
            // Confirmed. (Cannot happen via current code paths, but
            // belt-and-suspenders against legacy on-disk records.)
            if current_block <= rec.appeal_deadline { continue; }
            let already_slashed = {
                let vs = self.validators.read();
                match vs.validators.get(&rec.offender) {
                    None => true, // unknown validator — can't replay safely
                    Some(v) => v.self_stake < rec.final_slash_wei
                        || (rec.jailed_by_slash && matches!(
                            v.status,
                            ValidatorStatus::Jailed
                            | ValidatorStatus::Tombstoned))
                        // Condition 3 — conservative non-Active
                        // skip. See banner comment above for the
                        // double-burn-vs-under-burn trade-off.
                        || v.status != ValidatorStatus::Active,
                }
            };
            if already_slashed {
                // Mark the flag to prevent re-scanning every tick.
                let mut reg = self.registry.lock();
                reg.set_burn_applied(&rec.id);
                if let Some(r2) = reg.get_record(&rec.id).cloned() {
                    drop(reg);
                    let _ = self.store.put_record(&r2);
                }
                continue;
            }
            let finalized = SlashingRegistryV2::recompute_splits(&rec);
            let force_tombstone = {
                let reg = self.registry.lock();
                let lifetime = reg.lifetime_confirmed_slashes(&rec.offender);
                lifetime >= 2 || rec.evidence_type == EvidenceType::InvalidBlock
            };
            let (jailed, tombstoned) = {
                let mut vs = self.validators.write();
                apply_slash_burn_v2(
                    &mut vs, rec.offender, finalized.slash_wei,
                    &finalized.splits, force_tombstone,
                )
            };
            // Close the two-phase cycle: stamp burn_applied (+ the
            // jailed_by_slash flag if THIS replay jailed them) and
            // persist the record again.
            {
                let mut reg = self.registry.lock();
                reg.set_burn_applied(&rec.id);
                if jailed || tombstoned {
                    reg.set_jailed_by_slash(&rec.id, true);
                }
                if let Some(r2) = reg.get_record(&rec.id).cloned() {
                    drop(reg);
                    self.store.put_record(&r2)?;
                }
            }
            applied.push(AppliedSlash {
                record_id:        rec.id,
                offender:         rec.offender,
                burn_wei:         finalized.slash_wei,
                whistleblower_wei: finalized.total_reward,
                whistleblower:    finalized.splits.first().map(|(a, _)| *a).unwrap_or(rec.offender),
                reporters:        finalized.splits.iter().map(|(a, _)| *a).collect(),
                splits:           finalized.splits.iter().map(|(_, w)| *w).collect(),
                jailed, tombstoned,
            });
            info!(record_id = ?rec.id, offender = ?rec.offender,
                  burn_wei = finalized.slash_wei,
                  "slashing pipeline: replayed unapplied burn (crash recovery)");
        }

        let ready = self.store.load_records_ready_to_finalize(current_block)?;
        if ready.is_empty() {
            return Ok(applied);
        }

        applied.reserve(ready.len());
        for rec in ready {
            // Only finalize if registry still says Pending — guards
            // against a concurrent appeal between disk-load and
            // finalize.
            let result = {
                let mut reg = self.registry.lock();
                reg.finalize_slash(rec.id, current_block)?
            };

            let Some(finalized) = result else {
                // Registry already moved it (e.g., appealed since
                // last load). Skip.
                continue;
            };

            // SEC-2026-05-09 Pass-11 (architect-review follow-up):
            // ATOMICITY. We MUST NOT early-return on persist failure
            // between finalize and burn — that would leave the
            // in-memory registry as Confirmed but the on-disk record
            // as Pending AND skip the burn for the rest of this
            // process lifetime. Persist Confirmed record BEFORE the
            // burn; on failure propagate without burning so the next
            // tick re-attempts (finalize_slash is idempotent on
            // already-Confirmed records).
            let updated = {
                let reg = self.registry.lock();
                reg.get_record(&rec.id).cloned()
            };
            let Some(r) = updated else {
                return Err(StakingError::Persistence(format!(
                    "post-finalize record {:?} vanished from registry",
                    rec.id
                )));
            };
            self.store.put_record(&r)?;

            // Tombstone trigger: the registry has already flipped THIS
            // record to Confirmed, so `lifetime_confirmed_slashes`
            // includes it. >= 2 means "at least the 2nd lifetime
            // slash" — operator should not be able to recover.
            // InvalidBlock evidence is always catastrophic.
            let force_tombstone = {
                let reg = self.registry.lock();
                let lifetime = reg.lifetime_confirmed_slashes(&rec.offender);
                lifetime >= 2 || r.evidence_type == EvidenceType::InvalidBlock
            };

            // State mutation: burn from validator's self_stake, jail
            // OR tombstone, and credit each reporter their split.
            let (jailed, tombstoned) = {
                let mut vs = self.validators.write();
                apply_slash_burn_v2(
                    &mut vs, rec.offender, finalized.slash_wei,
                    &finalized.splits, force_tombstone,
                )
            };

            // Two-phase finalize commit: stamp `burn_applied = true`
            // (and `jailed_by_slash = true` if THIS slash is what
            // flipped the validator) then persist. The combined
            // re-persist closes the crash-recovery cycle: if we got
            // here, the burn HAS been applied; on the next process
            // boot the replay loop will see `burn_applied = true`
            // and skip the record.
            //
            // Persist failure here is a real risk — it means the
            // next tick / boot would replay the burn (double-slash).
            // We surface as an error so the caller can decide policy
            // (typically: log + halt the tick; the replay logic also
            // has a self_stake guard for defence-in-depth).
            {
                let mut reg = self.registry.lock();
                reg.set_burn_applied(&rec.id);
                if jailed || tombstoned {
                    reg.set_jailed_by_slash(&rec.id, true);
                }
                if let Some(r2) = reg.get_record(&rec.id).cloned() {
                    drop(reg);
                    self.store.put_record(&r2)?;
                }
            }

            applied.push(AppliedSlash {
                record_id:        rec.id,
                offender:         rec.offender,
                burn_wei:         finalized.slash_wei,
                whistleblower_wei: finalized.total_reward,
                whistleblower:    finalized.splits.first().map(|(a, _)| *a).unwrap_or(rec.offender),
                reporters:        finalized.splits.iter().map(|(a, _)| *a).collect(),
                splits:           finalized.splits.iter().map(|(_, w)| *w).collect(),
                jailed,
                tombstoned,
            });

            info!(
                record_id = ?rec.id,
                offender = ?rec.offender,
                burn_wei = finalized.slash_wei,
                total_reward_wei = finalized.total_reward,
                reporters = finalized.splits.len(),
                jailed, tombstoned,
                "slashing pipeline: stake burnt + validator jailed/tombstoned",
            );
        }
        Ok(applied)
    }

    /// Overturn an appealed slash + refund stake + un-jail (if
    /// applicable) + refund appeal bond + forfeit whistleblower
    /// bonds. Atomic from the caller's perspective — every state
    /// change is committed before the function returns Ok.
    ///
    /// The caller is governance / RPC — appeals are off-chain-voted
    /// in this iteration; on-chain governance integration is a
    /// separate sprint.
    pub fn overturn_and_refund(
        &self,
        evidence_id: H256,
    ) -> Result<AppliedOverturn, StakingError> {
        // ── Atomicity model (architect-review follow-up) ──────────
        //
        // The architect flagged the original "flip-status-first"
        // ordering as crash-unsafe: a crash after persisting
        // Overturned but before refunding stake would terminally
        // strand funds. The current ordering moves ALL financial
        // work BEFORE the on-disk Overturned commit, so the commit
        // is the single transactional boundary:
        //
        //   1. SNAPSHOT the Appealed record (must be Appealed; error
        //      otherwise — `OverturnNotAllowed`).
        //   2. List bonds and compute refund / forfeit amounts.
        //   3. Credit offender's self_stake + un-jail (in-memory).
        //   4. Credit appeal-bond refund into pending_rewards.
        //   5. Flip registry (Appealed → Overturned) + persist the
        //      Overturned record. THIS is the commit point.
        //   6. Best-effort delete the now-settled bond entries.
        //
        // Crash before step 5: in-memory mutations are lost, on-disk
        // record is still Appealed, bonds are intact → next call to
        // `overturn_and_refund` retries cleanly from step 1.
        //
        // Crash between step 5 and step 6: record is Overturned (so
        // no double-credit on retry) but a few bond entries may be
        // stranded on disk. They are no longer credited to anyone —
        // overturn_and_refund will refuse a retry (status check), and
        // the stranded entries are harmless janitorial residue. A
        // future sweep tool can `list_bonds_for_record` on Overturned
        // records and delete leftovers.
        let record = {
            let reg = self.registry.lock();
            reg.get_record(&evidence_id).cloned()
        }.ok_or(StakingError::EvidenceNotFound)?;
        if record.status != EvidenceStatus::Appealed {
            return Err(StakingError::OverturnNotAllowed);
        }
        // **Refund gating** (architect-review follow-up #2):
        //
        // Stake refund + un-jail ONLY apply if the slash was actually
        // burned. With the current Pending-only appeal flow
        // (`file_appeal` requires `status == Pending`, slashing_v2.rs)
        // an Appealed record by construction has `burn_applied=false`
        // — nothing was burned, so there is nothing to refund and
        // the validator was never jailed-by-this-slash.
        //
        // This gate is forward-compatible: a future "delayed appeal"
        // feature that allows appeals on Confirmed records (after the
        // burn has been applied) will hit `burn_applied=true` here
        // and trigger the refund + un-jail path automatically. Without
        // this gate the function would mint stake from nothing on
        // overturn, since `record.final_slash_wei` is a *target*
        // amount, not proof that the burn was executed.
        let refunded_wei = if record.burn_applied {
            record.final_slash_wei
        } else {
            0
        };

        // Compute refund/forfeit amounts up-front.
        let bonds = self.store.list_bonds_for_record(&evidence_id)?;
        let mut appeal_bond_refunded = 0u128;
        let mut whistleblower_bonds_forfeited = 0u128;
        for (_, entry) in &bonds {
            match entry.kind {
                BondKind::Appeal => {
                    appeal_bond_refunded =
                        appeal_bond_refunded.saturating_add(entry.wei);
                }
                BondKind::Whistleblower => {
                    whistleblower_bonds_forfeited =
                        whistleblower_bonds_forfeited.saturating_add(entry.wei);
                }
            }
        }

        // In-memory financial work — undone automatically by process
        // restart if we crash before the commit at step 5.
        let mut unjailed = false;
        {
            let mut vs = self.validators.write();
            if let Some(v) = vs.validators.get_mut(&record.offender) {
                v.self_stake = v.self_stake.saturating_add(refunded_wei);
                // Un-jail only if THIS slash actually applied the
                // burn (burn_applied=true) AND that burn is what
                // jailed them. Same forward-compat reasoning as
                // refunded_wei above.
                if record.burn_applied
                    && record.jailed_by_slash
                    && v.status == ValidatorStatus::Jailed
                {
                    // Defence-in-depth: NEVER un-tombstone, even if
                    // jailed_by_slash is somehow set on a Tombstoned
                    // record. Tombstoning happens only after Active.
                    v.status = ValidatorStatus::Pending;
                    unjailed = true;
                }
                // Refund appeal bond as pending_rewards (offender
                // claims via existing ClaimRewards flow).
                v.pending_rewards = v.pending_rewards
                    .saturating_add(appeal_bond_refunded);
            } else {
                warn!(offender = ?record.offender,
                      "overturn refund: offender no longer in validator set");
            }
        }

        // ── COMMIT POINT ─────────────────────────────────────────
        // Flip registry (Appealed → Overturned) + persist the
        // Overturned record. After this returns Ok, the operation
        // is durably committed — retries will see Overturned and
        // refuse via OverturnNotAllowed.
        {
            let mut reg = self.registry.lock();
            let _ = reg.overturn_slash(evidence_id)?;
        }
        let updated = {
            let reg = self.registry.lock();
            reg.get_record(&evidence_id).cloned()
        }.ok_or_else(|| StakingError::Persistence(
            "overturned record vanished from registry".into()))?;
        self.store.put_record(&updated)?;

        // ── Post-commit cleanup ──────────────────────────────────
        // Settle the bond ledger — best-effort. A failure here
        // leaves stranded entries (harmless residue) but does NOT
        // affect the financial outcome (already credited above).
        for (reporter, _) in &bonds {
            if let Err(e) = self.store.delete_bond(&evidence_id, reporter) {
                warn!(?e, record_id = ?evidence_id, reporter = ?reporter,
                      "post-overturn bond cleanup failed — \
                       leaves harmless residue on disk");
            }
        }

        let out = AppliedOverturn {
            record_id: evidence_id,
            offender:  record.offender,
            refunded_wei,
            appeal_bond_refunded_wei: appeal_bond_refunded,
            whistleblower_bonds_forfeited_wei: whistleblower_bonds_forfeited,
            unjailed,
        };
        info!(
            record_id = ?evidence_id,
            offender = ?record.offender,
            refunded_wei,
            appeal_bond_refunded,
            whistleblower_bonds_forfeited,
            unjailed,
            "slashing pipeline: appeal overturned — stake refunded"
        );
        Ok(out)
    }

    /// Expose the store for `dispatch_file_appeal_tx` (the on-chain
    /// FileAppeal handler needs to persist the bond entry).
    pub fn store(&self) -> &EvidenceStore { &self.store }

    /// Expose the registry for the on-chain FileAppeal handler.
    pub fn registry(&self) -> &Arc<Mutex<SlashingRegistryV2>> { &self.registry }

    pub fn pending_count(&self) -> usize {
        self.registry.lock().pending_count()
    }
}

/// Legacy single-submitter burn — kept for back-compat with any
/// external caller that still uses the pre-upgrade signature. New
/// pipeline path uses `apply_slash_burn_v2`.
pub fn apply_slash_burn(
    vs:                   &mut ValidatorSet,
    offender:             Address,
    slash_wei:            u128,
    whistleblower:        Address,
    whistleblower_reward: u128,
) -> bool {
    let splits = vec![(whistleblower, whistleblower_reward)];
    let (jailed, _) = apply_slash_burn_v2(vs, offender, slash_wei, &splits, false);
    jailed
}

/// Apply a confirmed slash to the validator set with co-witness
/// reward splits AND optional tombstoning.
///
/// - Debits `slash_wei` from the offender's `self_stake` (saturating
///   at zero — over-slash is impossible by registry construction,
///   but stake may have been withdrawn since submission; saturating
///   is safer than panic).
/// - Transitions offender to `Tombstoned` if `force_tombstone` is true
///   (caller decides based on lifetime slash count + evidence type),
///   else to `Jailed` if they were `Active`.
/// - Credits each reporter's share via `splits` into their
///   `pending_rewards`. Non-validator reporters are silently skipped
///   here — their bond ledger entry covers them via a future on-
///   chain claim path.
///
/// Returns `(newly_jailed, newly_tombstoned)`.
pub fn apply_slash_burn_v2(
    vs:              &mut ValidatorSet,
    offender:        Address,
    slash_wei:       u128,
    splits:          &[(Address, u128)],
    force_tombstone: bool,
) -> (bool, bool) {
    let mut newly_jailed = false;
    let mut newly_tombstoned = false;
    if let Some(v) = vs.validators.get_mut(&offender) {
        let actual = slash_wei.min(v.self_stake);
        v.self_stake = v.self_stake.saturating_sub(actual);
        // Never demote Tombstoned (already permanent). Active is the
        // only transition source for both Jailed and Tombstoned —
        // a validator already Jailed for liveness gets the slash but
        // retains its Jailed status (or Tombstone if forced).
        match (v.status, force_tombstone) {
            (ValidatorStatus::Tombstoned, _) => { /* no-op */ }
            (_, true) => {
                v.status = ValidatorStatus::Tombstoned;
                newly_tombstoned = true;
            }
            (ValidatorStatus::Active, false) => {
                v.status = ValidatorStatus::Jailed;
                newly_jailed = true;
            }
            _ => { /* Jailed/Pending/Unbonding/Inactive — keep */ }
        }
    } else {
        warn!(?offender,
              "slash: offender not in validator set — no stake to burn");
    }
    for (reporter, reward_wei) in splits {
        if let Some(w) = vs.validators.get_mut(reporter) {
            w.pending_rewards = w.pending_rewards.saturating_add(*reward_wei);
        }
        // Non-validator reporters are tracked via the bond ledger;
        // their reward is owed but settled via a separate claim path.
    }
    // Offender must leave the active set on jail OR tombstone.
    vs.active_set.retain(|a| *a != offender);
    (newly_jailed, newly_tombstoned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slashing_v2::{SlashingRegistryV2, EvidenceType};
    use crate::validator::{Validator, ValidatorStatus};
    use zbx_consensus::vote::{Vote, VoteData};
    use zbx_crypto::bls::BlsPrivKey;
    use zbx_storage::ZbxDb;
    use tempfile::TempDir;

    const STAKE: u128 = 100_000 * 10u128.pow(18);

    fn fresh_pipeline() -> (TempDir, SlashingPipeline, BlsPrivKey, Address) {
        let tmp = TempDir::new().unwrap();
        let db  = Arc::new(ZbxDb::open(tmp.path()).unwrap());
        let store = EvidenceStore::new(db);
        let registry = Arc::new(Mutex::new(SlashingRegistryV2::new(10)));

        let offender = Address([0xaa; 20]);
        let sk = BlsPrivKey::from_bytes(&[42u8; 32]).unwrap();
        let pk = sk.to_pubkey();

        let mut vs = ValidatorSet::new();
        vs.validators.insert(offender, Validator {
            address: offender, bls_pubkey: pk,
            self_stake: STAKE, delegated_stake: 0,
            commission_bps: 500, status: ValidatorStatus::Active,
            last_signed_block: 0, pending_rewards: 0,
            delegator_reward_pool: 0, pool_denominator: 0, registered_epoch: 0,
        });
        vs.active_set = vec![offender];
        let validators = Arc::new(RwLock::new(vs));

        (tmp, SlashingPipeline::new(store, registry, validators), sk, offender)
    }

    fn mk_evidence(sk: &BlsPrivKey, addr: Address,
                    hash_a: H256, hash_b: H256) -> EquivocationEvidence {
        let pk = sk.to_pubkey();
        let mk_vote = |h: H256| {
            let data = VoteData {
                block_hash: h, block_number: 5, phase: 0, epoch: 0,
            };
            let sig_msg = zbx_crypto::keccak::keccak256(&data.signing_bytes());
            let sig = sk.sign(&sig_msg);
            Vote { data, voter: addr, signature: sig }
        };
        EquivocationEvidence {
            validator: addr, round: 5, phase: 0,
            vote_a: mk_vote(hash_a),
            vote_b: mk_vote(hash_b),
            pubkey: pk,
        }
    }

    #[test]
    fn full_flow_ingest_finalize_burn() {
        let (_tmp, pipeline, sk, offender) = fresh_pipeline();
        let ev = mk_evidence(&sk, offender,
                              H256([1u8; 32]), H256([2u8; 32]));
        let submitter = Address([0xbb; 20]);

        // 1. Submit
        let id = pipeline.ingest_equivocation(
            &ev, submitter, /*block*/ 1, /*epoch*/ 0, STAKE).unwrap();
        assert_eq!(pipeline.pending_count(), 1);

        // 2. Tick before deadline → no-op
        let none = pipeline.tick_finalize(100).unwrap();
        assert!(none.is_empty(), "tick before appeal deadline must no-op");

        // 3. Tick after deadline → finalize + burn
        let applied = pipeline.tick_finalize(
            crate::slashing_v2::APPEAL_WINDOW_BLOCKS + 10).unwrap();
        assert_eq!(applied.len(), 1);
        let a = &applied[0];
        assert_eq!(a.record_id, id);
        assert_eq!(a.offender, offender);
        assert!(a.burn_wei > 0);
        assert!(a.jailed);

        // 4. Validator state mutation
        {
            let vs = pipeline.validators.read();
            let v = vs.validators.get(&offender).unwrap();
            assert_eq!(v.self_stake, STAKE - a.burn_wei);
            assert_eq!(v.status, ValidatorStatus::Jailed);
            assert!(!vs.active_set.contains(&offender),
                    "jailed validator must be removed from active set");
        }
    }

    #[test]
    fn ingest_rejects_unverified_evidence() {
        let (_tmp, pipeline, sk, offender) = fresh_pipeline();
        let mut ev = mk_evidence(&sk, offender,
                                  H256([1u8; 32]), H256([2u8; 32]));
        // Tamper with vote_b's hash so the cached signature no
        // longer matches → verify() returns false.
        ev.vote_b.data.block_hash = H256([99u8; 32]);
        let err = pipeline.ingest_equivocation(
            &ev, Address([0xbb; 20]), 1, 0, STAKE).unwrap_err();
        match err {
            StakingError::InvalidEvidence(_) => {}
            e => panic!("expected InvalidEvidence, got {e:?}"),
        }
    }

    #[test]
    fn re_detection_is_idempotent() {
        let (_tmp, pipeline, sk, offender) = fresh_pipeline();
        let ev = mk_evidence(&sk, offender,
                              H256([1u8; 32]), H256([2u8; 32]));
        let submitter = Address([0xbb; 20]);

        let id1 = pipeline.ingest_equivocation(
            &ev, submitter, 1, 0, STAKE).unwrap();
        let id2 = pipeline.ingest_equivocation(
            &ev, submitter, 1, 0, STAKE).unwrap();
        assert_eq!(id1, id2, "re-detection must return same record ID");
        assert_eq!(pipeline.pending_count(), 1);
    }

    #[test]
    fn rehydrate_restores_records_after_restart() {
        // Persist a record under one pipeline, then build a fresh
        // pipeline against the same DB — rehydrate must restore the
        // pending record so finalize still fires.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().to_owned();
        let offender = Address([0xcc; 20]);
        let sk = BlsPrivKey::from_bytes(&[88u8; 32]).unwrap();

        // Phase A: ingest, then drop everything.
        let id = {
            let db  = Arc::new(ZbxDb::open(&path).unwrap());
            let store = EvidenceStore::new(db);
            let registry = Arc::new(Mutex::new(SlashingRegistryV2::new(10)));
            let mut vs = ValidatorSet::new();
            vs.validators.insert(offender, Validator {
                address: offender, bls_pubkey: sk.to_pubkey(),
                self_stake: STAKE, delegated_stake: 0,
                commission_bps: 500, status: ValidatorStatus::Active,
                last_signed_block: 0, pending_rewards: 0,
                delegator_reward_pool: 0, pool_denominator: 0, registered_epoch: 0,
            });
            vs.active_set = vec![offender];
            let validators = Arc::new(RwLock::new(vs));
            let pipeline = SlashingPipeline::new(store, registry, validators);

            let ev = mk_evidence(&sk, offender,
                                  H256([1; 32]), H256([2; 32]));
            pipeline.ingest_equivocation(
                &ev, Address([0xbb; 20]), 1, 0, STAKE).unwrap()
        };

        // Phase B: fresh pipeline, same DB. Rehydrate + finalize.
        let db2 = Arc::new(ZbxDb::open(&path).unwrap());
        let store2 = EvidenceStore::new(db2);
        let registry2 = Arc::new(Mutex::new(SlashingRegistryV2::new(10)));
        let mut vs2 = ValidatorSet::new();
        vs2.validators.insert(offender, Validator {
            address: offender, bls_pubkey: sk.to_pubkey(),
            self_stake: STAKE, delegated_stake: 0,
            commission_bps: 500, status: ValidatorStatus::Active,
            last_signed_block: 0, pending_rewards: 0,
            delegator_reward_pool: 0, pool_denominator: 0, registered_epoch: 0,
        });
        vs2.active_set = vec![offender];
        let validators2 = Arc::new(RwLock::new(vs2));
        let pipeline2 = SlashingPipeline::new(
            store2, registry2.clone(), validators2);

        let n = pipeline2.rehydrate_from_disk().unwrap();
        assert_eq!(n, 1, "must rehydrate the persisted record");
        assert_eq!(pipeline2.pending_count(), 1);

        // Finalize after window
        let applied = pipeline2.tick_finalize(
            crate::slashing_v2::APPEAL_WINDOW_BLOCKS + 10).unwrap();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].record_id, id);
        assert!(applied[0].jailed);
    }

    #[test]
    fn appealed_record_is_not_finalized() {
        let (_tmp, pipeline, sk, offender) = fresh_pipeline();
        let ev = mk_evidence(&sk, offender,
                              H256([1; 32]), H256([2; 32]));
        let id = pipeline.ingest_equivocation(
            &ev, Address([0xbb; 20]), 1, 0, STAKE).unwrap();

        // File appeal before deadline
        pipeline.registry.lock().file_appeal(id, 100).unwrap();

        // Tick after deadline → still no burn (status=Appealed)
        let applied = pipeline.tick_finalize(
            crate::slashing_v2::APPEAL_WINDOW_BLOCKS + 10).unwrap();
        assert!(applied.is_empty(), "appealed record must not auto-finalize");
        let vs = pipeline.validators.read();
        assert_eq!(vs.validators.get(&offender).unwrap().self_stake, STAKE,
                   "no stake burnt while under appeal");
    }

    // ─── Slashing-upgrade tests ─────────────────────────────────────

    /// Two honest reporters submit the SAME equivocation evidence.
    /// Pre-upgrade: the second reporter was silently dropped. Post-
    /// upgrade: they are added to `reporters`, the disk record reflects
    /// it, and `tick_finalize` splits the whistleblower reward equally
    /// between them.
    #[test]
    fn co_witness_two_reporters_split_reward_equally() {
        let (_tmp, pipeline, sk, offender) = fresh_pipeline();
        // Register both reporters as validators so the credit-to-
        // pending_rewards path can be observed.
        let reporter_a = Address([0xa1; 20]);
        let reporter_b = Address([0xa2; 20]);
        {
            let mut vs = pipeline.validators.write();
            let sk_a = BlsPrivKey::from_bytes(&[10u8; 32]).unwrap();
            let sk_b = BlsPrivKey::from_bytes(&[11u8; 32]).unwrap();
            for (addr, sk) in [(reporter_a, sk_a), (reporter_b, sk_b)] {
                vs.validators.insert(addr, Validator {
                    address: addr, bls_pubkey: sk.to_pubkey(),
                    self_stake: STAKE, delegated_stake: 0,
                    commission_bps: 500, status: ValidatorStatus::Active,
                    last_signed_block: 0, pending_rewards: 0,
                    delegator_reward_pool: 0, pool_denominator: 0,
                    registered_epoch: 0,
                });
            }
        }
        let ev = mk_evidence(&sk, offender, H256([7; 32]), H256([8; 32]));

        let id_a = pipeline.ingest_equivocation(&ev, reporter_a, 1, 0, STAKE).unwrap();
        let id_b = pipeline.ingest_equivocation(&ev, reporter_b, 1, 0, STAKE).unwrap();
        assert_eq!(id_a, id_b, "same evidence id reused for co-witness");
        // Bond ledger now holds entries for BOTH reporters.
        let bonds = pipeline.store().list_bonds_for_record(&id_a).unwrap();
        assert_eq!(bonds.len(), 2, "both reporters get bond entries");

        // Finalize after window
        let applied = pipeline.tick_finalize(
            crate::slashing_v2::APPEAL_WINDOW_BLOCKS + 10).unwrap();
        assert_eq!(applied.len(), 1);
        let a = &applied[0];
        assert_eq!(a.reporters.len(), 2,
                   "both reporters present in finalized splits");
        // Equal split (integer division, ≤ 1 wei remainder).
        assert_eq!(a.splits[0], a.splits[1],
                   "co-witness split must be equal");
        let total: u128 = a.splits.iter().sum();
        assert!(total <= a.whistleblower_wei
                && a.whistleblower_wei - total < a.reporters.len() as u128);
        // Both reporters credited into pending_rewards.
        let vs = pipeline.validators.read();
        assert!(vs.validators.get(&reporter_a).unwrap().pending_rewards > 0);
        assert!(vs.validators.get(&reporter_b).unwrap().pending_rewards > 0);
        assert_eq!(
            vs.validators.get(&reporter_a).unwrap().pending_rewards,
            vs.validators.get(&reporter_b).unwrap().pending_rewards,
            "both reporters credited the same share");
    }

    /// On-chain `FileAppeal` happy path: offender files appeal via
    /// the new dispatcher, bond is persisted, finalize is suppressed.
    /// Also covers rejection: non-offender sender and wrong bond value.
    #[test]
    fn file_appeal_dispatch_happy_and_sad_paths() {
        use crate::tx_handler::dispatch_file_appeal_tx;
        use zbx_types::staking_tx::APPEAL_BOND_WEI;

        let (_tmp, pipeline, sk, offender) = fresh_pipeline();
        let ev = mk_evidence(&sk, offender, H256([3; 32]), H256([4; 32]));
        let id = pipeline.ingest_equivocation(
            &ev, Address([0xbb; 20]), 1, 0, STAKE).unwrap();

        // Sad path 1: non-offender sender → AppealNotByOffender.
        let stranger = Address([0x99; 20]);
        let err = dispatch_file_appeal_tx(id, stranger, APPEAL_BOND_WEI, 100,
                                           &pipeline).unwrap_err();
        assert!(matches!(err, StakingError::AppealNotByOffender),
                "got {err:?}");

        // Sad path 2: wrong bond value → AppealBondMismatch.
        let err = dispatch_file_appeal_tx(id, offender, APPEAL_BOND_WEI - 1, 100,
                                           &pipeline).unwrap_err();
        assert!(matches!(err, StakingError::AppealBondMismatch { .. }),
                "got {err:?}");

        // Happy path: offender + correct bond → record flips to
        // Appealed, bond ledger holds an Appeal entry, finalize tick
        // is a no-op.
        let gas = dispatch_file_appeal_tx(id, offender, APPEAL_BOND_WEI, 100,
                                           &pipeline).unwrap();
        assert_eq!(gas, crate::tx_handler::STAKING_GAS_FILE_APPEAL);
        let bond = pipeline.store().get_bond(&id, &offender).unwrap()
            .expect("appeal bond must be persisted");
        assert_eq!(bond.wei, APPEAL_BOND_WEI);
        assert_eq!(bond.kind, BondKind::Appeal);

        // Finalize must skip the appealed record.
        let none = pipeline.tick_finalize(
            crate::slashing_v2::APPEAL_WINDOW_BLOCKS + 10).unwrap();
        assert!(none.is_empty(), "appealed record must not finalize");
        // Stake untouched.
        assert_eq!(pipeline.validators.read().validators.get(&offender).unwrap().self_stake, STAKE);
    }

    /// Overturn after appeal: stake re-credited, validator un-jailed
    /// only if `jailed_by_slash` was set, appeal bond refunded as
    /// pending_rewards, whistleblower bonds forfeited. Also exercises
    /// the sad path (overturn on Pending status → OverturnNotAllowed).
    #[test]
    fn overturn_and_refund_restores_stake_and_unjails() {
        use crate::tx_handler::dispatch_file_appeal_tx;
        use zbx_types::staking_tx::APPEAL_BOND_WEI;

        let (_tmp, pipeline, sk, offender) = fresh_pipeline();
        let whistleblower = Address([0xbb; 20]);
        let ev = mk_evidence(&sk, offender, H256([5; 32]), H256([6; 32]));
        let id = pipeline.ingest_equivocation(
            &ev, whistleblower, 1, 0, STAKE).unwrap();

        // Sad path: overturn before appeal → OverturnNotAllowed.
        let err = pipeline.overturn_and_refund(id).unwrap_err();
        assert!(matches!(err, StakingError::OverturnNotAllowed), "got {err:?}");

        // File appeal so overturn becomes allowed.
        dispatch_file_appeal_tx(id, offender, APPEAL_BOND_WEI, 100,
                                 &pipeline).unwrap();

        // Pending-only appeal flow: `burn_applied = false` at appeal
        // time, so overturn refunds the appeal bond + forfeits the
        // whistleblower bond, but does NOT credit self_stake or
        // un-jail (nothing was ever burned). This matches the
        // refund-gate logic in `overturn_and_refund`.
        let out = pipeline.overturn_and_refund(id).unwrap();
        assert_eq!(out.refunded_wei, 0,
                   "no burn happened (Pending-only appeal) → no stake refund");
        assert_eq!(out.appeal_bond_refunded_wei, APPEAL_BOND_WEI);
        assert!(!out.unjailed,
                "validator was never jailed-by-this-slash (no burn ran)");

        // Validator state: stake UNCHANGED + status unchanged +
        // appeal bond credited to pending_rewards.
        let vs = pipeline.validators.read();
        let v = vs.validators.get(&offender).unwrap();
        assert_eq!(v.self_stake, STAKE, "stake untouched (nothing was burnt)");
        assert_eq!(v.status, ValidatorStatus::Active,
                   "status untouched (Pending-only appeal never jailed)");
        assert_eq!(v.pending_rewards, APPEAL_BOND_WEI,
                   "appeal bond refunded as pending_rewards");
        // Bond ledger empty after overturn.
        let bonds = pipeline.store().list_bonds_for_record(&id).unwrap();
        assert!(bonds.is_empty(), "all bonds settled (refunded or forfeit) on overturn");
    }

    /// Upgrade-boundary regression test: a record submitted pre-
    /// upgrade (simulated by clearing format_version after submit)
    /// must still be eligible for two-phase replay if a crash
    /// happens between the Confirmed-persist and the burn.
    ///
    /// `finalize_slash` stamps `format_version = 1` at the moment
    /// of status transition, so the replay loop's strict triple-gate
    /// (`format_version >= 1 && Confirmed && !burn_applied`) DOES
    /// catch the record.
    #[test]
    fn legacy_pending_record_finalized_post_upgrade_is_replayable() {
        let (_tmp, pipeline, sk, offender) = fresh_pipeline();
        let ev = mk_evidence(&sk, offender, H256([77; 32]), H256([88; 32]));
        let id = pipeline.ingest_equivocation(
            &ev, Address([0xbb; 20]), 1, 0, STAKE).unwrap();
        // Simulate a pre-upgrade record: clear format_version in
        // both memory + on-disk.
        {
            let mut reg = pipeline.registry().lock();
            let r = reg.records_mut().get_mut(&id).unwrap();
            r.format_version = 0;
        }
        let r0 = pipeline.registry().lock().get_record(&id).cloned().unwrap();
        pipeline.store().put_record(&r0).unwrap();

        // Run finalize_slash directly so we can intercept BEFORE the
        // burn. (tick_finalize does flip+persist+burn in one pass; we
        // need the post-finalize / pre-burn state on disk to simulate
        // the crash.)
        {
            let mut reg = pipeline.registry().lock();
            let _ = reg.finalize_slash(id,
                crate::slashing_v2::APPEAL_WINDOW_BLOCKS + 10).unwrap();
        }
        // Persist the post-finalize record but DO NOT run the burn —
        // this is the crash window.
        let r1 = pipeline.registry().lock().get_record(&id).cloned().unwrap();
        assert_eq!(r1.status, EvidenceStatus::Confirmed);
        assert_eq!(r1.format_version, 1,
                   "finalize_slash must stamp the current format version");
        assert!(!r1.burn_applied, "burn not yet applied");
        pipeline.store().put_record(&r1).unwrap();

        // The replay scan must include this record.
        let to_replay = pipeline.store().load_records_with_unapplied_burn().unwrap();
        assert_eq!(to_replay.len(), 1, "upgraded record is replay-eligible");
        assert_eq!(to_replay[0].id, id);

        // Next tick replays the burn cleanly.
        let stake_before = pipeline.validators.read()
            .validators.get(&offender).unwrap().self_stake;
        let applied = pipeline.tick_finalize(
            crate::slashing_v2::APPEAL_WINDOW_BLOCKS + 10).unwrap();
        assert_eq!(applied.len(), 1, "burn replayed");
        let stake_after = pipeline.validators.read()
            .validators.get(&offender).unwrap().self_stake;
        assert!(stake_after < stake_before, "burn debited stake on replay");

        // And the replay loop closes the cycle — second tick is a no-op.
        let again = pipeline.tick_finalize(
            crate::slashing_v2::APPEAL_WINDOW_BLOCKS + 20).unwrap();
        assert!(again.is_empty(), "second tick must not re-burn");
    }

    /// Regression for the post-burn-persist-failure crash window
    /// with a NON-Active offender (Pending start state, small 5%
    /// slash → stake remains well above slash amount). Conditions
    /// 1 and 2 of the replay guard would both miss; condition 3
    /// (status != Active) catches it.
    #[test]
    fn replay_skips_when_offender_status_is_non_active_small_slash() {
        let (_tmp, pipeline, sk, offender) = fresh_pipeline();
        // Mutate offender status to Pending BEFORE the slash so
        // apply_slash_burn_v2 keeps the status (its match arm only
        // transitions Active→Jailed).
        {
            let mut vs = pipeline.validators.write();
            vs.validators.get_mut(&offender).unwrap().status =
                ValidatorStatus::Pending;
        }
        let ev = mk_evidence(&sk, offender, H256([81; 32]), H256([82; 32]));
        let id = pipeline.ingest_equivocation(
            &ev, Address([0xbb; 20]), 1, 0, STAKE).unwrap();
        // Drive finalize + persist Confirmed(!burn_applied).
        {
            let mut reg = pipeline.registry().lock();
            reg.finalize_slash(id,
                crate::slashing_v2::APPEAL_WINDOW_BLOCKS + 10).unwrap();
        }
        let r1 = pipeline.registry().lock().get_record(&id).cloned().unwrap();
        pipeline.store().put_record(&r1).unwrap();
        // Apply the burn directly but DO NOT persist burn_applied.
        let pre = pipeline.validators.read()
            .validators.get(&offender).unwrap().self_stake;
        {
            let mut vs = pipeline.validators.write();
            let v = vs.validators.get_mut(&offender).unwrap();
            v.self_stake = v.self_stake.saturating_sub(r1.final_slash_wei);
            // Status STAYS Pending (no Active→Jailed transition).
        }
        let post = pipeline.validators.read()
            .validators.get(&offender).unwrap().self_stake;
        assert_eq!(post, pre - r1.final_slash_wei);
        assert!(post > r1.final_slash_wei,
                "small slash: stake remains > slash amount (condition 1 misses)");
        let status_after = pipeline.validators.read()
            .validators.get(&offender).unwrap().status;
        assert_eq!(status_after, ValidatorStatus::Pending,
                   "status stayed non-Active (condition 2 misses)");
        // Replay must still skip — condition 3 catches non-Active.
        let applied = pipeline.tick_finalize(
            crate::slashing_v2::APPEAL_WINDOW_BLOCKS + 20).unwrap();
        assert!(applied.is_empty(),
                "must not double-burn a non-Active offender");
        assert_eq!(pipeline.validators.read()
                    .validators.get(&offender).unwrap().self_stake,
                   post,
                   "stake unchanged after replay-skip");
        let r_final = pipeline.store().get_record(&id).unwrap().unwrap();
        assert!(r_final.burn_applied,
                "skip path must stamp burn_applied=true");
    }

    /// Regression for the post-burn-persist-failure crash window
    /// (architect-review #3 hardening). Scenario:
    ///   1. finalize_slash flips Pending → Confirmed (in-memory).
    ///   2. put_record(Confirmed, burn_applied=false) succeeds.
    ///   3. apply_slash_burn_v2 runs — validator stake debited +
    ///      status → Jailed.
    ///   4. **CRASH** before set_burn_applied + 2nd put_record.
    /// On restart, the replay loop must NOT re-burn the validator.
    /// The status-based OR-condition in the replay guard catches it.
    #[test]
    fn replay_does_not_double_burn_when_post_burn_persist_failed() {
        let (_tmp, pipeline, sk, offender) = fresh_pipeline();
        let ev = mk_evidence(&sk, offender, H256([91; 32]), H256([92; 32]));
        let id = pipeline.ingest_equivocation(
            &ev, Address([0xbb; 20]), 1, 0, STAKE).unwrap();

        // Step 1+2: drive finalize_slash directly and persist the
        // Confirmed+!burn_applied record.
        {
            let mut reg = pipeline.registry().lock();
            reg.finalize_slash(id,
                crate::slashing_v2::APPEAL_WINDOW_BLOCKS + 10).unwrap();
        }
        let r1 = pipeline.registry().lock().get_record(&id).cloned().unwrap();
        assert_eq!(r1.status, EvidenceStatus::Confirmed);
        assert!(!r1.burn_applied);
        pipeline.store().put_record(&r1).unwrap();

        // Step 3: apply the burn directly to validator state (mimics
        // apply_slash_burn_v2 running). We do NOT update the
        // burn_applied flag or re-persist — simulating the crash.
        let pre_stake = pipeline.validators.read()
            .validators.get(&offender).unwrap().self_stake;
        {
            let mut vs = pipeline.validators.write();
            let v = vs.validators.get_mut(&offender).unwrap();
            v.self_stake = v.self_stake.saturating_sub(r1.final_slash_wei);
            v.status = ValidatorStatus::Jailed;
        }
        let post_burn_stake = pipeline.validators.read()
            .validators.get(&offender).unwrap().self_stake;
        assert_eq!(post_burn_stake, pre_stake - r1.final_slash_wei);
        // Sanity: the disk record still says jailed_by_slash=false and
        // burn_applied=false — i.e. the post-burn write was lost.
        assert!(!r1.jailed_by_slash);
        assert!(!r1.burn_applied);

        // Step 4: restart simulation — next tick. Replay MUST detect
        // the burn already ran (status==Jailed) and skip.
        let applied = pipeline.tick_finalize(
            crate::slashing_v2::APPEAL_WINDOW_BLOCKS + 20).unwrap();
        assert!(applied.is_empty(), "must NOT double-burn on replay");
        let final_stake = pipeline.validators.read()
            .validators.get(&offender).unwrap().self_stake;
        assert_eq!(final_stake, post_burn_stake,
                   "stake unchanged — no double-burn");

        // And the replay loop closed the cycle: record should now be
        // burn_applied=true so it never re-scans.
        let r_final = pipeline.store().get_record(&id).unwrap().unwrap();
        assert!(r_final.burn_applied,
                "replay loop must stamp burn_applied=true after the skip");
    }

    /// Forward-compat path: if a record IS already burned
    /// (`burn_applied=true`) and is then somehow appealed (delayed-
    /// appeal flow, not currently exposed), overturn DOES refund
    /// stake + un-jail. Exercises the gated branch.
    #[test]
    fn overturn_refunds_stake_when_burn_was_applied() {
        use crate::tx_handler::dispatch_file_appeal_tx;
        use zbx_types::staking_tx::APPEAL_BOND_WEI;

        let (_tmp, pipeline, sk, offender) = fresh_pipeline();
        let ev = mk_evidence(&sk, offender, H256([55; 32]), H256([66; 32]));
        let id = pipeline.ingest_equivocation(
            &ev, Address([0xbb; 20]), 1, 0, STAKE).unwrap();
        // Run the burn first.
        let applied = pipeline.tick_finalize(
            crate::slashing_v2::APPEAL_WINDOW_BLOCKS + 10).unwrap();
        assert_eq!(applied.len(), 1);
        assert!(applied[0].jailed, "first slash → jailed");
        let stake_after_burn = pipeline.validators.read()
            .validators.get(&offender).unwrap().self_stake;
        assert!(stake_after_burn < STAKE, "burn debited stake");

        // Manually flip the registry back to Appealed (simulates a
        // future delayed-appeal flow). `file_appeal_for_tx` would
        // refuse because status is Confirmed; we patch the in-memory
        // record + on-disk record to model the scenario.
        {
            let mut reg = pipeline.registry().lock();
            if let Some(r) = reg.records_mut().get_mut(&id) {
                r.status = EvidenceStatus::Appealed;
            }
        }
        let appealed_record = pipeline.registry().lock()
            .get_record(&id).cloned().unwrap();
        pipeline.store().put_record(&appealed_record).unwrap();
        // Add an appeal bond as if the offender had filed.
        pipeline.store().put_bond(&id, &offender, &BondEntry {
            wei:  APPEAL_BOND_WEI,
            kind: BondKind::Appeal,
        }).unwrap();

        let out = pipeline.overturn_and_refund(id).unwrap();
        assert_eq!(out.refunded_wei, applied[0].burn_wei,
                   "burn_applied=true → full slash refunded");
        assert!(out.unjailed, "burn-jailed validator un-jailed on overturn");
        assert_eq!(out.appeal_bond_refunded_wei, APPEAL_BOND_WEI);

        let vs = pipeline.validators.read();
        let v = vs.validators.get(&offender).unwrap();
        assert_eq!(v.self_stake, stake_after_burn + out.refunded_wei,
                   "stake restored to pre-burn value");
        assert_eq!(v.status, ValidatorStatus::Pending,
                   "un-jailed to Pending");
    }

    /// Two distinct lifetime slashes against the same offender:
    /// second slash promotes Jailed → Tombstoned. Once tombstoned the
    /// validator never re-enters the active set even after operator
    /// status reset.
    #[test]
    fn second_slash_promotes_to_tombstoned() {
        let (_tmp, pipeline, sk, offender) = fresh_pipeline();
        // First evidence + finalize.
        let ev1 = mk_evidence(&sk, offender, H256([10; 32]), H256([11; 32]));
        let _id1 = pipeline.ingest_equivocation(
            &ev1, Address([0xbb; 20]), 1, 0, STAKE).unwrap();
        let applied1 = pipeline.tick_finalize(
            crate::slashing_v2::APPEAL_WINDOW_BLOCKS + 10).unwrap();
        assert_eq!(applied1.len(), 1);
        assert!(applied1[0].jailed, "first slash → jailed");
        assert!(!applied1[0].tombstoned, "first slash does NOT tombstone");

        // Second evidence (different vote hash pair). Ingest at a later
        // current_block so the record id is distinct.
        let ev2 = mk_evidence(&sk, offender, H256([20; 32]), H256([21; 32]));
        let _id2 = pipeline.ingest_equivocation(
            &ev2, Address([0xbb; 20]),
            crate::slashing_v2::APPEAL_WINDOW_BLOCKS + 50,
            0, STAKE).unwrap();
        let applied2 = pipeline.tick_finalize(
            crate::slashing_v2::APPEAL_WINDOW_BLOCKS * 2 + 100).unwrap();
        assert_eq!(applied2.len(), 1);
        assert!(applied2[0].tombstoned,
                "2nd lifetime slash MUST tombstone");

        // Validator status is permanent Tombstoned.
        {
            let vs = pipeline.validators.read();
            let v = vs.validators.get(&offender).unwrap();
            assert_eq!(v.status, ValidatorStatus::Tombstoned);
        }

        // Operator status reset attempt: tombstoned validator must not
        // be re-electable. We simulate the operator setting status →
        // Active manually, then call elect_active_set and confirm the
        // tombstoned validator is filtered out by `is_eligible`.
        // Tombstoned is so permanent that even a direct status flip
        // by the operator would be rejected on the next election —
        // here we just assert the eligibility predicate.
        let vs = pipeline.validators.read();
        let v = vs.validators.get(&offender).unwrap();
        // `is_eligible` returns false for Tombstoned regardless of
        // self_stake (which after 2 slashes is also depleted).
        assert!(!v.is_eligible(),
                "tombstoned validator is never eligible");
    }

    /// Whistleblower bonds persist across process restart — a crash
    /// after `ingest_equivocation` but before finalize/overturn must
    /// not lose the bond ledger.
    #[test]
    fn whistleblower_bonds_persist_across_restart() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().to_owned();
        let offender = Address([0xcc; 20]);
        let whistleblower = Address([0xbb; 20]);
        let sk = BlsPrivKey::from_bytes(&[99u8; 32]).unwrap();

        // Phase A: ingest, then drop everything.
        let id = {
            let db = Arc::new(ZbxDb::open(&path).unwrap());
            let store = EvidenceStore::new(db);
            let registry = Arc::new(Mutex::new(SlashingRegistryV2::new(10)));
            let mut vs = ValidatorSet::new();
            vs.validators.insert(offender, Validator {
                address: offender, bls_pubkey: sk.to_pubkey(),
                self_stake: STAKE, delegated_stake: 0,
                commission_bps: 500, status: ValidatorStatus::Active,
                last_signed_block: 0, pending_rewards: 0,
                delegator_reward_pool: 0, pool_denominator: 0,
                registered_epoch: 0,
            });
            vs.active_set = vec![offender];
            let pipeline = SlashingPipeline::new(
                store, registry, Arc::new(RwLock::new(vs)));
            let ev = mk_evidence(&sk, offender, H256([30; 32]), H256([31; 32]));
            pipeline.ingest_equivocation(&ev, whistleblower, 1, 0, STAKE).unwrap()
        };

        // Phase B: fresh DB handle, same on-disk path. Bond ledger
        // must still hold the whistleblower entry.
        let db2 = Arc::new(ZbxDb::open(&path).unwrap());
        let store2 = EvidenceStore::new(db2);
        let bonds = store2.list_bonds_for_record(&id).unwrap();
        assert_eq!(bonds.len(), 1, "bond entry must survive restart");
        let (reporter, entry) = &bonds[0];
        assert_eq!(reporter, &whistleblower);
        assert_eq!(entry.kind, BondKind::Whistleblower);
    }
}
