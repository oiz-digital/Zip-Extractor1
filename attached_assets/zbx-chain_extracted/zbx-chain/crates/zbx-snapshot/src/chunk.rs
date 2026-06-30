//! Snapshot chunk — binary blob of accounts.

use serde::{Deserialize, Serialize};
use sha3::{Keccak256, Digest};
use super::AccountSnapshot;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotChunk {
    pub index:     u32,
    pub accounts:  Vec<AccountSnapshot>,
    pub start_key: [u8; 20],
    pub end_key:   [u8; 20],
    pub checksum:  [u8; 32],
}

impl SnapshotChunk {
    pub fn new(index: u32, accounts: Vec<AccountSnapshot>) -> Self {
        let start = accounts.first().map(|a| a.address).unwrap_or([0u8; 20]);
        let end   = accounts.last() .map(|a| a.address).unwrap_or([0xff; 20]);
        let ck    = Self::checksum(&accounts);
        Self { index, accounts, start_key: start, end_key: end, checksum: ck }
    }

    fn checksum(accounts: &[AccountSnapshot]) -> [u8; 32] {
        let mut h = Keccak256::new();
        for a in accounts { h.update(&a.address); h.update(&a.balance); h.update(&a.nonce.to_be_bytes()); }
        h.finalize().into()
    }

    pub fn verify(&self) -> bool { Self::checksum(&self.accounts) == self.checksum }
    pub fn to_bytes(&self) -> Result<Vec<u8>, Box<dyn std::error::Error>> { Ok(bincode::serialize(self)?) }
    pub fn from_bytes(b: &[u8]) -> Result<Self, Box<dyn std::error::Error>> { Ok(bincode::deserialize(b)?) }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn sample_accounts(n: usize) -> Vec<AccountSnapshot> {
        (0..n).map(|i| AccountSnapshot {
            address: { let mut a = [0u8; 20]; a[19] = i as u8; a },
            balance: { let mut b = [0u8; 32]; b[31] = (i * 10) as u8; b },
            nonce:   i as u64,
            code_hash: [0u8; 32],
            storage_root: [0u8; 32],
        }).collect()
    }

    #[test]
    fn new_chunk_sets_start_end_keys() {
        let accs = sample_accounts(3);
        let chunk = SnapshotChunk::new(0, accs.clone());
        assert_eq!(chunk.start_key, accs[0].address);
        assert_eq!(chunk.end_key,   accs[2].address);
        assert_eq!(chunk.index, 0);
        assert_eq!(chunk.accounts.len(), 3);
    }

    #[test]
    fn verify_passes_on_unmodified_chunk() {
        let chunk = SnapshotChunk::new(0, sample_accounts(5));
        assert!(chunk.verify());
    }

    #[test]
    fn verify_fails_after_tamper() {
        let mut chunk = SnapshotChunk::new(0, sample_accounts(5));
        chunk.accounts[0].nonce = 999_999;
        assert!(!chunk.verify());
    }

    #[test]
    fn empty_chunk_has_zero_address_bounds() {
        let chunk = SnapshotChunk::new(0, vec![]);
        assert_eq!(chunk.start_key, [0u8; 20]);
        assert_eq!(chunk.end_key,   [0u8; 20]);
    }

    #[test]
    fn deterministic_checksum() {
        let accs = sample_accounts(4);
        let c1 = SnapshotChunk::new(0, accs.clone());
        let c2 = SnapshotChunk::new(0, accs);
        assert_eq!(c1.checksum, c2.checksum);
    }
}
