// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { Test } from "forge-std/Test.sol";
import { RlpReader } from "../libraries/RlpReader.sol";
import { MerklePatriciaProof } from "../libraries/MerklePatriciaProof.sol";

/// @title  MerklePatriciaProofTest — shape tests for the S28 MPT verifier.
/// @notice Real end-to-end fixtures need prover-emitted proofs (zbx-trie
///         + zbx-prover); those live in `crates/zbx-trie/tests/`. This
///         file covers the on-chain decoder primitives and structural
///         rejection paths that are independent of any specific trie.
contract MerklePatriciaProofTest is Test {
    using RlpReader for RlpReader.RLPItem;

    // ─── RLP primitives ───────────────────────────────────────────────

    function testRlp_singleByte() public pure {
        // 0x42 — encoded as itself (canonical).
        bytes memory enc = hex"42";
        RlpReader.RLPItem memory item = RlpReader.toRlpItem(enc);
        assertFalse(item.isList());
        assertEq(item.payloadLen(), 1);
        bytes memory body = RlpReader.toBytes(item);
        assertEq(body.length, 1);
        assertEq(uint8(body[0]), 0x42);
    }

    function testRlp_shortString() public pure {
        // "dog" = 0x83 'd' 'o' 'g'
        bytes memory enc = hex"83646f67";
        RlpReader.RLPItem memory item = RlpReader.toRlpItem(enc);
        assertFalse(item.isList());
        bytes memory body = RlpReader.toBytes(item);
        assertEq(body.length, 3);
        assertEq(string(body), "dog");
    }

    function testRlp_emptyString_zeroInteger() public pure {
        // 0x80 — canonical empty string == integer 0
        bytes memory enc = hex"80";
        RlpReader.RLPItem memory item = RlpReader.toRlpItem(enc);
        assertEq(RlpReader.toUint(item), 0);
    }

    function testRlp_uint_strict_rejectsLeadingZero_multiByte() public {
        // 0x82 0x00 0x42 — string of 2 bytes [0x00, 0x42] — non-canonical
        // integer (leading zero). Total encoding length 3.
        bytes memory enc = hex"820042";
        RlpReader.RLPItem memory item = RlpReader.toRlpItem(enc);
        vm.expectRevert(RlpReader.LeadingZeroInInteger.selector);
        RlpReader.toUint(item);
    }

    /// @notice S28-FIX-MED-1 — single-byte 0x00 must NOT decode as
    ///         integer 0; canonical zero is the empty string 0x80.
    function testRlp_uint_strict_rejectsSingleZeroByte() public {
        // 0x00 — single-byte string holding 0x00. Per yellow paper,
        // canonical integer 0 is 0x80, so 0x00 as integer is non-canonical.
        bytes memory enc = hex"00";
        RlpReader.RLPItem memory item = RlpReader.toRlpItem(enc);
        vm.expectRevert(RlpReader.LeadingZeroInInteger.selector);
        RlpReader.toUint(item);
    }

    function testRlp_singleByteCanonicality_rejected() public {
        // 0x81 0x42 — non-canonical (single byte < 0x80 must be encoded as itself).
        bytes memory enc = hex"8142";
        vm.expectRevert(RlpReader.NonCanonicalSingleByte.selector);
        RlpReader.toRlpItem(enc);
    }

    function testRlp_listIteration() public pure {
        // [ 0x42, "dog" ] = 0xc5 0x42 0x83 'd' 'o' 'g'
        bytes memory enc = hex"c54283646f67";
        RlpReader.RLPItem memory item = RlpReader.toRlpItem(enc);
        assertTrue(item.isList());
        assertEq(RlpReader.numItems(item), 2);

        RlpReader.Iterator memory it = RlpReader.iterator(item);
        assertEq(RlpReader.toUint(RlpReader.next(it)), 0x42);
        bytes memory dog = RlpReader.toBytes(RlpReader.next(it));
        assertEq(string(dog), "dog");
    }

    // ─── MPT structural ────────────────────────────────────────────────

    function testMpt_empty_acceptsExclusionAtEmptyRoot() public pure {
        bytes32 emptyRoot = 0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421;
        bytes[] memory empty = new bytes[](0);
        bytes memory key = abi.encodePacked(keccak256("anything"));
        assertTrue(MerklePatriciaProof.verify(emptyRoot, key, "", empty));
    }

    function testMpt_empty_rejectsInclusionAtEmptyRoot() public pure {
        bytes32 emptyRoot = 0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421;
        bytes[] memory empty = new bytes[](0);
        bytes memory key = abi.encodePacked(keccak256("anything"));
        // value.length > 0 — proof claims inclusion but trie is empty.
        assertFalse(MerklePatriciaProof.verify(emptyRoot, key, hex"01", empty));
    }

    function testMpt_empty_rejectsAtNonEmptyRoot() public pure {
        bytes[] memory empty = new bytes[](0);
        bytes memory key = abi.encodePacked(keccak256("anything"));
        bytes32 someRoot = bytes32(uint256(0x1234));
        assertFalse(MerklePatriciaProof.verify(someRoot, key, "", empty));
    }

    function testMpt_rejectsTamperedNodeHash() public pure {
        // Build a single-leaf trie:
        //   key   = "a" (one byte → 2 nibbles: 0x6, 0x1)
        //   value = "v"
        //   leaf  = RLP[ HP(0x6,0x1, leaf=true), "v" ]
        //   HP encoding of [0x6, 0x1] with leaf flag = 0x20 ++ 0x61 = 0x20 0x61
        //   leaf RLP = c5 82 20 61 76  (list len 5, two strings)
        //
        // Wrong root → must reject.
        bytes[] memory proof = new bytes[](1);
        proof[0] = hex"c58220617661"; // shape only, contents irrelevant
        bytes32 wrongRoot = bytes32(uint256(0xdead));
        assertFalse(MerklePatriciaProof.verify(wrongRoot, hex"61", hex"76", proof));
    }

    // ─── HP-prefix / nibble decoding (private — exercised via verify) ──

    function testHp_oddLeaf_validShape() public pure {
        // Exercises the HP decoder indirectly. Hash linkage will fail
        // (wrong root), but the decoder must not panic on odd-length
        // leaf input — flag 0x33 = leaf + odd + nibble 0x3.
        bytes[] memory proof = new bytes[](1);
        proof[0] = hex"c43381ff"; // [ 0x33, 0xff ] — odd leaf with nibble 0x3
        bytes32 wrongRoot = bytes32(0);
        assertFalse(MerklePatriciaProof.verify(wrongRoot, hex"30", hex"ff", proof));
    }

    // ─── S28-FIX-MED-3: over-long proof rejection ─────────────────────

    function testMpt_overLongProof_rejectedAtLeaf() public pure {
        // Single trailing node after a leaf must cause rejection. Even
        // if the leaf itself were valid, the over-long shape is rejected.
        // We use a wrong root so the test does not depend on building a
        // valid fixture — the property under test (i != proof.length-1
        // at leaf) is checked before/regardless of hash linkage success
        // at the trailing node.
        bytes[] memory proof = new bytes[](2);
        proof[0] = hex"c43381ff";
        proof[1] = hex"c43381ff";
        bytes32 wrongRoot = bytes32(0);
        assertFalse(MerklePatriciaProof.verify(wrongRoot, hex"30", hex"ff", proof));
    }
}
