//! Enhanced validator slashing v2 (ZEP-023).
//!
//! Upgrades over v1:
//! - On-chain evidence storage with unique IDs
//! - Optimistic slashing with 10-day appeal window
//! - Correlated slashing: slash % scales with how many validators misbehave
//! - Whistleblower rewards: 5% of slashed amount to evidence submitter
//! - Invalid block proofs: new evidence type

use crate::{
    error::StakingError,
    SLASH_DOUBLE_SIGN, SLASH_LIVENESS_DAILY,
};
use zbx_types::{address::Address, H256};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sha3::{Digest, Sha3_256};
use tracing::{info, warn};

/// Per-record slash finalization result returned by
/// `SlashingRegistryV2::finalize_slash`. Carries the total burn plus
/// the per-reporter reward split (sums to `total_reward`, modulo
/// integer-division remainder ≤ N-1 wei dropped at the boundary).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizedSlash {
    pub slash_wei:    u128,
    pub total_reward: u128,
    pub splits:       Vec<(Address, u128)>,
}

/// Appeal window in blocks (~10 days at 5s/block).
pub const APPEAL_WINDOW_BLOCKS: u64 = 172_800;
/// Whistleblower reward: 5% of slashed amount (in basis points).
pub const WHISTLEBLOWER_REWARD_BPS: u128 = 500;
/// Evidence bond required to submit (prevents spam): 100 ZBX in wei.
pub const EVIDENCE_BOND_WEI: u128 = 100 * 10u128.pow(18);
/// Correlated slashing multiplier base (in basis points).
pub const CORRELATED_BASE_BPS: u128 = 10_000; // 100%

/// Type of slashable offence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvidenceType {
    DoubleSign,
    LivenessFault,
    ConsecutiveMiss,
    InvalidBlock,
    SurrogateVote,
}

/// Status of a slash evidence record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvidenceStatus {
    Pending,
    Appealed,
    Confirmed,
    Rejected,
    Overturned,
}

/// Proof that a validator double-signed two conflicting blocks at the same round and phase.
///
/// Equivocation is detected when the same validator signs two different blocks at
/// the same (height, round, phase). Evidence is valid only if both BLS signatures
/// cryptographically verify against the validator's registered public key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoubleSignProof {
    pub height:    u64,
    pub round:     u64,
    pub phase:     u8,
    pub block_a:   H256,
    pub block_b:   H256,
    /// BLS12-381 signature (96 bytes) over block_a from the offending validator.
    pub sig_a:     Vec<u8>,
    /// BLS12-381 signature (96 bytes) over block_b from the offending validator.
    pub sig_b:     Vec<u8>,
    pub validator: Address,
    /// Registered BLS12-381 G1 public key of the validator (48 bytes).
    ///
    /// Must match the on-chain validator registry entry for `validator`.
    /// Slashing is rejected if this key is not registered for the address.
    pub validator_bls_pubkey: Vec<u8>,
}

impl DoubleSignProof {
    /// Verify the double-sign proof using real BLS12-381 pairing checks.
    ///
    /// Accepts the proof only if ALL of the following hold:
    /// 1. block_a ≠ block_b (otherwise it's not equivocation)
    /// 2. Both sig_a and sig_b are 96-byte valid G2 points
    /// 3. validator_bls_pubkey is a 48-byte valid G1 point
    /// 4. `verify_single(sig_a, pk, block_a)` passes — e(g₁, σ_a) == e(pk, H(block_a))
    /// 5. `verify_single(sig_b, pk, block_b)` passes — e(g₁, σ_b) == e(pk, H(block_b))
    pub fn verify(&self) -> bool {
        use zbx_crypto::bls::{BlsPubKey, BlsSignature, verify_single};

        // Blocks must differ — same block hash is not equivocation.
        if self.block_a == self.block_b {
            return false;
        }

        // Parse the validator's BLS public key (48-byte G1 point).
        let pk = match BlsPubKey::from_bytes(&self.validator_bls_pubkey) {
            Ok(p)  => p,
            Err(_) => return false,
        };

        // Both signatures must be exactly 96 bytes (compressed G2 point).
        if self.sig_a.len() != 96 || self.sig_b.len() != 96 {
            return false;
        }

        // Parse signature over block_a.
        let sig_a = match BlsSignature::from_bytes(&self.sig_a) {
            Ok(s)  => s,
            Err(_) => return false,
        };

        // Parse signature over block_b.
        let sig_b = match BlsSignature::from_bytes(&self.sig_b) {
            Ok(s)  => s,
            Err(_) => return false,
        };

        // Both BLS pairing checks: e(g₁, σ) == e(pk, H(block_hash)).
        // The message signed by validators is the raw block hash (H256).
        verify_single(&sig_a, &pk, &self.block_a)
            && verify_single(&sig_b, &pk, &self.block_b)
    }
}

/// Evidence of an invalid block proposed by a validator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvalidBlockProof {
    pub block_hash: H256,
    pub proposer:   Address,
    pub violation:  BlockViolation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BlockViolation {
    InvalidStateRoot { claimed: H256, actual: H256 },
    InvalidTxRoot,
    GasLimitExceeded { claimed: u64, max: u64 },
    InvalidTimestamp { claimed: u64, expected_min: u64 },
    ChainIdMismatch  { claimed: u64, expected: u64 },
}

/// All supported evidence types.
///
/// ## M-02 fix (ZBX-M-02): SurrogateVote variant added
///
/// A `SurrogateVote` occurs when validator A submits a vote (bearing its own
/// signature) on behalf of validator B — i.e., A signs vote data that should
/// only be signed by B. This was slashable at the `EvidenceType` / `base_slash_bps`
/// level but the corresponding `SlashEvidenceV2` variant was missing, making it
/// impossible to actually submit or process such evidence.
///
/// Fields:
/// * `vote_hash`  — hash of the fraudulent vote message (content-hash ID)
/// * `block_a`    — the block the surrogate vote was cast on
/// * `block_b`    — the block the legitimate validator was supposed to vote on
///                  (evidence of a fork-attempt; may equal `block_a`)
/// * `validator`  — the offending validator address (the surrogate)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SlashEvidenceV2 {
    DoubleSign(DoubleSignProof),
    LivenessFault { epoch: u64, missed: u64, total: u64 },
    ConsecutiveMiss { from_block: u64, count: u64 },
    InvalidBlock(InvalidBlockProof),
    /// M-02 fix: surrogate-vote evidence now part of the slashable evidence enum.
    SurrogateVote {
        vote_hash: H256,
        block_a:   H256,
        block_b:   H256,
        validator: Address,
    },
}

impl SlashEvidenceV2 {
    pub fn evidence_type(&self) -> EvidenceType {
        match self {
            SlashEvidenceV2::DoubleSign(_)            => EvidenceType::DoubleSign,
            SlashEvidenceV2::LivenessFault { .. }     => EvidenceType::LivenessFault,
            SlashEvidenceV2::ConsecutiveMiss { .. }   => EvidenceType::ConsecutiveMiss,
            SlashEvidenceV2::InvalidBlock(_)           => EvidenceType::InvalidBlock,
            SlashEvidenceV2::SurrogateVote { .. }     => EvidenceType::SurrogateVote,
        }
    }

    pub fn offender(&self) -> Option<Address> {
        match self {
            SlashEvidenceV2::DoubleSign(p)              => Some(p.validator),
            SlashEvidenceV2::InvalidBlock(p)             => Some(p.proposer),
            SlashEvidenceV2::SurrogateVote { validator, .. } => Some(*validator),
            _                                            => None,
        }
    }
}

/// An on-chain slash evidence record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlashEvidenceRecord {
    /// Unique ID = keccak256(serialized evidence)
    pub id:              H256,
    pub evidence_type:   EvidenceType,
    pub offender:        Address,
    /// First reporter (kept for backward compat & legacy single-witness paths).
    /// Co-witness reporters are appended to `reporters` instead.
    pub submitted_by:    Address,
    pub submit_block:    u64,
    pub evidence:        SlashEvidenceV2,
    pub status:          EvidenceStatus,
    /// Base slash amount (before correlation multiplier) in ZBX wei
    pub base_slash_wei:  u128,
    /// Slash after correlated multiplier
    pub final_slash_wei: u128,
    pub appeal_deadline: u64,
    /// **Slashing-upgrade**: full list of co-witness reporters of the
    /// SAME equivocation. The first entry is always `submitted_by`;
    /// each subsequent honest reporter that re-submits the same
    /// evidence is appended here (no duplicates). The whistleblower
    /// reward is split **equally** across all entries at finalization
    /// so the second honest reporter is no longer dropped silently.
    ///
    /// `#[serde(default)]` keeps backward compatibility with pre-
    /// upgrade serialised records (they deserialize with an empty
    /// vec, and the pipeline falls back to `submitted_by` alone).
    #[serde(default)]
    pub reporters:       Vec<Address>,
    /// **Slashing-upgrade**: tracks whether the slashing pipeline was
    /// the cause of the offender's `Jailed` status. Set true when
    /// `apply_slash_burn` transitions the validator from `Active` to
    /// `Jailed` for THIS record. Used by `overturn_and_refund` to
    /// know whether to un-jail on overturn (we must not un-jail a
    /// validator who was jailed for an *independent* reason —
    /// liveness fault, manual operator jail, etc.).
    #[serde(default)]
    pub jailed_by_slash: bool,
    /// **Slashing-upgrade** (crash-consistency follow-up): two-phase
    /// finalize marker. `finalize_slash` flips status → Confirmed
    /// with `burn_applied=false`; the pipeline only sets this `true`
    /// AFTER the validator-set burn + reward-credit pass succeeds
    /// and persists the record a second time. On node restart the
    /// pipeline scans for `Confirmed && !burn_applied` and replays
    /// the burn — splits are recomputed deterministically from
    /// `submitted_by + reporters + final_slash_wei`, so the replay
    /// produces byte-identical results to the original pass.
    ///
    /// `#[serde(default)]` (= `false`) is backward-compatible with
    /// pre-upgrade Confirmed records on disk. Those records belong
    /// to a prior process that already applied the burn (legacy code
    /// did burn-then-persist), so to avoid re-burning legacy stake
    /// the replay path additionally gates on
    /// `EvidenceStore::was_confirmed_pre_upgrade()` — see
    /// `SlashingPipeline::tick_finalize` for the full rule.
    #[serde(default)]
    pub burn_applied: bool,
    /// **Slashing-upgrade — replay-safety discriminator.**
    ///
    /// `0` = pre-upgrade record written by the legacy burn-then-persist
    /// path. Those records ALREADY had their burn applied before they
    /// were persisted, so the two-phase replay loop must NOT touch
    /// them (even though `burn_applied` deserializes false by default).
    ///
    /// `1` = upgraded record written by the new persist-Confirmed-then-
    /// burn-then-re-persist path. Only `version == 1` records are
    /// eligible for the crash-recovery replay; the replay loop filters
    /// strictly on `version >= 1 && !burn_applied`.
    ///
    /// This is a stronger guard than the `self_stake >= final_slash_wei`
    /// heuristic (which fails for legacy 5%-slashed validators whose
    /// remaining stake still exceeds the slash amount).
    ///
    /// `#[serde(default)]` (= `0`) is what makes pre-upgrade records
    /// safe by construction: they are explicitly excluded from replay.
    #[serde(default)]
    pub format_version: u8,
}

/// Current on-disk format version for SlashEvidenceRecord. Bumped
/// whenever a new field is added that the crash-recovery replay
/// loop depends on for correctness.
pub const SLASH_RECORD_FORMAT_VERSION: u8 = 1;

impl SlashEvidenceRecord {
    fn compute_id(evidence: &SlashEvidenceV2, offender: &Address, submit_block: u64) -> H256 {
        Self::compute_id_for_offender(offender, submit_block, evidence.evidence_type())
    }

    /// SEC-2026-05-09 Pass-11 — public ID computation for the
    /// slashing pipeline's idempotent re-detection path. The
    /// pipeline needs to recover the canonical ID for a duplicate
    /// submission *without* re-running `submit_evidence` (the
    /// registry returned `DuplicateEvidence`). Mirrors the private
    /// `compute_id` exactly so the two paths cannot drift.
    pub fn compute_id_for_offender(
        offender:      &Address,
        submit_block:   u64,
        evidence_type:  EvidenceType,
    ) -> H256 {
        // Use stable u8 discriminants instead of Debug format strings — prevents
        // ID drift if enum variant names are ever renamed (ZBX-M-03 fix).
        let type_discriminant: u8 = match evidence_type {
            EvidenceType::DoubleSign      => 0,
            EvidenceType::LivenessFault   => 1,
            EvidenceType::ConsecutiveMiss => 2,
            EvidenceType::InvalidBlock    => 3,
            EvidenceType::SurrogateVote   => 4,
        };
        let mut h = Sha3_256::new();
        h.update(&offender.0);
        h.update(submit_block.to_le_bytes());
        h.update([type_discriminant]);
        let bytes = h.finalize();
        let mut id = [0u8; 32];
        id.copy_from_slice(&bytes);
        H256(id)
    }
}

/// Base slash amounts in basis points (100 bps = 1%).
pub fn base_slash_bps(evidence_type: &EvidenceType) -> u128 {
    match evidence_type {
        EvidenceType::DoubleSign      => SLASH_DOUBLE_SIGN,        // 500 bps = 5%
        EvidenceType::LivenessFault   => SLASH_LIVENESS_DAILY,     // 1 bps/day
        EvidenceType::ConsecutiveMiss => 100,                       // 1%
        EvidenceType::InvalidBlock    => 2_000,                     // 20%
        EvidenceType::SurrogateVote   => 500,                       // 5%
    }
}

/// Calculate correlated slash: scales with fraction of validators misbehaving.
///
/// Formula: `base_slash × (1 + 3 × (N_slashed / N_total))²`
///
/// At 33% validators misbehaving: ~3.7× base slash.
/// At 67%+ validators misbehaving: capped at 100%.
pub fn correlated_slash_bps(
    base_bps: u128,
    n_slashed_this_epoch: u64,
    n_total_validators: u64,
) -> u128 {
    if n_total_validators == 0 { return base_bps; }

    // Scale factor: (1 + 3 * ratio)^2 in fixed-point (×10000)
    let ratio_bps = (n_slashed_this_epoch as u128 * 10_000) / n_total_validators as u128;
    let factor_fp = 10_000 + 3 * ratio_bps; // (1 + 3*ratio) × 10000
    let factor_sq = factor_fp * factor_fp / 10_000; // squared

    let slashed = base_bps * factor_sq / 10_000;
    slashed.min(10_000) // cap at 100%
}

/// Calculate final slash amount in wei given validator's stake.
///
/// STK-SLASH-01: uses `checked_mul` to prevent u128 overflow for adversarial
/// or test inputs where `stake_wei × slash_bps` could exceed `u128::MAX`.
/// For realistic PoS values (total supply ≤ 10^27 wei, `slash_bps` ≤ 10 000)
/// the product stays well within range, so the fallback path is only hit in
/// pathological scenarios.  The fallback avoids truncation by computing
/// `(stake_wei / 10_000) × slash_bps` — integer division before multiply
/// loses at most `(10_000 - 1)` wei of precision, which is acceptable for a
/// slash that is itself approximate (commission rounding, etc.).
pub fn slash_amount_wei(
    stake_wei: u128,
    slash_bps: u128,
) -> u128 {
    stake_wei
        .checked_mul(slash_bps)
        .map(|p| p / 10_000)
        .unwrap_or_else(|| (stake_wei / 10_000).saturating_mul(slash_bps))
}

/// Outcome of a successful `submit_evidence` call.
///
/// Distinguishes a brand-new submission from a co-witness merge so
/// the pipeline can persist the right bonds and tell callers whether
/// a new whistleblower joined an existing record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitOutcome {
    /// First submission of this evidence — `record_id` is fresh.
    NewRecord(H256),
    /// Same evidence already submitted by another reporter — the
    /// caller has been added to `reporters` and will share the
    /// whistleblower reward at finalization. Returns the existing id.
    CoWitnessAdded(H256),
    /// Same `(offender, submit_block, evidence_type)` AND the caller
    /// was already in `reporters`. Pure no-op idempotent re-detection.
    AlreadyRecorded(H256),
}

impl SubmitOutcome {
    pub fn record_id(&self) -> H256 {
        match self {
            SubmitOutcome::NewRecord(id)
            | SubmitOutcome::CoWitnessAdded(id)
            | SubmitOutcome::AlreadyRecorded(id) => *id,
        }
    }
}

/// The on-chain slashing registry (ZEP-023).
///
/// Bond persistence is handled by the `SlashingPipeline` layer, which calls
/// `EvidenceStore::put_bond` / `get_bond` backed by the `SlashingBonds` RocksDB
/// column family. The legacy in-memory `pending_bonds` mirror has been removed
/// (M-2 fix, 2026-06-27) — bond state is durable across restarts via RocksDB.
pub struct SlashingRegistryV2 {
    /// All evidence records by ID
    records: HashMap<H256, SlashEvidenceRecord>,
    /// Slashes confirmed this epoch per validator
    epoch_slash_count: HashMap<(u64, Address), u64>,
    total_validators: u64,
}

impl SlashingRegistryV2 {
    pub fn new(total_validators: u64) -> Self {
        SlashingRegistryV2 {
            records:           HashMap::new(),
            epoch_slash_count: HashMap::new(),
            total_validators,
        }
    }

    /// Submit new slash evidence — with co-witness support.
    ///
    /// Returns a `SubmitOutcome` that distinguishes:
    ///   * `NewRecord(id)`        — first time this evidence is seen.
    ///   * `CoWitnessAdded(id)`   — duplicate evidence from a NEW reporter;
    ///     the caller has been appended to the record's `reporters` vec
    ///     and will share the whistleblower reward at finalization.
    ///   * `AlreadyRecorded(id)`  — duplicate evidence from the SAME
    ///     reporter; idempotent no-op.
    ///
    /// **Slashing-upgrade**: pre-upgrade this returned
    /// `Err(DuplicateEvidence)` whenever the (offender, block, type)
    /// id collided, silently dropping the second honest reporter and
    /// keeping their bond locked with no reward share. That behaviour
    /// is gone — the second reporter is now first-class.
    pub fn submit_evidence(
        &mut self,
        evidence:        SlashEvidenceV2,
        submitted_by:    Address,
        current_block:   u64,
        current_epoch:   u64,
        offender_stake:  u128,
    ) -> Result<SubmitOutcome, StakingError> {
        let offender = evidence.offender()
            .ok_or_else(|| StakingError::InvalidEvidence("cannot determine offender".into()))?;

        let evidence_type = evidence.evidence_type();
        let id = SlashEvidenceRecord::compute_id(&evidence, &offender, current_block);

        // Co-witness path: same evidence id already exists.
        if let Some(existing) = self.records.get_mut(&id) {
            // Only allow co-witness while the record is still Pending —
            // once it transitions to Confirmed/Overturned/Rejected the
            // reward split is fixed and adding reporters would break
            // accounting.
            if existing.status != EvidenceStatus::Pending {
                return Ok(SubmitOutcome::AlreadyRecorded(id));
            }
            // Idempotent on (record_id, reporter): same submitter
            // re-detecting their own evidence is a no-op.
            let already_listed = existing.submitted_by == submitted_by
                || existing.reporters.iter().any(|r| *r == submitted_by);
            if already_listed {
                return Ok(SubmitOutcome::AlreadyRecorded(id));
            }
            existing.reporters.push(submitted_by);
            // Bond for co-witness is persisted by SlashingPipeline → EvidenceStore.
            info!(evidence_id = ?id, co_witness = ?submitted_by,
                  total_reporters = existing.reporters.len() + 1,
                  "co-witness reporter added — will share whistleblower reward");
            return Ok(SubmitOutcome::CoWitnessAdded(id));
        }

        // Calculate slash with correlation
        let n_slashed = self.epoch_slash_count
            .get(&(current_epoch, offender))
            .copied()
            .unwrap_or(0) + 1;
        let base_bps   = base_slash_bps(&evidence_type);
        let corr_bps   = correlated_slash_bps(base_bps, n_slashed, self.total_validators);
        let slash_wei  = slash_amount_wei(offender_stake, corr_bps);

        let record = SlashEvidenceRecord {
            id,
            evidence_type,
            offender,
            submitted_by,
            submit_block:    current_block,
            evidence,
            status:          EvidenceStatus::Pending,
            base_slash_wei:  slash_amount_wei(offender_stake, base_bps),
            final_slash_wei: slash_wei,
            appeal_deadline: current_block + APPEAL_WINDOW_BLOCKS,
            // Co-witness list starts empty — `submitted_by` is the
            // canonical "first reporter" and is added to the reward
            // split implicitly at finalization time.
            reporters: Vec::new(),
            jailed_by_slash: false,
            burn_applied:    false,
            format_version:  SLASH_RECORD_FORMAT_VERSION,
        };

        // Bond for this evidence is persisted by SlashingPipeline → EvidenceStore.
        self.records.insert(id, record);

        info!(
            evidence_id = ?id,
            %offender,
            slash_bps = corr_bps,
            slash_wei,
            "Slash evidence submitted"
        );

        Ok(SubmitOutcome::NewRecord(id))
    }

    /// Mutable accessor for the pipeline's `jailed_by_slash` write-back
    /// after `apply_slash_burn` runs. Kept narrow on purpose — the
    /// registry should not expose unrestricted record mutation.
    /// Test-only accessor: mutable view into the raw records map.
    /// Used by `pipeline::tests` to model forward-compat scenarios
    /// (e.g. flipping a Confirmed record back to Appealed to exercise
    /// the burn-was-applied refund branch). Not exposed to
    /// production code — production status transitions must go
    /// through `file_appeal` / `finalize_slash` / `overturn_slash` /
    /// `reject_appeal` to keep the state-machine invariants intact.
    #[cfg(test)]
    pub fn records_mut(&mut self) -> &mut std::collections::HashMap<H256, SlashEvidenceRecord> {
        &mut self.records
    }

    pub fn set_jailed_by_slash(&mut self, id: &H256, jailed: bool) {
        if let Some(r) = self.records.get_mut(id) {
            r.jailed_by_slash = jailed;
        }
    }

    /// File an appeal against a slash record (by the offending validator).
    pub fn file_appeal(
        &mut self,
        evidence_id: H256,
        current_block: u64,
    ) -> Result<(), StakingError> {
        let record = self.records.get_mut(&evidence_id)
            .ok_or(StakingError::EvidenceNotFound)?;

        if record.status != EvidenceStatus::Pending {
            return Err(StakingError::AppealNotAllowed);
        }
        if current_block > record.appeal_deadline {
            return Err(StakingError::AppealWindowExpired);
        }

        record.status = EvidenceStatus::Appealed;
        info!(evidence_id = ?evidence_id, "Appeal filed");
        Ok(())
    }

    /// Outcome of `finalize_slash`: slash amount + a list of
    /// `(reporter, reward_share_wei)` pairs split equally across all
    /// co-witnesses (first reporter + every entry in `reporters`).
    /// Returns `None` if the record is not yet finalizable.
    pub fn finalize_slash(
        &mut self,
        evidence_id: H256,
        current_block: u64,
    ) -> Result<Option<FinalizedSlash>, StakingError> {
        let record = self.records.get_mut(&evidence_id)
            .ok_or(StakingError::EvidenceNotFound)?;

        if record.status != EvidenceStatus::Pending {
            return Ok(None); // Already appealed or finalized
        }
        if current_block <= record.appeal_deadline {
            return Ok(None); // Appeal window still open
        }

        record.status = EvidenceStatus::Confirmed;
        // **Upgrade-boundary normalization** (architect-review #3):
        //
        // Stamp the current format version onto the record at the
        // moment it enters the two-phase finalize flow. This handles
        // the legacy-Pending-record-finalized-post-upgrade case: such
        // a record deserialized with `format_version=0` (serde
        // default), but as soon as `finalize_slash` runs on it the
        // record is on the NEW persist-Confirmed-then-burn path and
        // MUST be eligible for the crash-recovery replay loop if a
        // crash happens between the Confirmed-persist and the burn.
        // Without this stamp, such a record would be permanently
        // excluded from replay (legacy-version filter) AND from
        // re-finalization (already-Confirmed filter) — a permanent
        // skipped burn.
        record.format_version = SLASH_RECORD_FORMAT_VERSION;
        // Also stamp burn_applied=false explicitly (it's already
        // false by serde default for legacy records, but being
        // explicit here documents the two-phase contract: persist
        // this Confirmed-but-unapplied record → burn → re-persist
        // with burn_applied=true.)
        record.burn_applied = false;
        let slash       = record.final_slash_wei;
        let total_reward = slash * WHISTLEBLOWER_REWARD_BPS / 10_000;

        // Build full reporter list: first reporter + co-witnesses,
        // de-duplicated (defence-in-depth — `submit_evidence` already
        // dedups, but on-disk legacy records may not have).
        let mut all_reporters: Vec<Address> =
            std::iter::once(record.submitted_by)
                .chain(record.reporters.iter().copied())
                .collect();
        all_reporters.sort_unstable();
        all_reporters.dedup();

        // Equal split — integer division, remainder dropped (≤ N-1 wei,
        // negligible at 18-decimal scale). Sort + dedup above means
        // the split is deterministic across nodes.
        let n = all_reporters.len() as u128;
        let per_reporter = if n == 0 { 0 } else { total_reward / n };
        let splits: Vec<(Address, u128)> = all_reporters
            .into_iter()
            .map(|r| (r, per_reporter))
            .collect();

        info!(
            evidence_id = ?evidence_id,
            offender = ?record.offender,
            slash_wei = slash,
            total_reward_wei = total_reward,
            per_reporter_wei = per_reporter,
            reporters = splits.len(),
            "Slash confirmed after appeal window"
        );

        Ok(Some(FinalizedSlash {
            slash_wei:    slash,
            total_reward: total_reward,
            splits,
        }))
    }

    /// Overturn an appealed slash. Caller is expected to refund the
    /// returned `slash_wei` to the offender (and unjail if
    /// `record.jailed_by_slash` is true). The registry only flips
    /// the status — `SlashingPipeline::overturn_and_refund` wires
    /// the actual ValidatorSet & bond ledger updates.
    pub fn overturn_slash(
        &mut self,
        evidence_id: H256,
    ) -> Result<u128, StakingError> {
        let record = self.records.get_mut(&evidence_id)
            .ok_or(StakingError::EvidenceNotFound)?;

        if record.status != EvidenceStatus::Appealed {
            return Err(StakingError::OverturnNotAllowed);
        }

        let slash_to_return = record.final_slash_wei;
        record.status = EvidenceStatus::Overturned;
        warn!(evidence_id = ?evidence_id, "Slash overturned — stake to be returned");
        Ok(slash_to_return)
    }

    /// Reject an appealed slash that governance decided is legitimate.
    /// Returns the slash amount which the caller MUST apply via the
    /// validator-set burn (the registry only flips status).
    pub fn reject_appeal(
        &mut self,
        evidence_id: H256,
    ) -> Result<u128, StakingError> {
        let record = self.records.get_mut(&evidence_id)
            .ok_or(StakingError::EvidenceNotFound)?;
        if record.status != EvidenceStatus::Appealed {
            return Err(StakingError::OverturnNotAllowed);
        }
        record.status = EvidenceStatus::Confirmed;
        let slash = record.final_slash_wei;
        warn!(evidence_id = ?evidence_id,
              "Appeal rejected — slash confirmed, appeal bond forfeit");
        Ok(slash)
    }

    pub fn get_record(&self, id: &H256) -> Option<&SlashEvidenceRecord> {
        self.records.get(id)
    }

    pub fn pending_count(&self) -> usize {
        self.records.values()
            .filter(|r| r.status == EvidenceStatus::Pending)
            .count()
    }

    /// Set the `burn_applied` marker on a Confirmed record. Called
    /// by `SlashingPipeline::tick_finalize` AFTER a successful
    /// validator-set burn + reward-credit pass, to close the
    /// two-phase finalize cycle. Idempotent.
    pub fn set_burn_applied(&mut self, id: &H256) {
        if let Some(r) = self.records.get_mut(id) {
            r.burn_applied = true;
        }
    }

    /// Recompute the (reporter, reward_share_wei) splits for a
    /// Confirmed record. Used by `tick_finalize`'s replay path so
    /// a crash between status-flip and burn can be recovered
    /// without re-running `finalize_slash` (which would refuse on a
    /// non-Pending record). Splits are derived deterministically
    /// from `(submitted_by, reporters, final_slash_wei)` — identical
    /// arithmetic to `finalize_slash`'s split block so replay is
    /// byte-equivalent to the original pass.
    pub fn recompute_splits(record: &SlashEvidenceRecord) -> FinalizedSlash {
        let slash        = record.final_slash_wei;
        let total_reward = slash * WHISTLEBLOWER_REWARD_BPS / 10_000;
        let mut all_reporters: Vec<Address> =
            std::iter::once(record.submitted_by)
                .chain(record.reporters.iter().copied())
                .collect();
        all_reporters.sort_unstable();
        all_reporters.dedup();
        let n = all_reporters.len() as u128;
        let per_reporter = if n == 0 { 0 } else { total_reward / n };
        let splits: Vec<(Address, u128)> = all_reporters
            .into_iter()
            .map(|r| (r, per_reporter))
            .collect();
        FinalizedSlash { slash_wei: slash, total_reward, splits }
    }

    /// Lifetime count of Confirmed slashes against `offender` —
    /// powers the tombstone-on-repeat trigger in
    /// `SlashingPipeline::tick_finalize`.
    pub fn lifetime_confirmed_slashes(&self, offender: &Address) -> u64 {
        self.records.values()
            .filter(|r| r.status == EvidenceStatus::Confirmed && &r.offender == offender)
            .count() as u64
    }

    /// File an appeal — programmatic accessor for the on-chain
    /// `FileAppeal` transaction handler. Wraps the same checks but
    /// returns the updated record so the caller can persist it.
    pub fn file_appeal_for_tx(
        &mut self,
        evidence_id: H256,
        sender: Address,
        current_block: u64,
    ) -> Result<SlashEvidenceRecord, StakingError> {
        let record = self.records.get(&evidence_id)
            .ok_or(StakingError::EvidenceNotFound)?;
        if record.offender != sender {
            return Err(StakingError::AppealNotByOffender);
        }
        self.file_appeal(evidence_id, current_block)?;
        Ok(self.records.get(&evidence_id).cloned().unwrap())
    }

    /// SEC-2026-05-09 Pass-11 — bypass-validation insert used ONLY by
    /// `SlashingPipeline::rehydrate_from_disk` at node startup.
    ///
    /// The on-disk record is treated as canonical — it has already
    /// been through `submit_evidence`'s validation path on a
    /// previous boot. Re-running `submit_evidence` here would
    /// (a) re-charge correlated-slash multipliers (double-counting),
    /// (b) re-set `appeal_deadline = current_block + APPEAL_WINDOW`
    /// effectively un-aging the record. Both are wrong. We restore
    /// the record verbatim so a node crash mid-window does not
    /// reset the slashing clock.
    ///
    /// Idempotent on `record.id` — duplicate rehydration is a no-op.
    pub fn insert_rehydrated_record(&mut self, record: SlashEvidenceRecord) {
        // Update epoch counter for correlated-slash math on any
        // *future* submissions in the same epoch (records loaded
        // here already have their `final_slash_wei` baked in).
        if record.status == EvidenceStatus::Confirmed
            || record.status == EvidenceStatus::Pending
            || record.status == EvidenceStatus::Appealed
        {
            // Conservative: only count slashes that aren't yet
            // overturned/rejected. We approximate the original
            // epoch as block / EPOCH_LENGTH (172_800). Off by an
            // epoch in pathological cases but never silently
            // under-slashes (correlated multiplier monotonically
            // increases in n_slashed).
            let approx_epoch = record.submit_block / crate::EPOCH_LENGTH;
            *self.epoch_slash_count
                .entry((approx_epoch, record.offender))
                .or_insert(0) += 1;
        }
        self.records.insert(record.id, record);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_double_sign(validator: Address) -> SlashEvidenceV2 {
        SlashEvidenceV2::DoubleSign(DoubleSignProof {
            height:    100,
            round:     1,
            phase:     0,
            block_a:   H256([1u8; 32]),
            block_b:   H256([2u8; 32]),
            sig_a:     vec![1u8; 96],
            sig_b:     vec![2u8; 96],
            validator,
            // Placeholder — registry tests exercise submission/finalization logic,
            // not BLS cryptographic verification.
            validator_bls_pubkey: vec![0u8; 48],
        })
    }

    #[test]
    fn submit_and_finalize() {
        let mut reg = SlashingRegistryV2::new(100);
        let offender  = Address([1u8; 20]);
        let submitter = Address([2u8; 20]);
        let stake     = 100_000 * 10u128.pow(18);

        let outcome = reg.submit_evidence(
            make_double_sign(offender),
            submitter, 1, 0, stake,
        ).unwrap();
        assert!(matches!(outcome, SubmitOutcome::NewRecord(_)));
        let id = outcome.record_id();

        // Cannot finalize during appeal window
        assert!(reg.finalize_slash(id, 100).unwrap().is_none());

        // Finalize after window
        let result = reg.finalize_slash(id, APPEAL_WINDOW_BLOCKS + 10).unwrap();
        let f = result.expect("finalize must produce a result past the window");
        assert!(f.slash_wei > 0);
        assert_eq!(f.total_reward, f.slash_wei * 500 / 10_000);
        assert_eq!(f.splits.len(), 1, "single-reporter slash → one split entry");
        assert_eq!(f.splits[0].0, submitter);
        assert_eq!(f.splits[0].1, f.total_reward);
    }

    #[test]
    fn co_witness_second_reporter_shares_reward() {
        let mut reg = SlashingRegistryV2::new(100);
        let offender   = Address([1u8; 20]);
        let reporter_a = Address([2u8; 20]);
        let reporter_b = Address([3u8; 20]);
        let stake      = 100_000 * 10u128.pow(18);

        let a = reg.submit_evidence(make_double_sign(offender), reporter_a, 1, 0, stake).unwrap();
        assert!(matches!(a, SubmitOutcome::NewRecord(_)));
        let id = a.record_id();

        // Second honest reporter of the SAME equivocation — pre-upgrade
        // this returned DuplicateEvidence and they got no reward.
        let b = reg.submit_evidence(make_double_sign(offender), reporter_b, 1, 0, stake).unwrap();
        assert_eq!(b, SubmitOutcome::CoWitnessAdded(id),
            "second honest reporter must be added as co-witness");

        // Same reporter re-submitting → AlreadyRecorded, not a third entry.
        let dup = reg.submit_evidence(make_double_sign(offender), reporter_a, 1, 0, stake).unwrap();
        assert_eq!(dup, SubmitOutcome::AlreadyRecorded(id));

        let f = reg.finalize_slash(id, APPEAL_WINDOW_BLOCKS + 10).unwrap().unwrap();
        assert_eq!(f.splits.len(), 2, "two reporters → two equal splits");
        let split_sum: u128 = f.splits.iter().map(|(_, w)| *w).sum();
        // Integer-division remainder ≤ 1 wei may be dropped.
        assert!(f.total_reward - split_sum <= 1,
            "splits must cover total reward modulo integer rounding");
        // Both reporters present in splits.
        let addrs: Vec<Address> = f.splits.iter().map(|(a, _)| *a).collect();
        assert!(addrs.contains(&reporter_a));
        assert!(addrs.contains(&reporter_b));
    }

    #[test]
    fn correlated_slash_scales() {
        let base = 500u128; // 5%
        let single  = correlated_slash_bps(base, 1,  100);
        let tenth   = correlated_slash_bps(base, 10, 100);
        let third   = correlated_slash_bps(base, 33, 100);

        assert!(single < tenth, "More slashes = higher %");
        assert!(tenth  < third, "More slashes = higher %");
        assert!(third  <= 10_000, "Capped at 100%");
    }

    #[test]
    fn double_sign_proof_bls_verification() {
        use zbx_crypto::bls::BlsPrivKey;

        let sk = BlsPrivKey::from_bytes(&[42u8; 32]).unwrap();
        let pk = sk.to_pubkey();

        let block_a = H256([1u8; 32]);
        let block_b = H256([2u8; 32]);
        let sig_a   = sk.sign(&block_a);
        let sig_b   = sk.sign(&block_b);

        let proof = DoubleSignProof {
            height: 1, round: 0, phase: 0,
            block_a,
            block_b,
            sig_a: sig_a.as_bytes().to_vec(),
            sig_b: sig_b.as_bytes().to_vec(),
            validator:            Address([1u8; 20]),
            validator_bls_pubkey: pk.as_bytes().to_vec(),
        };

        // Real BLS pairing check must pass for a valid equivocation proof.
        assert!(proof.verify(), "valid BLS double-sign proof must verify");

        // Same block is not equivocation — must be rejected.
        let mut same = proof.clone();
        same.block_b = block_a;
        assert!(!same.verify(), "same-block proof must be rejected");
    }

    #[test]
    fn double_sign_proof_rejects_wrong_sig() {
        use zbx_crypto::bls::BlsPrivKey;

        let sk1 = BlsPrivKey::from_bytes(&[11u8; 32]).unwrap();
        let sk2 = BlsPrivKey::from_bytes(&[22u8; 32]).unwrap();
        let pk1  = sk1.to_pubkey();

        let block_a = H256([1u8; 32]);
        let block_b = H256([2u8; 32]);

        // sig_b is from a DIFFERENT key — proof must be rejected.
        let sig_a = sk1.sign(&block_a);
        let sig_b = sk2.sign(&block_b); // wrong signer

        let proof = DoubleSignProof {
            height: 1, round: 0, phase: 0,
            block_a,
            block_b,
            sig_a: sig_a.as_bytes().to_vec(),
            sig_b: sig_b.as_bytes().to_vec(),
            validator:            Address([1u8; 20]),
            validator_bls_pubkey: pk1.as_bytes().to_vec(),
        };
        assert!(!proof.verify(), "mismatched signer must be rejected");
    }
}
