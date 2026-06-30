//! Vote messages and Quorum Certificates.

use zbx_types::{address::Address, H256};
use zbx_crypto::bls::{BlsSignature, BlsPubKey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The data that validators sign when casting a vote.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VoteData {
    /// Block hash being voted on.
    pub block_hash: H256,
    /// Block number (round).
    pub block_number: u64,
    /// Consensus phase: 0=Prepare, 1=PreCommit, 2=Commit.
    pub phase: u8,
    /// Epoch number for validator set rotation.
    pub epoch: u64,
}

impl VoteData {
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(72);
        b.extend_from_slice(self.block_hash.as_bytes());
        b.extend_from_slice(&self.block_number.to_be_bytes());
        b.push(self.phase);
        b.extend_from_slice(&self.epoch.to_be_bytes());
        b
    }
}

/// A signed vote from one validator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vote {
    pub data: VoteData,
    pub voter: Address,
    pub signature: BlsSignature,
}

/// A Quorum Certificate (QC) — 2f+1 votes on the same VoteData.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuorumCertificate {
    pub vote_data: VoteData,
    /// Aggregated BLS signature from all signers.
    pub agg_signature: BlsSignature,
    /// Ordered list of validator addresses that contributed.
    pub signers: Vec<Address>,
    /// Corresponding BLS public keys for verification.
    pub signer_pubkeys: Vec<BlsPubKey>,
}

impl QuorumCertificate {
    pub fn block_hash(&self) -> &H256 {
        &self.vote_data.block_hash
    }

    pub fn block_number(&self) -> u64 {
        self.vote_data.block_number
    }

    pub fn phase(&self) -> u8 {
        self.vote_data.phase
    }

    pub fn signer_count(&self) -> usize {
        self.signers.len()
    }

    pub fn verify(&self) -> bool {
        // Genesis QC has empty signers — accept unconditionally.
        if self.signer_pubkeys.is_empty() { return true; }
        let msg: H256 = {
            use zbx_crypto::keccak::keccak256;
            keccak256(&self.vote_data.signing_bytes())
        };
        zbx_crypto::bls::verify_aggregate(&self.agg_signature, &self.signer_pubkeys, &msg)
    }
}

/// Accumulates votes and forms a QC once quorum is reached.
pub struct VoteAccumulator {
    pub data: VoteData,
    votes: HashMap<Address, (Vote, BlsPubKey)>,
    quorum: usize,
}

impl VoteAccumulator {
    pub fn new(data: VoteData, quorum: usize) -> Self {
        VoteAccumulator { data, votes: HashMap::new(), quorum }
    }

    pub fn add_vote(
        &mut self,
        vote: Vote,
        pubkey: BlsPubKey,
    ) -> Result<Option<QuorumCertificate>, crate::ConsensusError> {
        if vote.data != self.data {
            return Ok(None); // vote for different data, ignore
        }
        let addr = vote.voter;
        if self.votes.contains_key(&addr) {
            return Err(crate::ConsensusError::DuplicateVote(format!("{:?}", addr)));
        }

        // M-08 fix (ZBX-M-08): verify the BLS signature before counting the vote.
        //
        // The old code inserted the vote unconditionally — a validator could
        // submit an all-zero signature and still contribute to quorum.  We now
        // verify the per-vote BLS signature over `vote.data.signing_bytes()` using
        // the caller-supplied public key before the vote is accepted into the
        // accumulator.  Votes with invalid signatures are silently dropped
        // (not counted; no error — consistent with how unknown-data votes are handled).
        let msg_hash = {
            use zbx_crypto::keccak::keccak256;
            keccak256(&vote.data.signing_bytes())
        };
        if !zbx_crypto::bls::verify_single(&vote.signature, &pubkey, &msg_hash) {
            return Ok(None); // invalid BLS signature — drop vote
        }

        self.votes.insert(addr, (vote, pubkey));
        if self.votes.len() >= self.quorum {
            Ok(Some(self.form_qc()))
        } else {
            Ok(None)
        }
    }

    fn form_qc(&self) -> QuorumCertificate {
        let (sigs, pubkeys, signers): (Vec<_>, Vec<_>, Vec<_>) = self
            .votes
            .values()
            .map(|(v, pk)| (v.signature.clone(), pk.clone(), v.voter))
            .unzip3_custom();

        // M-8 fix: was .expect() which would panic on BLS aggregation failure
        // (e.g. degenerate inputs, empty sigs). Now logs an error and returns
        // a zeroed signature — the QC will fail downstream verification cleanly
        // rather than crashing the consensus engine.
        let agg_sig = match zbx_crypto::bls::aggregate_signatures(&sigs) {
            Ok(sig) => sig,
            Err(e) => {
                tracing::error!(
                    round = ?self.data.block_number,
                    vote_count = sigs.len(),
                    error = %e,
                    "BLS aggregate_signatures failed in form_qc — QC will be invalid"
                );
                zbx_crypto::bls::BlsSignature([0u8; 96])
            }
        };
        QuorumCertificate {
            vote_data: self.data.clone(),
            agg_signature: agg_sig,
            signers,
            signer_pubkeys: pubkeys,
        }
    }
}

// SEC-2026-05-09 Pass-10 (architect-review follow-up): cross-validator
// equivocation evidence. Surfaced from `HotStuff2::on_vote` /
// `HotStuff::on_vote` when a single validator address has been observed
// signing two different block hashes for the same `(round, phase)`.
//
// Both votes (and the validator's BLS pubkey) are retained verbatim so
// the node-layer handler can:
//   1. Re-verify both signatures with the supplied pubkey (defence in
//      depth — the accumulator already verified each signature, but the
//      detector path also re-runs the check before raising the event).
//   2. Persist the evidence to RocksDB.
//   3. Feed it into `SlashingRegistryV2::submit_evidence(...)`.
//
// The struct is intentionally `Clone + Serialize + Deserialize` so it
// can be gossiped to peers and stored on-chain without re-derivation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EquivocationEvidence {
    pub validator: Address,
    pub round:     u64,
    pub phase:     u8,
    pub vote_a:    Vote,
    pub vote_b:    Vote,
    pub pubkey:    BlsPubKey,
}

impl EquivocationEvidence {
    /// Re-verify both votes' BLS signatures against `pubkey` and confirm
    /// the conflict is real (same validator + same round + same phase +
    /// different block hash). A `false` return means the evidence is
    /// malformed or fabricated and must NOT be acted on.
    pub fn verify(&self) -> bool {
        if self.vote_a.voter != self.vote_b.voter
            || self.vote_a.voter != self.validator
            || self.vote_a.data.block_number != self.vote_b.data.block_number
            || self.vote_a.data.phase        != self.vote_b.data.phase
            || self.vote_a.data.block_hash   == self.vote_b.data.block_hash
            || self.vote_a.data.block_number != self.round
            || self.vote_a.data.phase        != self.phase
        {
            return false;
        }
        use zbx_crypto::keccak::keccak256;
        let m_a = keccak256(&self.vote_a.data.signing_bytes());
        let m_b = keccak256(&self.vote_b.data.signing_bytes());
        zbx_crypto::bls::verify_single(&self.vote_a.signature, &self.pubkey, &m_a)
            && zbx_crypto::bls::verify_single(&self.vote_b.signature, &self.pubkey, &m_b)
    }
}

trait Unzip3Custom<A, B, C>: Iterator<Item = (A, B, C)> {
    fn unzip3_custom(self) -> (Vec<A>, Vec<B>, Vec<C>);
}
impl<I: Iterator<Item = (A, B, C)>, A, B, C> Unzip3Custom<A, B, C> for I {
    fn unzip3_custom(self) -> (Vec<A>, Vec<B>, Vec<C>) {
        let mut va = Vec::new(); let mut vb = Vec::new(); let mut vc = Vec::new();
        for (a, b, c) in self { va.push(a); vb.push(b); vc.push(c); }
        (va, vb, vc)
    }
}