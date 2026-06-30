//! SEC-2026-05-09 Pass-11 — slashing-evidence persistence layer.
//!
//! Pre-Pass-11 the consensus remote-equivocation detector
//! (`zbx_consensus::HotStuffConsensus::on_vote` →
//! `ConsensusError::RemoteEquivocation`) only emitted a
//! `tracing::error!("SLASHABLE")` log. The verified
//! `EquivocationEvidence` was thrown away on the next process
//! restart, the `SlashingRegistryV2` was never told, and zero stake
//! was ever burnt. The chain had a detector but no economic
//! security — listed as one of the two HARD mainnet blockers in
//! `docs/SUBSYSTEM-MATURITY-AUDIT-2026-05-09.md`.
//!
//! This module closes the input side of the gap: durable storage of
//! both raw `EquivocationEvidence` (the consensus-layer evidence
//! type) and `SlashEvidenceRecord` (the staking-layer registry
//! record). The `SlashingPipeline` (sibling module) consumes from
//! here and drives the registry.
//!
//! # Honest scope
//!
//! - Encoding is `bincode` v1 — compact, deterministic for given
//!   schema, and the existing repo-wide convention for non-RLP
//!   storage payloads (matches `zbx-snapshot`).
//! - Evidence ID = `keccak256(bincode(evidence))` so re-detecting
//!   the same `(validator, round, phase, vote_a, vote_b)` is
//!   idempotent in storage.
//! - Writes are fsynced (`ZbxDb::put_slashing_*` use
//!   `write_synced`). Slashing evidence is too important to lose to
//!   a sub-second crash.
//! - No on-chain bond escrow yet — `SlashingRegistryV2`'s
//!   `pending_bonds` map is in-memory; persisting bonds requires
//!   coupling to `StateDB::transfer_balance` which is a separate
//!   audit. Documented gap.

use crate::slashing_v2::{SlashEvidenceRecord, SlashEvidenceV2, EvidenceStatus, SLASH_RECORD_FORMAT_VERSION};
use crate::error::StakingError;
use zbx_consensus::vote::EquivocationEvidence;
use zbx_storage::ZbxDb;
use zbx_types::{address::Address, H256};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};
use std::sync::Arc;
use tracing::{debug, error, warn};

// ── Bond ledger types (Slashing-upgrade) ─────────────────────────────────

/// Distinguishes the purpose of a slashing-bond ledger entry. Carried
/// as part of `BondEntry` so a single `(record_id, reporter)` row can
/// be classified without re-reading the record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BondKind {
    /// Whistleblower deposit posted by an evidence submitter. Pre-
    /// upgrade these lived in `SlashingRegistryV2.pending_bonds` and
    /// vanished on restart. The on-chain consensus auto-detection
    /// path currently records 0-wei whistleblower bonds (the chain's
    /// own consensus is the "submitter" — no spam-deposit needed);
    /// non-zero entries come from future operator-submitted evidence
    /// transactions.
    Whistleblower,
    /// Appeal bond posted by the slashed validator on `FileAppeal`.
    /// Refunded on successful overturn, forfeited on rejection.
    Appeal,
}

/// On-disk bond record. Value half of `SlashingBonds` CF.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BondEntry {
    pub wei: u128,
    pub kind: BondKind,
}

/// Compute the canonical 32-byte ID for an `EquivocationEvidence`.
///
/// The ID is `keccak256(bincode(evidence))` — content-addressed so
/// re-detection of the same conflict is a no-op (the second `put`
/// overwrites the first with identical bytes).
///
/// We deliberately do NOT key by `(validator, round, phase)` alone —
/// two distinct equivocations in the same slot (e.g. an attacker
/// signing three different blocks) must each get their own record.
pub fn evidence_id(ev: &EquivocationEvidence) -> H256 {
    let bytes = bincode::serialize(ev)
        .expect("EquivocationEvidence is serde-serializable (struct of \
                 plain types) — serialization cannot fail");
    let mut h = Keccak256::new();
    h.update(&bytes);
    let out = h.finalize();
    let mut id = [0u8; 32];
    id.copy_from_slice(&out);
    H256(id)
}

/// Durable store for slashing evidence and registry records.
///
/// Backed by two `ZbxDb` column families
/// (`Column::SlashingEvidence` + `Column::SlashingRecords`). All
/// writes are fsynced — see module docs for rationale.
#[derive(Clone)]
pub struct EvidenceStore {
    db: Arc<ZbxDb>,
}

impl EvidenceStore {
    pub fn new(db: Arc<ZbxDb>) -> Self {
        Self { db }
    }

    // ── EquivocationEvidence (consensus-layer input) ────────────────────────

    /// Persist an equivocation evidence. Returns the canonical ID.
    /// Idempotent: storing the same evidence twice is a no-op (same
    /// key, same bytes).
    pub fn put_evidence(
        &self,
        ev: &EquivocationEvidence,
    ) -> Result<H256, StakingError> {
        let id = evidence_id(ev);
        let bytes = bincode::serialize(ev)
            .map_err(|e| StakingError::Persistence(format!("evidence encode: {e}")))?;
        self.db.put_slashing_evidence(&id.0, bytes)
            .map_err(|e| StakingError::Persistence(format!("evidence put: {e}")))?;
        debug!(evidence_id = ?id, validator = ?ev.validator, "evidence persisted");
        Ok(id)
    }

    pub fn get_evidence(
        &self,
        id: &H256,
    ) -> Result<Option<EquivocationEvidence>, StakingError> {
        let bytes = self.db.get_slashing_evidence(&id.0)
            .map_err(|e| StakingError::Persistence(format!("evidence get: {e}")))?;
        match bytes {
            None => Ok(None),
            Some(b) => bincode::deserialize::<EquivocationEvidence>(&b)
                .map(Some)
                .map_err(|e| StakingError::Persistence(
                    format!("evidence decode (corrupt entry — id={id:?}): {e}"))),
        }
    }

    /// Load every persisted evidence. Called at node startup to
    /// rehydrate the pipeline so a crash between detection and
    /// finalization does not lose the slashing event.
    pub fn load_all_evidence(
        &self,
    ) -> Result<Vec<(H256, EquivocationEvidence)>, StakingError> {
        let raw = self.db.iter_slashing_evidence()
            .map_err(|e| StakingError::Persistence(format!("evidence iter: {e}")))?;
        let mut out = Vec::with_capacity(raw.len());
        for (id_bytes, blob) in raw {
            match bincode::deserialize::<EquivocationEvidence>(&blob) {
                Ok(ev) => out.push((H256(id_bytes), ev)),
                Err(e) => {
                    // SEC-2026-05-09 Pass-11 (architect-review
                    // follow-up #2): FAIL-CLOSED. A corrupt evidence
                    // entry on disk is silently the same as silently
                    // forgiving an offender — we cannot tell whether
                    // the corruption is a real disk fault or a
                    // tampered file dropped by an attacker with shell
                    // access. Hard-fail rehydrate so the operator
                    // investigates before the chain re-joins
                    // consensus.
                    error!(id = ?H256(id_bytes), error = %e,
                          "FATAL: corrupt evidence entry on disk — refusing to rehydrate");
                    return Err(StakingError::Persistence(format!(
                        "corrupt evidence entry id={:?}: {}",
                        H256(id_bytes), e
                    )));
                }
            }
        }
        Ok(out)
    }

    // ── SlashEvidenceRecord (registry-layer state) ──────────────────────────

    /// Persist or overwrite a registry record. Called every time
    /// the registry transitions a record (Pending → Appealed →
    /// Confirmed / Overturned) so a node restart rehydrates the
    /// registry deterministically.
    pub fn put_record(
        &self,
        record: &SlashEvidenceRecord,
    ) -> Result<(), StakingError> {
        let bytes = bincode::serialize(record)
            .map_err(|e| StakingError::Persistence(format!("record encode: {e}")))?;
        self.db.put_slashing_record(&record.id.0, bytes)
            .map_err(|e| StakingError::Persistence(format!("record put: {e}")))?;
        debug!(
            record_id = ?record.id,
            offender = ?record.offender,
            status = ?record.status,
            "record persisted",
        );
        Ok(())
    }

    pub fn get_record(
        &self,
        id: &H256,
    ) -> Result<Option<SlashEvidenceRecord>, StakingError> {
        let bytes = self.db.get_slashing_record(&id.0)
            .map_err(|e| StakingError::Persistence(format!("record get: {e}")))?;
        match bytes {
            None => Ok(None),
            Some(b) => bincode::deserialize::<SlashEvidenceRecord>(&b)
                .map(Some)
                .map_err(|e| StakingError::Persistence(
                    format!("record decode (corrupt entry — id={id:?}): {e}"))),
        }
    }

    pub fn load_all_records(
        &self,
    ) -> Result<Vec<SlashEvidenceRecord>, StakingError> {
        let raw = self.db.iter_slashing_records()
            .map_err(|e| StakingError::Persistence(format!("record iter: {e}")))?;
        let mut out = Vec::with_capacity(raw.len());
        for (id_bytes, blob) in raw {
            match bincode::deserialize::<SlashEvidenceRecord>(&blob) {
                Ok(rec) => out.push(rec),
                Err(e) => {
                    // SEC-2026-05-09 Pass-11 (architect-review
                    // follow-up #2): FAIL-CLOSED on corrupt record —
                    // see same rationale in `load_all_evidence`.
                    error!(id = ?H256(id_bytes), error = %e,
                           "FATAL: corrupt record entry on disk — refusing to rehydrate");
                    return Err(StakingError::Persistence(format!(
                        "corrupt record entry id={:?}: {}",
                        H256(id_bytes), e
                    )));
                }
            }
        }
        Ok(out)
    }

    /// Filter helper used by the pipeline tick — returns just the
    /// records past their appeal deadline that are still `Pending`
    /// (i.e., ready for `finalize_slash`).
    pub fn load_records_ready_to_finalize(
        &self,
        current_block: u64,
    ) -> Result<Vec<SlashEvidenceRecord>, StakingError> {
        Ok(self.load_all_records()?
            .into_iter()
            .filter(|r| {
                r.status == EvidenceStatus::Pending
                    && current_block > r.appeal_deadline
            })
            .collect())
    }

    /// **Slashing-upgrade — crash-consistency replay filter.**
    /// Returns Confirmed records whose burn has not yet been
    /// applied (two-phase finalize). The pipeline replays the
    /// validator-set burn on each of these on startup / next tick
    /// so a crash between `finalize_slash` (status flip) and
    /// `apply_slash_burn_v2` cannot leave a record permanently
    /// Confirmed without its burn ever taking effect.
    ///
    /// Backward-compat note: legacy pre-upgrade Confirmed records
    /// deserialize with `burn_applied = false` (serde default). Those
    /// records have ALREADY had their burn applied by the legacy
    /// burn-then-persist path; the pipeline's replay loop checks
    /// the validator's current state (jailed/tombstoned + already
    /// reduced stake) to avoid double-burning. See
    /// `SlashingPipeline::tick_finalize` for the full guard.
    pub fn load_records_with_unapplied_burn(
        &self,
    ) -> Result<Vec<SlashEvidenceRecord>, StakingError> {
        Ok(self.load_all_records()?
            .into_iter()
            .filter(|r| {
                // Strict triple-gate: must be a NEW upgraded record
                // (format_version >= 1), Confirmed, and explicitly
                // not-yet-burned. Pre-upgrade records carry
                // format_version=0 (serde default) and are excluded
                // by construction even though they too have
                // burn_applied=false on disk.
                r.format_version >= SLASH_RECORD_FORMAT_VERSION
                    && r.status == EvidenceStatus::Confirmed
                    && !r.burn_applied
            })
            .collect())
    }

    // ── Bond ledger (Slashing-upgrade) ──────────────────────────────────────

    /// Persist a bond. Idempotent on `(record_id, reporter)`: writing
    /// the same key with a new value overwrites (used by appeal-bond
    /// post-overturn refund accounting).
    pub fn put_bond(
        &self,
        record_id: &H256,
        reporter:  &Address,
        bond:      &BondEntry,
    ) -> Result<(), StakingError> {
        let bytes = bincode::serialize(bond)
            .map_err(|e| StakingError::Persistence(format!("bond encode: {e}")))?;
        self.db.put_slashing_bond(&record_id.0, reporter, bytes)
            .map_err(|e| StakingError::Persistence(format!("bond put: {e}")))?;
        debug!(record_id = ?record_id, reporter = ?reporter,
               wei = bond.wei, kind = ?bond.kind, "bond persisted");
        Ok(())
    }

    pub fn get_bond(
        &self,
        record_id: &H256,
        reporter:  &Address,
    ) -> Result<Option<BondEntry>, StakingError> {
        let bytes = self.db.get_slashing_bond(&record_id.0, reporter)
            .map_err(|e| StakingError::Persistence(format!("bond get: {e}")))?;
        match bytes {
            None => Ok(None),
            Some(b) => bincode::deserialize::<BondEntry>(&b)
                .map(Some)
                .map_err(|e| StakingError::Persistence(
                    format!("bond decode (corrupt — id={record_id:?}, reporter={reporter:?}): {e}"))),
        }
    }

    pub fn delete_bond(
        &self,
        record_id: &H256,
        reporter:  &Address,
    ) -> Result<(), StakingError> {
        self.db.delete_slashing_bond(&record_id.0, reporter)
            .map_err(|e| StakingError::Persistence(format!("bond delete: {e}")))
    }

    /// All bonds attached to a single slash record (prefix scan on
    /// `record_id`). Used at finalize / overturn time to enumerate
    /// whistleblower + appeal bonds for a given slash.
    pub fn list_bonds_for_record(
        &self,
        record_id: &H256,
    ) -> Result<Vec<(Address, BondEntry)>, StakingError> {
        let raw = self.db.iter_slashing_bonds_for_record(&record_id.0)
            .map_err(|e| StakingError::Persistence(format!("bond list: {e}")))?;
        let mut out = Vec::with_capacity(raw.len());
        for (reporter, blob) in raw {
            // FAIL-CLOSED on corruption (same rationale as records).
            let entry = bincode::deserialize::<BondEntry>(&blob)
                .map_err(|e| {
                    error!(record_id = ?record_id, reporter = ?reporter, error = %e,
                           "FATAL: corrupt bond entry on disk");
                    StakingError::Persistence(format!(
                        "corrupt bond record_id={record_id:?} reporter={reporter:?}: {e}"
                    ))
                })?;
            out.push((reporter, entry));
        }
        Ok(out)
    }
}

/// Convert a consensus-layer `EquivocationEvidence` into the
/// staking-layer `SlashEvidenceV2::DoubleSign`. The conversion is
/// total — every field maps 1:1 because the consensus detector
/// already verified both BLS signatures against the registered
/// validator pubkey before raising `RemoteEquivocation`.
///
/// The phase (consensus 3-phase: Prepare / PreCommit / Commit) maps
/// directly to `DoubleSignProof.phase: u8` (0 / 1 / 2). The signed
/// message is `keccak256(VoteData::signing_bytes())` — but the
/// `DoubleSignProof::verify()` checks the BLS sig against the raw
/// 32-byte block hash, which differs from the consensus signing
/// scheme. We therefore include both raw blocks AND the consensus
/// vote signatures; downstream `verify()` re-checks the consensus
/// way (vote.sig over keccak(VoteData::signing_bytes())) NOT the
/// raw-hash way. Consequently we explicitly re-verify with
/// `EquivocationEvidence::verify()` *before* persisting.
pub fn evidence_to_double_sign(ev: &EquivocationEvidence) -> SlashEvidenceV2 {
    use crate::slashing_v2::DoubleSignProof;
    SlashEvidenceV2::DoubleSign(DoubleSignProof {
        height:    ev.vote_a.data.block_number,
        round:     ev.round,
        phase:     ev.phase,
        block_a:   ev.vote_a.data.block_hash,
        block_b:   ev.vote_b.data.block_hash,
        sig_a:     ev.vote_a.signature.as_bytes().to_vec(),
        sig_b:     ev.vote_b.signature.as_bytes().to_vec(),
        validator: ev.validator,
        validator_bls_pubkey: ev.pubkey.as_bytes().to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbx_consensus::vote::{Vote, VoteData};
    use zbx_crypto::bls::BlsPrivKey;
    use zbx_types::address::Address;
    use tempfile::TempDir;

    fn mk_evidence(addr: Address, hash_a: H256, hash_b: H256) -> EquivocationEvidence {
        let sk = BlsPrivKey::from_bytes(&[7u8; 32]).unwrap();
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
    fn roundtrip_evidence_through_db() {
        let tmp = TempDir::new().unwrap();
        let db  = Arc::new(ZbxDb::open(tmp.path()).unwrap());
        let store = EvidenceStore::new(db);

        let addr = Address([0x11; 20]);
        let ev = mk_evidence(addr, H256([1u8; 32]), H256([2u8; 32]));
        assert!(ev.verify(), "test fixture must self-verify");

        let id = store.put_evidence(&ev).unwrap();
        let loaded = store.get_evidence(&id).unwrap().unwrap();
        assert_eq!(loaded.validator, ev.validator);
        assert_eq!(loaded.vote_a.data.block_hash, H256([1u8; 32]));
        assert_eq!(loaded.vote_b.data.block_hash, H256([2u8; 32]));
        assert!(loaded.verify(), "loaded evidence must still verify");
    }

    #[test]
    fn evidence_id_is_content_hash_idempotent() {
        let tmp = TempDir::new().unwrap();
        let db  = Arc::new(ZbxDb::open(tmp.path()).unwrap());
        let store = EvidenceStore::new(db);

        let addr = Address([0x22; 20]);
        let ev   = mk_evidence(addr, H256([3u8; 32]), H256([4u8; 32]));
        let id1  = store.put_evidence(&ev).unwrap();
        let id2  = store.put_evidence(&ev).unwrap(); // re-detection
        assert_eq!(id1, id2, "same evidence → same content-hash ID");
        assert_eq!(store.load_all_evidence().unwrap().len(), 1,
                   "duplicate must coalesce");
    }

    #[test]
    fn load_all_evidence_returns_persisted() {
        let tmp = TempDir::new().unwrap();
        let db  = Arc::new(ZbxDb::open(tmp.path()).unwrap());
        let store = EvidenceStore::new(db);

        let a = mk_evidence(Address([1; 20]), H256([1; 32]), H256([2; 32]));
        let b = mk_evidence(Address([2; 20]), H256([3; 32]), H256([4; 32]));
        store.put_evidence(&a).unwrap();
        store.put_evidence(&b).unwrap();

        let loaded = store.load_all_evidence().unwrap();
        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn evidence_survives_db_reopen() {
        // Critical regression — pre-Pass-11 evidence vanished on
        // restart. Confirm a reopen+iter sees what was persisted.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().to_owned();

        let id_persisted = {
            let db = Arc::new(ZbxDb::open(&path).unwrap());
            let store = EvidenceStore::new(db);
            let ev = mk_evidence(Address([0x33; 20]),
                                 H256([5; 32]), H256([6; 32]));
            store.put_evidence(&ev).unwrap()
        };

        // simulate restart
        let db2 = Arc::new(ZbxDb::open(&path).unwrap());
        let store2 = EvidenceStore::new(db2);
        let loaded = store2.get_evidence(&id_persisted).unwrap();
        assert!(loaded.is_some(),
                "evidence must survive process restart (pre-Pass-11 bug)");
    }

    #[test]
    fn evidence_to_double_sign_preserves_fields() {
        let addr = Address([0x44; 20]);
        let ev = mk_evidence(addr, H256([7; 32]), H256([8; 32]));
        match evidence_to_double_sign(&ev) {
            SlashEvidenceV2::DoubleSign(p) => {
                assert_eq!(p.validator, addr);
                assert_eq!(p.block_a, H256([7; 32]));
                assert_eq!(p.block_b, H256([8; 32]));
                assert_eq!(p.round, 5);
                assert_eq!(p.phase, 0);
                assert_eq!(p.sig_a.len(), 96);
                assert_eq!(p.sig_b.len(), 96);
                assert_eq!(p.validator_bls_pubkey.len(), 48);
            }
            _ => panic!("must produce DoubleSign variant"),
        }
    }
}
