// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title  StarkMerkle — Binary Merkle proof verification (keccak256).
/// @notice STARK provers commit to evaluation vectors via binary Merkle
///         trees over keccak256. The verifier needs to authenticate query
///         openings against the committed roots.
///
/// @dev    Tree shape:
///           - leaves are at the bottom, indexed 0..(2^h − 1)
///           - parent(i) = keccak256(left || right) — **not** sorted-pair,
///             because we need positional information for the open()/verify()
///             round-trip in FRI's coset-pair authentication
///
///         Why not sorted-pair (OZ-style)?
///         Sorted-pair Merkle trees are convenient for membership proofs but
///         lose positional information. FRI requires `f(x)` and `f(-x)`
///         openings whose positions in the tree encode the coset relation;
///         sorted-pair concat would scramble that.
///
///         Why not Poseidon?
///         keccak256 has the cheapest EVM cost (30 + 6·words gas) and the
///         prover's existing Rust impl in `zbx-prover` already uses keccak
///         for STARK Merkle trees. Switching to Poseidon would require a
///         pairing-precompile-cost round trip and an off-chain prover
///         change — both deferred until benchmarks justify them.
///
/// @custom:s27  Added in S27 — STARK verifier framework
library StarkMerkle {
    error PathTooLong();
    error InvalidIndex(uint256 index, uint256 domainSize);

    /// @notice Verify a Merkle path proves `leafHash` is at position
    ///         `leafIndex` in a tree of depth `path.length` rooted at `root`.
    /// @param  root      Committed root.
    /// @param  leafIndex Position of the leaf (0-indexed).
    /// @param  leafHash  keccak256 hash of the (already-encoded) leaf data.
    /// @param  path      Sibling hashes from leaf level upward (length = depth).
    /// @return ok        True iff the recomputed root equals `root`.
    function verify(
        bytes32 root,
        uint256 leafIndex,
        bytes32 leafHash,
        bytes32[] calldata path
    ) internal pure returns (bool ok) {
        if (path.length > 64) revert PathTooLong(); // 2^64 leaves is plenty
        uint256 maxIdx = uint256(1) << path.length;
        if (leafIndex >= maxIdx) revert InvalidIndex(leafIndex, maxIdx);

        bytes32 node = leafHash;
        uint256 idx  = leafIndex;
        for (uint256 i = 0; i < path.length; ++i) {
            bytes32 sib = path[i];
            // Bit 0 of `idx` decides whether `node` is the left or right
            // child at this level. Concat is positional, NOT sorted.
            if (idx & 1 == 0) {
                node = keccak256(abi.encodePacked(node, sib));
            } else {
                node = keccak256(abi.encodePacked(sib, node));
            }
            idx >>= 1;
        }
        ok = node == root;
    }

    /// @notice Hash a leaf payload (uniform layout helper).
    /// @dev    Leaves carry a vector of field elements (e.g. column values
    ///         at one row, or layer values at one query position). The
    ///         encoding `keccak256(abi.encodePacked(values))` matches the
    ///         convention used by the Rust prover in `zbx-prover/src/
    ///         merkle.rs::leaf_hash`.
    function leafHash(uint256[] memory values) internal pure returns (bytes32) {
        return keccak256(abi.encodePacked(values));
    }

    /// @notice Convenience: hash a single value as a leaf (single-column
    ///         FRI layers use this).
    function leafHashSingle(uint256 v) internal pure returns (bytes32) {
        return keccak256(abi.encodePacked(v));
    }
}
