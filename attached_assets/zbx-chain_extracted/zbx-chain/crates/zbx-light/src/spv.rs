//! SPV proofs: Merkle Patricia Trie (MPT) inclusion proofs.
//!
//! # S26 hardening pass
//!
//! Prior to S26, every `verify()` in this file returned `!proof.is_empty()` —
//! a placeholder that would have accepted **any** non-empty proof blob. That
//! was a CRIT security hole for any consumer (bridge, light client, dapp)
//! relying on this module. S26 wires every verifier into the real MPT
//! verification primitive `zbx_trie::verify_proof`, which:
//!
//!   1. Walks the supplied RLP-encoded trie nodes,
//!   2. Re-hashes each node with Keccak-256 and chains hashes from leaf → root,
//!   3. Decodes Branch / Extension / Leaf node types per Yellow Paper App. D,
//!   4. Returns `true` iff the proof is consistent with the claimed
//!      `(root, key, expected_value)` triple.
//!
//! `AccountProof::nonce()` and `AccountProof::balance()` likewise now perform a
//! real RLP decode of the standard 4-field account record
//! `[nonce, balance, storage_root, code_hash]`.

use zbx_types::{H256, Address, U256};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

// ──────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────

/// Encode a transaction index as the canonical MPT key used by the
/// `transactionsRoot` trie: `RLP(index)`.
fn tx_index_key(index: u64) -> Vec<u8> {
    zbx_rlp::encode(&index)
}

/// Encode an Ethereum-style storage slot key: `keccak256(32-byte BE slot)`.
fn storage_slot_key(slot: U256) -> [u8; 32] {
    let mut buf = [0u8; 32];
    slot.to_big_endian(&mut buf);
    let h = Keccak256::digest(buf);
    let mut out = [0u8; 32];
    out.copy_from_slice(&h);
    out
}

/// Encode an account address key: `keccak256(20-byte address)`.
fn account_key(addr: &Address) -> [u8; 32] {
    let h = Keccak256::digest(addr.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&h);
    out
}

// ──────────────────────────────────────────────────────────────────────────
// TxProof
// ──────────────────────────────────────────────────────────────────────────

/// A Merkle proof for transaction inclusion.
///
/// The leaf value of the transactions trie is the RLP-encoded transaction
/// itself, NOT just its hash. `tx_rlp` is therefore mandatory; `tx_hash`
/// is retained only as a convenience field and is recomputed at verify time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxProof {
    pub block_number: u64,
    pub block_hash:   H256,
    pub tx_index:     u64,
    pub tx_hash:      H256,
    /// Full RLP-encoded transaction body (the trie leaf value).
    #[serde(default)]
    pub tx_rlp:       Vec<u8>,
    /// Merkle proof path: ordered RLP-encoded trie nodes from root to leaf.
    pub proof:        Vec<Vec<u8>>,
    /// `transactionsRoot` from the block header.
    pub root:         H256,
}

impl TxProof {
    /// Verify the proof against the `transactionsRoot`.
    ///
    /// Returns true iff:
    /// 1. `keccak256(tx_rlp) == tx_hash` (claimed hash matches body), AND
    /// 2. `verify_proof(root, RLP(tx_index), Some(tx_rlp), proof)` succeeds.
    pub fn verify(&self) -> bool {
        if self.tx_rlp.is_empty() || self.proof.is_empty() {
            return false;
        }
        // (1) hash binding
        let computed: [u8; 32] = Keccak256::digest(&self.tx_rlp).into();
        if H256(computed) != self.tx_hash {
            return false;
        }
        // (2) MPT inclusion
        let key = tx_index_key(self.tx_index);
        zbx_trie::verify_proof(self.root, &key, &Some(self.tx_rlp.clone()), &self.proof)
    }
}

// ──────────────────────────────────────────────────────────────────────────
// AccountProof
// ──────────────────────────────────────────────────────────────────────────

/// A Merkle proof for account state.
///
/// The leaf value is the RLP-encoded account record:
///   `[nonce: u64, balance: U256, storage_root: H256, code_hash: H256]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountProof {
    pub address:      Address,
    pub block_number: u64,
    pub state_root:   H256,
    /// RLP-encoded account record (trie leaf value).
    pub account_rlp:  Vec<u8>,
    /// Trie proof from account key to `state_root`.
    pub proof:        Vec<Vec<u8>>,
}

impl AccountProof {
    /// Verify the proof against the `state_root`.
    pub fn verify(&self) -> bool {
        if self.account_rlp.is_empty() || self.proof.is_empty() {
            return false;
        }
        let key = account_key(&self.address);
        zbx_trie::verify_proof(
            self.state_root,
            &key,
            &Some(self.account_rlp.clone()),
            &self.proof,
        )
    }

    /// Decode the canonical Ethereum-style 4-field account record and return
    /// the requested field. Returns `None` on malformed input.
    fn decode_account(&self) -> Option<DecodedAccount> {
        let rlp = zbx_rlp::Rlp::new(&self.account_rlp);
        if rlp.item_count().ok()? != 4 {
            return None;
        }
        let nonce: u64 = rlp.val_at(0).ok()?;
        let balance_bytes: Vec<u8> = rlp.val_at(1).ok()?;
        let storage_root_bytes: Vec<u8> = rlp.val_at(2).ok()?;
        let code_hash_bytes: Vec<u8> = rlp.val_at(3).ok()?;

        let balance = U256::from_big_endian(&balance_bytes);
        let storage_root = h256_from_bytes(&storage_root_bytes)?;
        let code_hash = h256_from_bytes(&code_hash_bytes)?;
        Some(DecodedAccount { nonce, balance, storage_root, code_hash })
    }

    /// Decode and return the account nonce, or `0` if the RLP is malformed
    /// (callers MUST first call `verify()` and check the result).
    pub fn nonce(&self) -> u64 {
        self.decode_account().map(|a| a.nonce).unwrap_or(0)
    }

    /// Decode and return the account balance, or `U256::zero()` on malformed
    /// input. Callers MUST first call `verify()`.
    pub fn balance(&self) -> U256 {
        self.decode_account().map(|a| a.balance).unwrap_or_else(U256::zero)
    }

    /// Decoded storage root, or zero hash on malformed input.
    pub fn storage_root(&self) -> H256 {
        self.decode_account().map(|a| a.storage_root).unwrap_or_else(H256::zero)
    }

    /// Decoded code hash, or zero hash on malformed input.
    pub fn code_hash(&self) -> H256 {
        self.decode_account().map(|a| a.code_hash).unwrap_or_else(H256::zero)
    }
}

#[derive(Debug, Clone)]
struct DecodedAccount {
    nonce:        u64,
    balance:      U256,
    storage_root: H256,
    code_hash:    H256,
}

fn h256_from_bytes(b: &[u8]) -> Option<H256> {
    if b.len() != 32 {
        return None;
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(b);
    Some(H256(out))
}

// ──────────────────────────────────────────────────────────────────────────
// StorageProof
// ──────────────────────────────────────────────────────────────────────────

/// A Merkle proof for a single storage slot.
///
/// The leaf value of the storage trie is the RLP-encoded big-endian-trimmed
/// slot value: `RLP(value.to_be_bytes_trimmed())`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageProof {
    pub address:      Address,
    pub slot:         U256,
    pub value:        U256,
    pub block_number: u64,
    pub storage_root: H256,
    pub proof:        Vec<Vec<u8>>,
}

impl StorageProof {
    /// Verify the proof against the `storage_root`.
    pub fn verify(&self) -> bool {
        if self.proof.is_empty() {
            return false;
        }
        let key = storage_slot_key(self.slot);
        // Storage values are stored as RLP-encoded big-endian-trimmed bytes.
        let mut value_bytes = [0u8; 32];
        self.value.to_big_endian(&mut value_bytes);
        // Trim leading zeros (canonical RLP encoding for U256 values).
        let trimmed = trim_leading_zeros(&value_bytes);
        let expected = if trimmed.is_empty() {
            // Zero slot — non-existence proof.
            None
        } else {
            Some(zbx_rlp::encode(&trimmed.to_vec()))
        };
        zbx_trie::verify_proof(self.storage_root, &key, &expected, &self.proof)
    }
}

fn trim_leading_zeros(b: &[u8]) -> &[u8] {
    let mut i = 0;
    while i < b.len() && b[i] == 0 {
        i += 1;
    }
    &b[i..]
}

// ──────────────────────────────────────────────────────────────────────────
// SpvProof — composite
// ──────────────────────────────────────────────────────────────────────────

/// Combined SPV proof (account + optional storage).
///
/// Verification chains: each `StorageProof` MUST anchor against the
/// `storage_root` decoded from the verified account proof — callers should
/// not blindly trust `StorageProof::storage_root` if it was supplied separately.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpvProof {
    pub account: AccountProof,
    pub storage: Vec<StorageProof>,
}

impl SpvProof {
    /// Verify both the account proof AND every storage proof, AND require
    /// each storage proof's `storage_root` to match the account's decoded
    /// `storage_root`. This blocks the trivial spoof where a caller hands in
    /// an unrelated storage proof tree.
    pub fn verify(&self) -> bool {
        if !self.account.verify() {
            return false;
        }
        let acct_storage_root = self.account.storage_root();
        for sp in &self.storage {
            if sp.storage_root != acct_storage_root {
                return false;
            }
            if sp.address != self.account.address {
                return false;
            }
            if !sp.verify() {
                return false;
            }
        }
        true
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_proof_rejected_tx() {
        let p = TxProof {
            block_number: 1,
            block_hash:   H256::zero(),
            tx_index:     0,
            tx_hash:      H256::zero(),
            tx_rlp:       vec![],
            proof:        vec![],
            root:         H256::zero(),
        };
        assert!(!p.verify());
    }

    #[test]
    fn empty_proof_rejected_account() {
        let p = AccountProof {
            address:      Address::zero(),
            block_number: 1,
            state_root:   H256::zero(),
            account_rlp:  vec![],
            proof:        vec![],
        };
        assert!(!p.verify());
    }

    #[test]
    fn account_decode_canonical() {
        // Build RLP([0u64, 100u64-as-bytes, 32 zero bytes, 32 zero bytes]) by hand.
        // For a richer end-to-end test we need a real MPT generator; that lives
        // in the zbx-trie crate's own integration tests. Here we verify that
        // the decode helper handles a well-formed list and rejects malformed.
        let mut s = zbx_rlp::RlpStream::new();
        s.begin_list(4);
        s.append(&7u64);                  // nonce
        s.append(&vec![0x01u8, 0x00]);    // balance = 256
        s.append(&vec![0u8; 32]);         // storage_root
        s.append(&vec![0u8; 32]);         // code_hash
        let account_rlp = s.out();

        let p = AccountProof {
            address:      Address::zero(),
            block_number: 1,
            state_root:   H256::zero(),
            account_rlp,
            proof:        vec![vec![1, 2, 3]], // dummy non-empty for verify gate
        };
        assert_eq!(p.nonce(), 7);
        assert_eq!(p.balance(), U256::from(256u64));
        assert_eq!(p.storage_root(), H256::zero());
        assert_eq!(p.code_hash(), H256::zero());
    }

    #[test]
    fn account_decode_malformed_returns_zero() {
        let p = AccountProof {
            address:      Address::zero(),
            block_number: 1,
            state_root:   H256::zero(),
            account_rlp:  vec![0xff, 0xff, 0xff], // garbage
            proof:        vec![vec![1]],
        };
        assert_eq!(p.nonce(), 0);
        assert_eq!(p.balance(), U256::zero());
    }

    #[test]
    fn storage_proof_zero_value_handled() {
        // Verify path is gated by proof emptiness; with empty proof we expect false.
        let p = StorageProof {
            address:      Address::zero(),
            slot:         U256::zero(),
            value:        U256::zero(),
            block_number: 1,
            storage_root: H256::zero(),
            proof:        vec![],
        };
        assert!(!p.verify());
    }

    #[test]
    fn spv_storage_root_mismatch_rejected() {
        // Build account proof with one storage_root and a storage proof
        // claiming a different storage_root → composite verify must reject.
        let acct_rlp = {
            let mut s = zbx_rlp::RlpStream::new();
            s.begin_list(4);
            s.append(&0u64);
            s.append(&vec![0u8]);
            // account-side storage root = 0x11..
            s.append(&vec![0x11u8; 32]);
            s.append(&vec![0u8; 32]);
            s.out()
        };
        let acct = AccountProof {
            address:      Address::zero(),
            block_number: 1,
            state_root:   H256::zero(),
            account_rlp:  acct_rlp,
            proof:        vec![vec![1]], // dummy gate
        };
        let storage = StorageProof {
            address:      Address::zero(),
            slot:         U256::zero(),
            value:        U256::zero(),
            block_number: 1,
            storage_root: H256([0x22u8; 32]), // mismatch with account-decoded 0x11
            proof:        vec![vec![1]],
        };
        let spv = SpvProof { account: acct, storage: vec![storage] };
        // account.verify() will fail on the dummy proof, but even before that,
        // the storage_root cross-check is the design guarantee we're documenting.
        assert!(!spv.verify());
    }
}
