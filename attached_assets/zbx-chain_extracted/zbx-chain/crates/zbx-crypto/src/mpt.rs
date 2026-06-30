//! Ethereum-compatible Modified Patricia Merkle Trie (MPT).
//!
//! Implements the full MPT specification from the Ethereum Yellow Paper
//! (Appendix D) and matches the go-ethereum `trie` package behaviour.
//! Used for `transactions_root` and `receipts_root` in block headers so
//! that Ethereum SPV inclusion proofs work against ZBX Chain blocks.
//!
//! ## Node types
//! - **Empty**      – the empty subtrie.
//! - **Leaf**       – `[HP(path, leaf=true),  value]`  — a key-value terminus.
//! - **Extension**  – `[HP(path, leaf=false), child]`  — shared path prefix.
//! - **Branch**     – `[c0..c15, value]`               — fork on next nibble.
//!
//! ## Child encoding inside a parent node
//! `len(RLP(child)) < 32`  → inline the raw RLP bytes.
//! `len(RLP(child)) >= 32` → `keccak256(RLP(child))` as a 32-byte string.
//!
//! ## Root hash
//! Always `keccak256(RLP(root_node))`, even when the root encodes to fewer
//! than 32 bytes (the root is never inlined by its caller).
//!
//! ## Key format for transactions
//! Key at index `i` = `rlp_uint64(i)` (Ethereum's canonical integer RLP):
//!   0   → `0x80`         (RLP of empty byte string, nibbles `[8, 0]`)
//!   1   → `0x01`         (nibbles `[0, 1]`)
//!   127 → `0x7f`         (nibbles `[7, f]`)
//!   128 → `0x81 0x80`    (nibbles `[8, 1, 8, 0]`)
//!
//! Value = 32-byte transaction hash encoded as an RLP byte string (`0xa0 || hash`).
//!
//! Reference: Yellow Paper §D, EIP-2718, go-ethereum `trie/trie.go`.

use crate::keccak::keccak256;

// ─── Node ──────────────────────────────────────────────────────────────────────

/// An MPT node.
#[derive(Clone)]
enum Node {
    Empty,
    Leaf(Vec<u8>, Vec<u8>),                    // nibble-path, value bytes
    Extension(Vec<u8>, Box<Node>),             // nibble-path, child
    Branch([Box<Node>; 16], Option<Vec<u8>>),  // 16 children, optional value
}

impl Default for Box<Node> {
    fn default() -> Self { Box::new(Node::Empty) }
}

// ─── Insertion ─────────────────────────────────────────────────────────────────

fn insert(node: Node, path: &[u8], value: Vec<u8>) -> Node {
    match node {
        Node::Empty => Node::Leaf(path.to_vec(), value),

        Node::Leaf(ref ep, ref ev) => {
            let cl = common_len(path, ep);
            // Exact match → update in place.
            if cl == ep.len() && cl == path.len() {
                return Node::Leaf(ep.clone(), value);
            }
            // Diverge: build a branch at nibble position `cl`.
            let mut children: [Box<Node>; 16] = Default::default();
            let mut bval: Option<Vec<u8>> = None;

            if cl == ep.len() {
                bval = Some(ev.clone());
            } else {
                let idx = ep[cl] as usize;
                children[idx] = Box::new(Node::Leaf(ep[cl + 1..].to_vec(), ev.clone()));
            }
            if cl == path.len() {
                bval = Some(value);
            } else {
                let idx = path[cl] as usize;
                children[idx] = Box::new(Node::Leaf(path[cl + 1..].to_vec(), value));
            }
            wrap_with_extension(common_len(path, ep), ep, Node::Branch(children, bval))
        }

        Node::Extension(ref ep, child) => {
            let cl = common_len(path, ep);
            if cl == ep.len() {
                // Continue past the extension.
                return Node::Extension(
                    ep.clone(),
                    Box::new(insert(*child, &path[cl..], value)),
                );
            }
            // Split the extension at nibble `cl`.
            let mut children: [Box<Node>; 16] = Default::default();
            let mut bval: Option<Vec<u8>> = None;

            let ext_idx = ep[cl] as usize;
            let remainder = &ep[cl + 1..];
            children[ext_idx] = if remainder.is_empty() {
                child
            } else {
                Box::new(Node::Extension(remainder.to_vec(), child))
            };

            let new_path = &path[cl..];
            if new_path.is_empty() {
                bval = Some(value);
            } else {
                let idx = new_path[0] as usize;
                children[idx] = Box::new(Node::Leaf(new_path[1..].to_vec(), value));
            }
            wrap_with_extension(cl, ep, Node::Branch(children, bval))
        }

        Node::Branch(mut children, bval) => {
            if path.is_empty() {
                return Node::Branch(children, Some(value));
            }
            let idx = path[0] as usize;
            let child = std::mem::replace(&mut children[idx], Box::new(Node::Empty));
            children[idx] = Box::new(insert(*child, &path[1..], value));
            Node::Branch(children, bval)
        }
    }
}

/// Wrap `inner` in an Extension covering the first `cl` nibbles of `ep`,
/// or return it directly if `cl == 0`.
fn wrap_with_extension(cl: usize, ep: &[u8], inner: Node) -> Node {
    if cl > 0 {
        Node::Extension(ep[..cl].to_vec(), Box::new(inner))
    } else {
        inner
    }
}

/// Number of leading equal elements between two slices.
fn common_len(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count()
}

// ─── Encoding ──────────────────────────────────────────────────────────────────

/// Encode a node for embedding inside a parent (inline if < 32 bytes, else hash).
fn encode_node(node: &Node) -> Vec<u8> {
    let rlp = node_rlp(node);
    if rlp.len() < 32 {
        rlp
    } else {
        rlp_bytes(&keccak256(&rlp).0)
    }
}

/// Full RLP of a node (used both for `encode_node` and for the root hash).
fn node_rlp(node: &Node) -> Vec<u8> {
    match node {
        Node::Empty => vec![0x80],

        Node::Leaf(path, value) => {
            rlp_list(&[hp_encode(path, true), rlp_bytes(value)])
        }

        Node::Extension(path, child) => {
            rlp_list(&[hp_encode(path, false), encode_node(child)])
        }

        Node::Branch(children, value) => {
            let mut items: Vec<Vec<u8>> = children.iter().map(|c| encode_node(c)).collect();
            items.push(value.as_deref().map(rlp_bytes).unwrap_or_else(|| vec![0x80]));
            rlp_list(&items)
        }
    }
}

/// Root hash of the trie: always `keccak256(RLP(root))`.
fn root_hash(root: &Node) -> [u8; 32] {
    keccak256(&node_rlp(root)).0
}

// ─── HP (Hex-Prefix / compact) encoding ────────────────────────────────────────

/// Ethereum Hex-Prefix encoding of a nibble sequence.
///
/// Prefix nibble layout (first nibble of output byte):
/// | is_leaf | odd | prefix nibble |
/// |---------|-----|---------------|
/// | false   |  0  |      0x0      |
/// | false   |  1  |      0x1      |
/// | true    |  0  |      0x2      |
/// | true    |  1  |      0x3      |
///
/// Even length: first byte = `prefix_nibble << 4 | 0x0`, then pack remaining nibbles.
/// Odd  length: first byte = `prefix_nibble << 4 | nibbles[0]`, then pack the rest.
///
/// Returns the HP bytes wrapped as an RLP byte string.
fn hp_encode(nibbles: &[u8], is_leaf: bool) -> Vec<u8> {
    let prefix: u8 = if is_leaf { 2 } else { 0 };
    let odd = nibbles.len() % 2 == 1;
    let mut hp = Vec::with_capacity(1 + (nibbles.len() + 1) / 2);
    if odd {
        hp.push(((prefix + 1) << 4) | nibbles[0]);
        pack_nibbles(&mut hp, &nibbles[1..]);
    } else {
        hp.push(prefix << 4);
        pack_nibbles(&mut hp, nibbles);
    }
    rlp_bytes(&hp)
}

fn pack_nibbles(out: &mut Vec<u8>, nibbles: &[u8]) {
    for pair in nibbles.chunks(2) {
        out.push((pair[0] << 4) | pair[1]);
    }
}

// ─── Minimal RLP encoder ───────────────────────────────────────────────────────

/// RLP-encode a byte string.
fn rlp_bytes(data: &[u8]) -> Vec<u8> {
    let n = data.len();
    match n {
        0 => vec![0x80],
        1 if data[0] < 0x80 => vec![data[0]],
        1..=55 => {
            let mut out = Vec::with_capacity(1 + n);
            out.push(0x80 + n as u8);
            out.extend_from_slice(data);
            out
        }
        _ => {
            let len_enc = be_minimal(n);
            let mut out = Vec::with_capacity(1 + len_enc.len() + n);
            out.push(0xb7 + len_enc.len() as u8);
            out.extend_from_slice(&len_enc);
            out.extend_from_slice(data);
            out
        }
    }
}

/// RLP-encode a list of already-encoded items.
fn rlp_list(items: &[Vec<u8>]) -> Vec<u8> {
    let payload: Vec<u8> = items.iter().flat_map(|i| i.iter().copied()).collect();
    let n = payload.len();
    if n <= 55 {
        let mut out = Vec::with_capacity(1 + n);
        out.push(0xc0 + n as u8);
        out.extend_from_slice(&payload);
        out
    } else {
        let len_enc = be_minimal(n);
        let mut out = Vec::with_capacity(1 + len_enc.len() + n);
        out.push(0xf7 + len_enc.len() as u8);
        out.extend_from_slice(&len_enc);
        out.extend_from_slice(&payload);
        out
    }
}

/// Big-endian minimal encoding of `n` (strips leading zero bytes).
fn be_minimal(n: usize) -> Vec<u8> {
    let b = (n as u64).to_be_bytes();
    let first = b.iter().position(|&x| x != 0).unwrap_or(b.len() - 1);
    b[first..].to_vec()
}

// ─── Key encoding ──────────────────────────────────────────────────────────────

/// RLP-encode a transaction index as a minimal byte string.
///
/// Matches `rlp.EncodeToBytes(uint64(i))` from go-ethereum.
/// 0 → `[0x80]` (empty byte string), 1..127 → single byte, 128+ → multi-byte.
pub(crate) fn rlp_uint64(n: u64) -> Vec<u8> {
    if n == 0 {
        return vec![0x80];
    }
    let be = n.to_be_bytes();
    let first = be.iter().position(|&b| b != 0).unwrap_or(be.len() - 1);
    rlp_bytes(&be[first..])
}

/// Convert a byte slice to a nibble sequence (2 nibbles per byte).
fn to_nibbles(data: &[u8]) -> Vec<u8> {
    let mut nibs = Vec::with_capacity(data.len() * 2);
    for &b in data {
        nibs.push(b >> 4);
        nibs.push(b & 0x0f);
    }
    nibs
}

// ─── Public API ────────────────────────────────────────────────────────────────

/// Ethereum-compatible `transactions_root` for a sequence of 32-byte tx hashes.
///
/// Key(i)   = `rlp_uint64(i)` (Ethereum integer RLP).
/// Value(i) = `rlp_bytes(tx_hashes[i])` (32-byte hash as RLP byte string).
///
/// Returns:
/// - Empty list → Ethereum empty-trie root: `keccak256(0x80)`
///   (`0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421`).
/// - Non-empty → `keccak256(RLP(root_node))`.
pub fn transactions_root_mpt(tx_hashes: &[[u8; 32]]) -> [u8; 32] {
    if tx_hashes.is_empty() {
        return keccak256(&[0x80]).0;
    }
    let mut root = Node::Empty;
    for (i, hash) in tx_hashes.iter().enumerate() {
        let key = rlp_uint64(i as u64);
        let key_nibbles = to_nibbles(&key);
        let value = rlp_bytes(hash); // 32-byte hash → 0xa0 || 32 bytes
        root = insert(root, &key_nibbles, value);
    }
    root_hash(&root)
}

/// General-purpose Ethereum MPT root from arbitrary `(key, value)` byte pairs.
///
/// Inserts all pairs in the order given and returns `keccak256(RLP(root))`.
/// Callers are responsible for key ordering consistency.
pub fn trie_root_from_pairs(items: &[(&[u8], &[u8])]) -> [u8; 32] {
    if items.is_empty() {
        return keccak256(&[0x80]).0;
    }
    let mut root = Node::Empty;
    for (key, value) in items {
        let nibs = to_nibbles(key);
        root = insert(root, &nibs, value.to_vec());
    }
    root_hash(&root)
}

// ─── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Basic correctness ───────────────────────────────────────────────────────

    #[test]
    fn empty_trie_matches_ethereum_empty_root() {
        // Ethereum empty-trie root is keccak256(0x80).
        let got = transactions_root_mpt(&[]);
        let want = keccak256(&[0x80]).0;
        assert_eq!(got, want, "empty trie root must match Ethereum constant");
    }

    #[test]
    fn empty_trie_matches_block_header_constant() {
        // 0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421
        let r = transactions_root_mpt(&[]);
        assert_eq!(r[0], 0x56);
        assert_eq!(r[1], 0xe8);
        assert_eq!(r[31], 0x21);
    }

    #[test]
    fn single_tx_is_deterministic() {
        let h = [0xab; 32];
        let r1 = transactions_root_mpt(&[h]);
        let r2 = transactions_root_mpt(&[h]);
        assert_eq!(r1, r2);
        // Must not equal empty-trie root.
        assert_ne!(r1, keccak256(&[0x80]).0);
    }

    #[test]
    fn two_txs_deterministic() {
        let h0 = [0x11; 32];
        let h1 = [0x22; 32];
        let r1 = transactions_root_mpt(&[h0, h1]);
        let r2 = transactions_root_mpt(&[h0, h1]);
        assert_eq!(r1, r2);
    }

    #[test]
    fn different_tx_set_different_root() {
        let a = transactions_root_mpt(&[[1u8; 32], [2u8; 32]]);
        let b = transactions_root_mpt(&[[1u8; 32], [3u8; 32]]);
        assert_ne!(a, b, "distinct tx sets must produce distinct roots");
    }

    #[test]
    fn order_matters() {
        // Swapping tx order must change the root (keys are position-based).
        let h0 = [0x11; 32];
        let h1 = [0x22; 32];
        let fwd = transactions_root_mpt(&[h0, h1]);
        let rev = transactions_root_mpt(&[h1, h0]);
        assert_ne!(fwd, rev, "tx order must affect the MPT root");
    }

    #[test]
    fn deterministic_100_txs() {
        let hashes: Vec<[u8; 32]> = (0u64..100)
            .map(|i| {
                let mut h = [0u8; 32];
                h[..8].copy_from_slice(&i.to_be_bytes());
                h
            })
            .collect();
        assert_eq!(
            transactions_root_mpt(&hashes),
            transactions_root_mpt(&hashes)
        );
    }

    #[test]
    fn deterministic_1000_txs() {
        let hashes: Vec<[u8; 32]> = (0u64..1000)
            .map(|i| {
                let mut h = [0u8; 32];
                h[..8].copy_from_slice(&i.to_be_bytes());
                h
            })
            .collect();
        let r1 = transactions_root_mpt(&hashes);
        let r2 = transactions_root_mpt(&hashes);
        assert_eq!(r1, r2, "1000-tx MPT must be deterministic");
    }

    // ── Key encoding ────────────────────────────────────────────────────────────

    #[test]
    fn rlp_uint64_boundary_values() {
        assert_eq!(rlp_uint64(0),   vec![0x80]);           // empty byte string
        assert_eq!(rlp_uint64(1),   vec![0x01]);           // single byte < 0x80
        assert_eq!(rlp_uint64(127), vec![0x7f]);           // largest 1-byte key
        assert_eq!(rlp_uint64(128), vec![0x81, 0x80]);     // 2-byte: length 1, payload 0x80
        assert_eq!(rlp_uint64(256), vec![0x82, 0x01, 0x00]); // 2-byte payload
    }

    #[test]
    fn nibble_conversion() {
        // index 0  → key 0x80         → nibbles [8, 0]
        assert_eq!(to_nibbles(&rlp_uint64(0)),   vec![8, 0]);
        // index 1  → key 0x01         → nibbles [0, 1]
        assert_eq!(to_nibbles(&rlp_uint64(1)),   vec![0, 1]);
        // index 15 → key 0x0f         → nibbles [0, 15]
        assert_eq!(to_nibbles(&rlp_uint64(15)),  vec![0, 15]);
        // index 128 → key 0x81 0x80   → nibbles [8,1,8,0]
        assert_eq!(to_nibbles(&rlp_uint64(128)), vec![8, 1, 8, 0]);
    }

    // ── RLP correctness ─────────────────────────────────────────────────────────

    #[test]
    fn rlp_bytes_single_zero() {
        assert_eq!(rlp_bytes(&[0x00]), vec![0x00]); // single byte < 0x80 encoded as-is
    }

    #[test]
    fn rlp_bytes_empty() {
        assert_eq!(rlp_bytes(&[]), vec![0x80]);
    }

    #[test]
    fn rlp_bytes_32() {
        let data = [0xde; 32];
        let enc = rlp_bytes(&data);
        assert_eq!(enc[0], 0xa0); // 0x80 + 32
        assert_eq!(&enc[1..], &data);
    }

    // ── HP encoding ─────────────────────────────────────────────────────────────

    #[test]
    fn hp_encode_even_leaf() {
        // Even, leaf: prefix byte = 0x20, then packed nibbles.
        let nibs: &[u8] = &[1, 2, 3, 4];
        let hp = hp_encode(nibs, true);
        // hp bytes = [0x20, 0x12, 0x34], then rlp_bytes wraps it.
        let inner = &[0x20u8, 0x12, 0x34];
        assert_eq!(hp, rlp_bytes(inner));
    }

    #[test]
    fn hp_encode_odd_extension() {
        // Odd, extension: prefix byte = 0x11 (first nibble in low), then rest.
        let nibs: &[u8] = &[1, 2, 3];
        let hp = hp_encode(nibs, false);
        // inner bytes = [0x11, 0x23]
        let inner = &[0x11u8, 0x23];
        assert_eq!(hp, rlp_bytes(inner));
    }
}
