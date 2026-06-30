//! M-of-N multisig wallet for ZBX Chain.
//!
//! Implements a threshold signature scheme:
//! - Any M of N owners can authorize a transaction
//! - Proposals are collected off-chain, then submitted to the ZVM contract
//! - Nonce increments after each execution to prevent replay attacks
//! - On-chain: deployed as a ZVM smart contract (ZEP-017 Account Abstraction)
//!
//! ## Workflow
//! 1. Any owner calls `propose(to, value, data)` → receives `proposal_id`
//! 2. M owners each call `sign(proposal_id, their_address, their_signature)`
//! 3. Any owner calls `execute(proposal_id)` → returns ready-to-broadcast tx
//!
//! ## Address derivation
//! wallet_address = keccak256(sorted_owners || threshold || "ZBX-MS-V1")[12..]
//!
//! This is deterministic: the same set of owners always recovers the same
//! wallet address without needing any on-chain registry.
//!
//! ## Serde note
//! Signatures are stored as `Vec<u8>` (not `[u8; 65]`) because serde does not
//! auto-implement Serialize/Deserialize for arrays larger than 32 bytes.

use sha3::{Keccak256, Digest};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;

/// M-of-N multisig wallet (off-chain state tracker).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiSigWallet {
    /// Derived wallet address — keccak256(sorted_owners || threshold || "ZBX-MS-V1")[12..]
    pub address:   [u8; 20],
    /// All N authorized signer addresses
    pub owners:    Vec<[u8; 20]>,
    /// Minimum signature count required (M ≤ N)
    pub threshold: usize,
    /// Execution nonce (increments after each successful execution)
    pub nonce:     u64,
    /// Pending proposals awaiting signatures
    #[serde(skip)]
    pub pending:   HashMap<[u8; 32], Proposal>,
}

/// A pending transaction proposal awaiting M signatures.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    /// Proposal ID = keccak256(to || value_be || data || nonce_be)
    pub id:         [u8; 32],
    /// Destination address
    pub to:         [u8; 20],
    /// Transfer amount in wei (u128 covers full EVM uint256 range for ZBX amounts)
    pub value:      u128,
    /// ABI-encoded call data (empty = plain ZBX transfer)
    pub data:       Vec<u8>,
    /// Wallet nonce at proposal creation (replay protection)
    pub nonce:      u64,
    /// Collected signatures: (signer_address, 65-byte ECDSA sig as Vec<u8>)
    ///
    /// Stored as `Vec<u8>` (not `[u8; 65]`) for serde compatibility —
    /// serde only auto-derives for arrays up to [T; 32].
    pub signatures: Vec<([u8; 20], Vec<u8>)>,
    /// True if this proposal has been executed
    pub executed:   bool,
}

/// Multisig errors.
#[derive(Debug, PartialEq)]
pub enum MultiSigError {
    /// Signer is not in the owners list
    NotOwner,
    /// This signer already submitted a signature for this proposal
    AlreadySigned,
    /// Proposal was already executed
    AlreadyExecuted,
    /// Not enough signatures to execute yet
    InsufficientSignatures,
    /// No proposal with this ID exists
    ProposalNotFound,
    /// threshold == 0 or threshold > owners.len()
    InvalidThreshold,
    /// owners list is empty
    NoOwners,
}

impl MultiSigWallet {
    /// Create a new M-of-N multisig wallet.
    ///
    /// The wallet address is deterministically derived from the owner set and
    /// threshold — no on-chain deployment needed to know the address in advance.
    pub fn new(owners: Vec<[u8; 20]>, threshold: usize) -> Result<Self, MultiSigError> {
        if owners.is_empty() {
            return Err(MultiSigError::NoOwners);
        }
        if threshold == 0 || threshold > owners.len() {
            return Err(MultiSigError::InvalidThreshold);
        }
        // Deterministic address from sorted owners (order-independent)
        let mut sorted = owners.clone();
        sorted.sort_unstable();
        let mut h = Keccak256::new();
        for o in &sorted { h.update(o); }
        h.update(&(threshold as u64).to_be_bytes());
        h.update(b"ZBX-MS-V1");
        let hash = h.finalize();
        let mut address = [0u8; 20];
        address.copy_from_slice(&hash[12..]);
        Ok(Self {
            address, owners, threshold, nonce: 0,
            pending: HashMap::new(),
        })
    }

    /// Propose a new transaction. Returns the `proposal_id` for signing.
    pub fn propose(&mut self, to: [u8; 20], value: u128, data: Vec<u8>) -> [u8; 32] {
        let id = proposal_id(&to, value, &data, self.nonce);
        self.pending.insert(id, Proposal {
            id, to, value, data,
            nonce:      self.nonce,
            signatures: Vec::new(),
            executed:   false,
        });
        id
    }

    /// Add a signature to a proposal.
    ///
    /// Accepts a 65-byte ECDSA signature. The caller is responsible for
    /// verifying that `signature` is valid before calling this.
    pub fn sign(
        &mut self,
        proposal_id: [u8; 32],
        signer:      [u8; 20],
        signature:   [u8; 65],
    ) -> Result<(), MultiSigError> {
        if !self.owners.iter().any(|o| *o == signer) {
            return Err(MultiSigError::NotOwner);
        }
        let p = self.pending.get_mut(&proposal_id)
            .ok_or(MultiSigError::ProposalNotFound)?;
        if p.executed {
            return Err(MultiSigError::AlreadyExecuted);
        }
        if p.signatures.iter().any(|(addr, _)| *addr == signer) {
            return Err(MultiSigError::AlreadySigned);
        }
        // Store as Vec<u8> for serde compatibility
        p.signatures.push((signer, signature.to_vec()));
        Ok(())
    }

    /// Return true if this proposal has at least M signatures.
    pub fn can_execute(&self, proposal_id: &[u8; 32]) -> bool {
        self.pending.get(proposal_id)
            .map(|p| !p.executed && p.signatures.len() >= self.threshold)
            .unwrap_or(false)
    }

    /// Mark a proposal as executed and increment the nonce.
    ///
    /// Returns the executed proposal for on-chain broadcast via zbx RPC.
    pub fn execute(&mut self, proposal_id: [u8; 32]) -> Result<Proposal, MultiSigError> {
        let p = self.pending.get_mut(&proposal_id)
            .ok_or(MultiSigError::ProposalNotFound)?;
        if p.executed {
            return Err(MultiSigError::AlreadyExecuted);
        }
        if p.signatures.len() < self.threshold {
            return Err(MultiSigError::InsufficientSignatures);
        }
        p.executed = true;
        let executed = p.clone();
        self.nonce += 1;
        Ok(executed)
    }

    /// Return all pending (not yet executed) proposals.
    pub fn pending_proposals(&self) -> Vec<&Proposal> {
        self.pending.values().filter(|p| !p.executed).collect()
    }

    /// Check whether an address is one of the N owners.
    pub fn is_owner(&self, address: &[u8; 20]) -> bool {
        self.owners.iter().any(|o| o == address)
    }

    /// Current signature count for a proposal.
    pub fn signature_count(&self, proposal_id: &[u8; 32]) -> usize {
        self.pending.get(proposal_id)
            .map(|p| p.signatures.len())
            .unwrap_or(0)
    }
}

/// Compute the proposal ID: keccak256(to || value_be || data || nonce_be).
fn proposal_id(to: &[u8; 20], value: u128, data: &[u8], nonce: u64) -> [u8; 32] {
    let mut h = Keccak256::new();
    h.update(to);
    h.update(&value.to_be_bytes());
    h.update(data);
    h.update(&nonce.to_be_bytes());
    h.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(b: u8) -> [u8; 20] { [b; 20] }
    fn dummy_sig() -> [u8; 65] { [0u8; 65] }

    fn three_of_five() -> MultiSigWallet {
        let owners = vec![addr(1), addr(2), addr(3), addr(4), addr(5)];
        MultiSigWallet::new(owners, 3).unwrap()
    }

    #[test]
    fn new_derives_deterministic_address() {
        let w1 = three_of_five();
        let w2 = three_of_five();
        assert_eq!(w1.address, w2.address);
    }

    #[test]
    fn owner_order_does_not_change_address() {
        let owners_a = vec![addr(1), addr(2), addr(3)];
        let owners_b = vec![addr(3), addr(1), addr(2)];
        let w1 = MultiSigWallet::new(owners_a, 2).unwrap();
        let w2 = MultiSigWallet::new(owners_b, 2).unwrap();
        assert_eq!(w1.address, w2.address);
    }

    #[test]
    fn invalid_threshold_rejected() {
        assert!(MultiSigWallet::new(vec![addr(1), addr(2)], 3).is_err());
        assert!(MultiSigWallet::new(vec![addr(1)], 0).is_err());
    }

    #[test]
    fn empty_owners_rejected() {
        assert!(MultiSigWallet::new(vec![], 1).is_err());
    }

    #[test]
    fn propose_returns_id() {
        let mut w = three_of_five();
        let id = w.propose(addr(9), 1_000_000, vec![]);
        assert!(!id.iter().all(|&b| b == 0));
    }

    #[test]
    fn sign_by_non_owner_fails() {
        let mut w = three_of_five();
        let id = w.propose(addr(9), 0, vec![]);
        assert_eq!(w.sign(id, addr(9), dummy_sig()), Err(MultiSigError::NotOwner));
    }

    #[test]
    fn double_sign_fails() {
        let mut w = three_of_five();
        let id = w.propose(addr(9), 0, vec![]);
        w.sign(id, addr(1), dummy_sig()).unwrap();
        assert_eq!(w.sign(id, addr(1), dummy_sig()), Err(MultiSigError::AlreadySigned));
    }

    #[test]
    fn cannot_execute_before_threshold() {
        let mut w = three_of_five();
        let id = w.propose(addr(9), 0, vec![]);
        w.sign(id, addr(1), dummy_sig()).unwrap();
        w.sign(id, addr(2), dummy_sig()).unwrap();
        assert_eq!(w.execute(id), Err(MultiSigError::InsufficientSignatures));
    }

    #[test]
    fn execute_after_threshold_succeeds_and_bumps_nonce() {
        let mut w = three_of_five();
        let id = w.propose(addr(9), 100, vec![0xab]);
        w.sign(id, addr(1), dummy_sig()).unwrap();
        w.sign(id, addr(2), dummy_sig()).unwrap();
        w.sign(id, addr(3), dummy_sig()).unwrap();
        assert!(w.can_execute(&id));
        let p = w.execute(id).unwrap();
        assert!(p.executed);
        assert_eq!(w.nonce, 1);
    }

    #[test]
    fn double_execute_fails() {
        let mut w = three_of_five();
        let id = w.propose(addr(9), 0, vec![]);
        for o in [addr(1), addr(2), addr(3)] { w.sign(id, o, dummy_sig()).unwrap(); }
        w.execute(id).unwrap();
        assert_eq!(w.execute(id), Err(MultiSigError::AlreadyExecuted));
    }

    #[test]
    fn is_owner_works() {
        let w = three_of_five();
        assert!(w.is_owner(&addr(1)));
        assert!(!w.is_owner(&addr(9)));
    }

    #[test]
    fn signature_count_tracks() {
        let mut w = three_of_five();
        let id = w.propose(addr(9), 0, vec![]);
        assert_eq!(w.signature_count(&id), 0);
        w.sign(id, addr(1), dummy_sig()).unwrap();
        assert_eq!(w.signature_count(&id), 1);
    }
}
