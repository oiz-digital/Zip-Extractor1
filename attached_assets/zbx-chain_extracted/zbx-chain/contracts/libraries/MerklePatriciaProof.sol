// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { RlpReader } from "./RlpReader.sol";

/// @title  MerklePatriciaProof — Ethereum-compatible MPT inclusion verifier.
/// @notice Verifies a Merkle-Patricia-Trie inclusion / exclusion proof
///         against a known root hash. Matches the reference Rust impl in
///         `zbx-chain/crates/zbx-trie/src/proof.rs` byte-for-byte and the
///         Ethereum yellow paper §4.1 (modified Merkle Patricia trie).
///
/// @dev    The MPT has three node kinds, distinguished by RLP item count:
///           - 2 items → leaf OR extension. The HP (hex-prefix) flag bits
///             on the first nibble of the path encode which:
///               bit 5 (0x20) = leaf flag
///               bit 4 (0x10) = odd-length flag
///           - 17 items → branch. Indices 0..15 are child references;
///             index 16 is an optional value-at-branch.
///
///         A child reference is either:
///           - a 32-byte hash (when the encoded child is ≥ 32 bytes), OR
///           - the inlined RLP-encoded child node (when < 32 bytes)
///
///         Verification walks node[i] → checks `keccak256(node[i]) ==
///         expectedHash` → consumes nibbles → updates expectedHash from
///         the chosen child reference. The last node must be a leaf (or
///         a branch whose value field equals `value`), and its remaining
///         path must consume exactly the remaining key nibbles.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:s28        Added in S28 — closes S26-FOLLOWUP-MPT-SOL
library MerklePatriciaProof {
    using RlpReader for RlpReader.RLPItem;
    using RlpReader for RlpReader.Iterator;

    error InvalidNodeHash();
    error InvalidNodeShape(uint256 itemCount);
    error PathConsumedButValueMismatch();
    error PathExhaustedAtNonLeaf();
    error InlineChildUnsupportedAtRoot();
    error EmptyChildAtBranch();
    error EmptyProofWithNonEmptyValue();
    /// @dev S28-FIX-MED-2 — HP (hex-prefix) compact-encoding canonicality.
    error MalformedCompactPath();
    error NonCanonicalEvenPathFlag();

    /// @notice Verify that the trie identified by `root` maps `key` to
    ///         `value`. To verify exclusion, pass `value.length == 0`.
    /// @param  root   Trie root hash.
    /// @param  key    Raw key bytes (will be decomposed into nibbles).
    /// @param  value  Expected value bytes (empty for exclusion proofs).
    /// @param  proof  Sequence of RLP-encoded MPT nodes from root → leaf.
    /// @return ok     True iff the proof is valid AND consistent with `value`.
    function verify(
        bytes32 root,
        bytes memory key,
        bytes memory value,
        bytes[] memory proof
    ) internal pure returns (bool ok) {
        // Empty trie special-case.
        if (proof.length == 0) {
            // Empty proof can only attest to absence (value == empty) and
            // requires the root to be the empty-string hash. We don't have
            // that constant inlined; the Eth empty root is keccak(rlp("")) =
            // 0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421.
            return value.length == 0 &&
                root == 0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421;
        }

        bytes memory pathNibbles = _bytesToNibbles(key);
        uint256 depth = 0;
        bytes32 expectedHash = root;

        for (uint256 i = 0; i < proof.length; ++i) {
            bytes memory encoded = proof[i];

            // Hash linkage check.
            if (keccak256(encoded) != expectedHash) revert InvalidNodeHash();

            RlpReader.RLPItem memory node = RlpReader.toRlpItem(encoded);
            uint256 itemCount = node.numItems();

            if (itemCount == 2) {
                // ── Leaf or Extension.
                RlpReader.Iterator memory it = node.iterator();
                bytes memory pathBytes = it.next().toBytes();
                RlpReader.RLPItem memory second = it.next();

                (bytes memory partial, bool isLeaf) = _decodeCompactPath(pathBytes);

                // The remaining key nibbles must start with `partial`.
                if (depth + partial.length > pathNibbles.length) {
                    return false;
                }
                for (uint256 j = 0; j < partial.length; ++j) {
                    if (pathNibbles[depth + j] != partial[j]) return false;
                }

                if (isLeaf) {
                    // Leaf: must consume the FULL remaining key, must
                    // match value exactly, AND must be the LAST proof
                    // node (S28-FIX-MED-3 — over-long proofs rejected).
                    if (i != proof.length - 1) return false;
                    if (depth + partial.length != pathNibbles.length) return false;
                    bytes memory leafValue = second.toBytes();
                    return _bytesEqual(leafValue, value);
                } else {
                    // Extension: advance depth and follow the child reference.
                    depth += partial.length;
                    expectedHash = _childHash(second, encoded);
                }
            } else if (itemCount == 17) {
                // ── Branch.
                RlpReader.Iterator memory it = node.iterator();

                if (depth == pathNibbles.length) {
                    // Path is fully consumed; the branch's value field
                    // (index 16) holds the answer. We must be at the LAST
                    // proof node — otherwise the proof is over-long.
                    if (i != proof.length - 1) return false;
                    // Skip 16 child fields.
                    for (uint256 j = 0; j < 16; ++j) it.next();
                    bytes memory branchValue = it.next().toBytes();
                    return _bytesEqual(branchValue, value);
                }

                uint8 branchIdx = uint8(pathNibbles[depth]);
                depth += 1;

                // Skip the first `branchIdx` siblings.
                for (uint256 j = 0; j < branchIdx; ++j) it.next();
                RlpReader.RLPItem memory child = it.next();

                // Empty child at the chosen branch ⇒ exclusion.
                // S28-FIX-MED-3: an empty-child terminal must also be
                // the LAST proof node — otherwise the proof carries
                // unreachable trailing nodes and is rejected.
                (, uint256 plen) = child.payloadLocation();
                if (plen == 0 && !child.isList()) {
                    if (i != proof.length - 1) return false;
                    return value.length == 0;
                }
                expectedHash = _childHash(child, encoded);
            } else {
                revert InvalidNodeShape(itemCount);
            }
        }

        // Walked every proof node without resolving — proof is incomplete.
        return false;
    }

    // ─── Internal helpers ─────────────────────────────────────────────────

    /// @dev Resolve a child reference. The MPT spec allows two forms:
    ///        - a 32-byte string ⇒ keccak hash of the next node
    ///        - an inlined node (payload < 32 bytes when the embedded
    ///          encoding fits). We compute its hash on the fly so the next
    ///          iteration's hash-linkage check works uniformly.
    ///      `parentEncoded` is the RLP bytes of the parent node — used
    ///      only for context in error reporting (kept unreferenced here).
    function _childHash(
        RlpReader.RLPItem memory child,
        bytes memory /*parentEncoded*/
    ) private pure returns (bytes32) {
        (uint256 payloadPtr, uint256 payloadLen) = child.payloadLocation();

        if (!child.isList() && payloadLen == 32) {
            // Direct hash reference.
            bytes32 h;
            assembly { h := mload(payloadPtr) }
            return h;
        }

        // Inlined node — keccak of the FULL RLP-encoded child item
        // (header + payload), not just its payload.
        bytes memory inlineEnc = new bytes(child.len);
        uint256 dst;
        assembly { dst := add(inlineEnc, 0x20) }
        _memcpy(child.memPtr, dst, child.len);
        return keccak256(inlineEnc);
    }

    /// @dev Convert a byte string to its 4-bit nibble decomposition.
    function _bytesToNibbles(bytes memory data) private pure returns (bytes memory nibs) {
        nibs = new bytes(data.length * 2);
        for (uint256 i = 0; i < data.length; ++i) {
            nibs[2 * i]     = bytes1(uint8(data[i]) >> 4);
            nibs[2 * i + 1] = bytes1(uint8(data[i]) & 0x0f);
        }
    }

    /// @dev Decode the HP (hex-prefix) compact encoding used by MPT
    ///      partial paths. The high nibble of the first byte holds two
    ///      flag bits:
    ///        bit 5 (0x20) → leaf
    ///        bit 4 (0x10) → odd-length path (the low nibble of byte 0
    ///                        is the FIRST path nibble)
    ///      Returns the path nibbles + whether this node is a leaf.
    ///
    /// @dev S28-FIX-MED-2 — strict canonicality rules per yellow paper:
    ///        - empty compact path is rejected (a 2-item node MUST carry
    ///          a meaningful HP-encoded path; the empty case is handled
    ///          separately via the branch node's value-at-branch slot)
    ///        - bits 6 and 7 of the first byte MUST be zero (only flag
    ///          bits 4 and 5 are defined; the upper two bits are reserved)
    ///        - for an EVEN-length path, the low nibble of byte 0 is a
    ///          padding placeholder and MUST be zero
    function _decodeCompactPath(bytes memory enc)
        private pure returns (bytes memory nibs, bool isLeaf)
    {
        if (enc.length == 0) revert MalformedCompactPath();

        uint8 flag = uint8(enc[0]);
        // Reserved high bits MUST be clear.
        if ((flag & 0xc0) != 0) revert NonCanonicalEvenPathFlag();
        isLeaf   = (flag & 0x20) != 0;
        bool odd = (flag & 0x10) != 0;
        // Even-length path: low nibble of flag byte is reserved zero pad.
        if (!odd && (flag & 0x0f) != 0) revert NonCanonicalEvenPathFlag();

        uint256 numNibs = (enc.length - 1) * 2 + (odd ? 1 : 0);
        nibs = new bytes(numNibs);

        uint256 outIdx = 0;
        if (odd) {
            nibs[outIdx++] = bytes1(flag & 0x0f);
        }
        for (uint256 i = 1; i < enc.length; ++i) {
            nibs[outIdx++] = bytes1(uint8(enc[i]) >> 4);
            nibs[outIdx++] = bytes1(uint8(enc[i]) & 0x0f);
        }
    }

    function _bytesEqual(bytes memory a, bytes memory b) private pure returns (bool) {
        if (a.length != b.length) return false;
        return keccak256(a) == keccak256(b);
    }

    /// @dev Word-aligned memcpy (raw memory pointers).
    function _memcpy(uint256 src, uint256 dst, uint256 len) private pure {
        uint256 wholeWords = len / 32;
        for (uint256 i = 0; i < wholeWords; ++i) {
            assembly { mstore(dst, mload(src)) }
            src += 32;
            dst += 32;
        }
        uint256 tail = len % 32;
        if (tail != 0) {
            uint256 mask = 256 ** (32 - tail) - 1;
            assembly {
                let srcPart := and(mload(src), not(mask))
                let dstPart := and(mload(dst), mask)
                mstore(dst, or(srcPart, dstPart))
            }
        }
    }
}
