//! SEC-2026-05-09 Pass-7 — REGRESSION repros for S38-TRIE-REGRESSION.
//!
//! These two-key minimal repros were derived by proptest shrinking
//! when the Pass-7 harness was first run. **FIXED in Pass-8** — the
//! underlying MPT decoder bug (Branch/Extension `val_at::<Vec<u8>>`
//! failing on inline list children) is closed in `node.rs`. These
//! tests are now active regression guards. See:
//!   - `docs/SECURITY_FIXES_2026-05-09.md` Pass-8 §"S38 fix"
//!   - Known Issue row "S38-TRIE-REGRESSION" in `replit.md` (now ✅).
use zbx_trie::trie::MemoryTrieDB;
use zbx_trie::MutableTrie;

#[test]
fn s38_repro_two_one_byte_keys_value_zero() {
    let mut t = MutableTrie::new(MemoryTrieDB::default());
    t.insert(&[0u8], vec![0]).unwrap();
    t.insert(&[1u8], vec![0]).unwrap();
    assert_eq!(t.get(&[0u8]).unwrap(), Some(vec![0]));
    assert_eq!(t.get(&[1u8]).unwrap(), Some(vec![0]));
}

#[test]
fn s38_repro_two_one_byte_keys_long_value() {
    let mut t = MutableTrie::new(MemoryTrieDB::default());
    t.insert(&[0u8], b"longer-value-aaaa".to_vec()).unwrap();
    t.insert(&[1u8], b"longer-value-bbbb".to_vec()).unwrap();
    assert_eq!(t.get(&[0u8]).unwrap(), Some(b"longer-value-aaaa".to_vec()));
    assert_eq!(t.get(&[1u8]).unwrap(), Some(b"longer-value-bbbb".to_vec()));
}
