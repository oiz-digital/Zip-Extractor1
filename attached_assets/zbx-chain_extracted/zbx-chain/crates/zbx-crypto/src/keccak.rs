//! Keccak-256 and Keccak-512 hashing (not SHA3 — the Ethereum variant).

use zbx_types::H256;
use sha3::{Digest, Keccak256 as K256, Keccak512 as K512};

/// Compute the Keccak-256 hash of arbitrary bytes.
///
/// This is the hash function used throughout Zebvix and Ethereum:
/// address derivation, transaction hashing, storage key derivation, etc.
pub fn keccak256(data: &[u8]) -> H256 {
    H256::from_slice(&K256::digest(data))
}

/// Compute the Keccak-512 hash of arbitrary bytes.
pub fn keccak512(data: &[u8]) -> [u8; 64] {
    K512::digest(data).into()
}

/// Keccak-256 of two concatenated byte slices (avoids allocation).
pub fn keccak256_pair(a: &[u8], b: &[u8]) -> H256 {
    let mut h = K256::new();
    h.update(a);
    h.update(b);
    H256::from_slice(&h.finalize())
}

/// EIP-191 personal sign prefix: byte 0x19 || "Ethereum Signed Message:\n" || len.
///
/// Audit-2026-05-01 S7-CR1: previous code used `format!("\\x19...\\n{}")` which
/// expanded to the literal 5-char string `\x19` + 4-char `\n{}` instead of
/// a single 0x19 byte and a literal LF. Same class of bug as Solidity
/// BridgeMultisig S5-BM1. Rewritten to write the bytes directly, byte-correct
/// against `eth_sign` / `personal_sign` and standard hardware/software wallets.
pub fn personal_sign_hash(msg: &[u8]) -> H256 {
    let mut prefix = Vec::with_capacity(48 + msg.len());
    prefix.push(0x19);
    prefix.extend_from_slice(b"Ethereum Signed Message:\n");
    prefix.extend_from_slice(msg.len().to_string().as_bytes());
    keccak256_pair(&prefix, msg)
}

/// Compute the Keccak-256 of an EVM storage slot key.
/// slot_hash(address, slot_index) used in state trie.
pub fn storage_slot_key(address: &[u8; 20], slot: &[u8; 32]) -> H256 {
    let mut buf = [0u8; 52];
    buf[..20].copy_from_slice(address);
    buf[20..].copy_from_slice(slot);
    keccak256(&buf)
}

/// Create4 contract address: keccak256(0xff ++ deployer ++ salt ++ keccak256(init_code))[12..].
pub fn create2_address(deployer: &[u8; 20], salt: &[u8; 32], init_code: &[u8]) -> [u8; 20] {
    let code_hash = keccak256(init_code);
    let mut buf = [0u8; 85];
    buf[0] = 0xff;
    buf[1..21].copy_from_slice(deployer);
    buf[21..53].copy_from_slice(salt);
    buf[53..85].copy_from_slice(code_hash.as_bytes());
    let h = keccak256(&buf);
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&h.as_bytes()[12..]);
    addr
}

/// Classic CREATE address: keccak256(RLP([sender, nonce]))[12..].
///
/// Audit-2026-05-01 S7-CR2: previous implementation used `0xd0 | (21 + rlp_len)`
/// for the list prefix (works only by accident for u64 nonces) and `rlp_len`
/// underestimated the encoded byte count by one for nonce ≥ 256, producing
/// addresses that diverge from canonical Ethereum RLP. Rewritten to encode
/// the nonce first into a stack buffer so the list prefix exactly matches
/// the payload byte count.
pub fn create_address(sender: &[u8; 20], nonce: u64) -> [u8; 20] {
    // 1. Encode the nonce as RLP into a stack buffer.
    //    - nonce == 0  → single byte 0x80
    //    - nonce in 1..=0x7F → single byte = nonce
    //    - else → 0x80 + len, then big-endian bytes
    let mut nonce_buf = [0u8; 9];
    let nonce_len: usize = if nonce == 0 {
        nonce_buf[0] = 0x80;
        1
    } else if nonce < 0x80 {
        nonce_buf[0] = nonce as u8;
        1
    } else {
        let be = nonce.to_be_bytes();
        let start = be.iter().position(|&b| b != 0).unwrap_or(7);
        let payload = &be[start..];
        nonce_buf[0] = 0x80 + payload.len() as u8;
        nonce_buf[1..1 + payload.len()].copy_from_slice(payload);
        1 + payload.len()
    };

    // 2. Sender encodes as 0x94 || 20 bytes (string-of-len-20). Total = 21 bytes.
    //    Payload total = 21 + nonce_len. All ZBX address+nonce encodings fit
    //    in the short-list range (payload < 56), so list prefix is 0xc0 + len.
    let payload_len = 21 + nonce_len;
    debug_assert!(payload_len < 56, "RLP short-list invariant: 21 + nonce_len < 56");

    let mut buf = Vec::with_capacity(1 + payload_len);
    buf.push(0xc0 + payload_len as u8); // list prefix
    buf.push(0x80 + 20);                // sender string prefix (0x94)
    buf.extend_from_slice(sender);
    buf.extend_from_slice(&nonce_buf[..nonce_len]);

    let h = keccak256(&buf);
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&h.as_bytes()[12..]);
    addr
}

#[cfg(test)]
mod create_address_tests {
    use super::*;

    /// Cross-checked vector: sender=0x6ac7ea33f8831ea9dcc53393aaa88b25a785dbf0, nonce=0
    /// -> 0xcd234a471b72ba2f1ccf0a70fcaba648a5eecd8d (canonical Ethereum CREATE).
    #[test]
    fn create_addr_nonce_zero() {
        let sender = hex_lit("6ac7ea33f8831ea9dcc53393aaa88b25a785dbf0");
        let got = create_address(&sender, 0);
        let want = hex_lit_n::<20>("cd234a471b72ba2f1ccf0a70fcaba648a5eecd8d");
        assert_eq!(got, want);
    }

    /// nonce=1 vector.
    #[test]
    fn create_addr_nonce_one() {
        let sender = hex_lit("6ac7ea33f8831ea9dcc53393aaa88b25a785dbf0");
        let got = create_address(&sender, 1);
        let want = hex_lit_n::<20>("343c43a37d37dff08ae8c4a11544c718abb4fcf8");
        assert_eq!(got, want);
    }

    /// Smoke: nonce=256 encodes as `0x82 0x01 0x00` (3 nonce bytes), giving a
    /// payload of 21 + 3 = 24 bytes and list prefix `0xc0 + 24 = 0xd8`. Pre-fix
    /// `rlp_len` reported 2 nonce bytes while `encode_nonce` wrote 3 → keccak
    /// over a wrong-length buffer → wrong CREATE address.
    #[test]
    fn create_addr_nonce_256_does_not_panic_and_is_stable() {
        let sender = hex_lit("6ac7ea33f8831ea9dcc53393aaa88b25a785dbf0");
        let a = create_address(&sender, 256);
        let b = create_address(&sender, 256);
        assert_eq!(a, b);
        // sanity: must differ from nonce=255 and nonce=257
        assert_ne!(a, create_address(&sender, 255));
        assert_ne!(a, create_address(&sender, 257));
    }

    fn hex_lit(s: &str) -> [u8; 20] { hex_lit_n::<20>(s) }
    fn hex_lit_n<const N: usize>(s: &str) -> [u8; N] {
        let v = hex::decode(s).expect("valid hex");
        assert_eq!(v.len(), N);
        let mut a = [0u8; N];
        a.copy_from_slice(&v);
        a
    }
}