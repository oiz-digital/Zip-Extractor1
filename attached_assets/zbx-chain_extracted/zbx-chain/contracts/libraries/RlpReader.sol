// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title  RlpReader — RLP (Recursive Length Prefix) decoder for Solidity.
/// @notice Pure, memory-only RLP decoder following the Ethereum yellow
///         paper appendix B. Used by `MerklePatriciaProof` to decode MPT
///         nodes and account-state values on-chain.
///
/// @dev    RLP encoding rules summary:
///         - Single byte in [0x00, 0x7f]                         → itself
///         - String, 0..55 bytes      → 0x80 + len, payload
///         - String, > 55 bytes       → 0xb7 + lenOfLen, lenBE, payload
///         - List, 0..55 bytes total  → 0xc0 + len, payload
///         - List, > 55 bytes total   → 0xf7 + lenOfLen, lenBE, payload
///
///         All decoders are STRICT — they reject:
///         - non-canonical length encodings (e.g. single-byte string
///           encoded as `0x81 0x42` instead of `0x42`)
///         - leading-zero integer encodings
///         - declared lengths that overflow the source slice
///         These are required to prevent prover-side malleability.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:s28        Added in S28 — Solidity MPT verifier
library RlpReader {
    // ─── Types ────────────────────────────────────────────────────────────

    /// @dev Pointer + length window into a `bytes memory` blob. The blob
    ///      itself is held alive by the caller; this struct is just an
    ///      offset/length pair (cheap to copy).
    struct RLPItem {
        uint256 memPtr;  // pointer to the FIRST header byte
        uint256 len;     // total length of the item (header + payload)
    }

    /// @dev Iterator over the payload of a list item.
    struct Iterator {
        RLPItem item;       // the parent list
        uint256 nextPtr;    // pointer to the next child's first header byte
    }

    // ─── Errors ───────────────────────────────────────────────────────────

    error InvalidRlpLength(uint256 want, uint256 got);
    error LeadingZeroInLength();
    error LeadingZeroInInteger();
    error NonCanonicalSingleByte();
    error NotAList();
    error NotAString();
    error UintOverflow();
    error IteratorExhausted();

    // ─── Entry: bytes memory → RLPItem ────────────────────────────────────

    /// @notice Wrap a `bytes memory` blob in an RLPItem covering its full
    ///         range. Callers MUST keep `data` alive for the lifetime of
    ///         every derived `RLPItem` — pointers reference its memory.
    function toRlpItem(bytes memory data) internal pure returns (RLPItem memory item) {
        if (data.length == 0) revert InvalidRlpLength(1, 0);
        uint256 ptr;
        // The first 32 bytes of `bytes memory` is the length; payload
        // starts at `data + 0x20`.
        assembly {
            ptr := add(data, 0x20)
        }
        item.memPtr = ptr;
        item.len    = data.length;
        // Sanity-check: the declared item length must equal the slice length.
        uint256 fullLen = _itemLength(ptr);
        if (fullLen != data.length) revert InvalidRlpLength(fullLen, data.length);
    }

    // ─── Header inspection ────────────────────────────────────────────────

    function isList(RLPItem memory item) internal pure returns (bool) {
        if (item.len == 0) return false;
        uint8 b0;
        uint256 ptr = item.memPtr;
        assembly { b0 := byte(0, mload(ptr)) }
        return b0 >= 0xc0;
    }

    /// @notice Pointer to the FIRST payload byte and its length.
    function payloadLocation(RLPItem memory item)
        internal pure returns (uint256 memPtr, uint256 len)
    {
        uint256 ptr = item.memPtr;
        uint8 b0;
        assembly { b0 := byte(0, mload(ptr)) }

        if (b0 < 0x80) {
            // single byte string
            memPtr = ptr;
            len    = 1;
        } else if (b0 < 0xb8) {
            // short string: header is 1 byte, payload is (b0 - 0x80) bytes
            memPtr = ptr + 1;
            len    = b0 - 0x80;
        } else if (b0 < 0xc0) {
            // long string: header is 1 + lenOfLen, then payload
            uint256 lol = b0 - 0xb7;
            memPtr = ptr + 1 + lol;
            len    = _readBigEndian(ptr + 1, lol);
        } else if (b0 < 0xf8) {
            // short list: header is 1, payload is (b0 - 0xc0) bytes
            memPtr = ptr + 1;
            len    = b0 - 0xc0;
        } else {
            // long list
            uint256 lol = b0 - 0xf7;
            memPtr = ptr + 1 + lol;
            len    = _readBigEndian(ptr + 1, lol);
        }
    }

    // ─── List iteration ───────────────────────────────────────────────────

    function iterator(RLPItem memory item) internal pure returns (Iterator memory it) {
        if (!isList(item)) revert NotAList();
        (uint256 payloadPtr, ) = payloadLocation(item);
        it.item    = item;
        it.nextPtr = payloadPtr;
    }

    function hasNext(Iterator memory it) internal pure returns (bool) {
        return it.nextPtr < it.item.memPtr + it.item.len;
    }

    function next(Iterator memory it) internal pure returns (RLPItem memory child) {
        if (!hasNext(it)) revert IteratorExhausted();
        uint256 childLen = _itemLength(it.nextPtr);
        child.memPtr = it.nextPtr;
        child.len    = childLen;
        it.nextPtr  += childLen;
    }

    /// @notice Count list children. Walks the list once.
    function numItems(RLPItem memory item) internal pure returns (uint256 count) {
        if (!isList(item)) revert NotAList();
        (uint256 payloadPtr, uint256 payloadLen) = payloadLocation(item);
        uint256 end = payloadPtr + payloadLen;
        uint256 ptr = payloadPtr;
        while (ptr < end) {
            ptr += _itemLength(ptr);
            count++;
        }
        if (ptr != end) revert InvalidRlpLength(end, ptr);
    }

    // ─── Payload extraction ───────────────────────────────────────────────

    /// @notice Extract the payload bytes of `item` as a fresh `bytes memory`.
    ///         Works for any string-shaped item (rejects lists).
    function toBytes(RLPItem memory item) internal pure returns (bytes memory out) {
        if (isList(item)) revert NotAString();
        (uint256 payloadPtr, uint256 payloadLen) = payloadLocation(item);
        out = new bytes(payloadLen);
        if (payloadLen != 0) {
            uint256 dst;
            assembly { dst := add(out, 0x20) }
            _memcpy(payloadPtr, dst, payloadLen);
        }
    }

    /// @notice Strict integer decode. Rejects leading-zero encodings as
    ///         required by the yellow paper: the canonical encoding of
    ///         `0` is `0x80` (empty string), so `0x00` is NON-canonical
    ///         and rejected. Likewise any multi-byte payload starting
    ///         with `0x00` is rejected. Payloads > 32 bytes overflow.
    /// @dev    S28-FIX-MED-1: previously `payloadLen > 1 && b0 == 0`
    ///         allowed single-byte `0x00`; tightened to reject all
    ///         leading-zero forms for any non-empty payload.
    function toUint(RLPItem memory item) internal pure returns (uint256 value) {
        if (isList(item)) revert NotAString();
        (uint256 payloadPtr, uint256 payloadLen) = payloadLocation(item);
        if (payloadLen == 0) return 0;                     // canonical "0" via 0x80
        if (payloadLen > 32) revert UintOverflow();
        uint8 b0;
        assembly { b0 := byte(0, mload(payloadPtr)) }
        // Strict: any non-empty integer with leading zero byte is
        // non-canonical (yellow paper §appendix B).
        if (b0 == 0) revert LeadingZeroInInteger();
        assembly {
            value := shr(mul(8, sub(32, payloadLen)), mload(payloadPtr))
        }
    }

    /// @notice Decode exactly 32 bytes (e.g. an MPT child hash).
    function toBytes32(RLPItem memory item) internal pure returns (bytes32 out) {
        if (isList(item)) revert NotAString();
        (uint256 payloadPtr, uint256 payloadLen) = payloadLocation(item);
        if (payloadLen != 32) revert InvalidRlpLength(32, payloadLen);
        assembly { out := mload(payloadPtr) }
    }

    /// @notice Length of the payload in bytes (without the header).
    function payloadLen(RLPItem memory item) internal pure returns (uint256 len) {
        ( , len) = payloadLocation(item);
    }

    // ─── Internal helpers ─────────────────────────────────────────────────

    /// @dev Compute the TOTAL length (header + payload) of the RLP item
    ///      starting at `ptr`. Performs canonical-encoding checks.
    function _itemLength(uint256 ptr) private pure returns (uint256 itemLen) {
        uint8 b0;
        assembly { b0 := byte(0, mload(ptr)) }

        if (b0 < 0x80) {
            // single byte
            itemLen = 1;
        } else if (b0 < 0xb8) {
            // short string [0x80, 0xb7]; payload len = b0 - 0x80, in [0, 55]
            uint256 strLen = b0 - 0x80;
            // Canonicality: a single byte in [0x00, 0x7f] must be encoded
            // as itself, NOT as `0x81 byte`.
            if (strLen == 1) {
                uint8 b1;
                assembly { b1 := byte(0, mload(add(ptr, 1))) }
                if (b1 < 0x80) revert NonCanonicalSingleByte();
            }
            itemLen = 1 + strLen;
        } else if (b0 < 0xc0) {
            // long string [0xb8, 0xbf]; lenOfLen = b0 - 0xb7, in [1, 8]
            uint256 lol = b0 - 0xb7;
            uint256 strLen = _readBigEndian(ptr + 1, lol);
            // Canonicality: long-string form is only legal when length > 55.
            if (strLen <= 55) revert InvalidRlpLength(56, strLen);
            itemLen = 1 + lol + strLen;
        } else if (b0 < 0xf8) {
            // short list
            itemLen = 1 + (b0 - 0xc0);
        } else {
            // long list
            uint256 lol = b0 - 0xf7;
            uint256 listLen = _readBigEndian(ptr + 1, lol);
            if (listLen <= 55) revert InvalidRlpLength(56, listLen);
            itemLen = 1 + lol + listLen;
        }
    }

    /// @dev Read a big-endian unsigned integer from `lol` bytes starting
    ///      at `ptr`. Rejects leading-zero encodings as required by the
    ///      yellow paper.
    function _readBigEndian(uint256 ptr, uint256 lol) private pure returns (uint256 val) {
        if (lol == 0 || lol > 8) revert InvalidRlpLength(8, lol);
        uint8 first;
        assembly { first := byte(0, mload(ptr)) }
        if (first == 0) revert LeadingZeroInLength();
        assembly {
            val := shr(mul(8, sub(32, lol)), mload(ptr))
        }
    }

    /// @dev Word-aligned memcpy. Both src and dst are RAW memory pointers
    ///      (NOT bytes memory headers).
    function _memcpy(uint256 src, uint256 dst, uint256 len) private pure {
        uint256 wholeWords = len / 32;
        for (uint256 i = 0; i < wholeWords; ++i) {
            assembly { mstore(dst, mload(src)) }
            src += 32;
            dst += 32;
        }
        uint256 tail = len % 32;
        if (tail != 0) {
            // mask: top `tail` bytes set
            uint256 mask = 256 ** (32 - tail) - 1;
            assembly {
                let srcPart := and(mload(src), not(mask))
                let dstPart := and(mload(dst), mask)
                mstore(dst, or(srcPart, dstPart))
            }
        }
    }
}
