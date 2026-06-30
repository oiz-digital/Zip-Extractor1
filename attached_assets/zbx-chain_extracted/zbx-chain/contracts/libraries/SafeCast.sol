// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title SafeCast — Safe downcasting for Solidity (no silent truncation).
/// @notice All casts revert on overflow instead of silently truncating.
///         Used throughout ZBX Chain DeFi contracts.

library SafeCast {

    error Overflow(uint256 value, uint256 maxValue);

    function toUint224(uint256 value) internal pure returns (uint224) {
        if (value > type(uint224).max) revert Overflow(value, type(uint224).max);
        return uint224(value);
    }

    function toUint128(uint256 value) internal pure returns (uint128) {
        if (value > type(uint128).max) revert Overflow(value, type(uint128).max);
        return uint128(value);
    }

    function toUint112(uint256 value) internal pure returns (uint112) {
        if (value > type(uint112).max) revert Overflow(value, type(uint112).max);
        return uint112(value);
    }

    function toUint96(uint256 value) internal pure returns (uint96) {
        if (value > type(uint96).max) revert Overflow(value, type(uint96).max);
        return uint96(value);
    }

    function toUint64(uint256 value) internal pure returns (uint64) {
        if (value > type(uint64).max) revert Overflow(value, type(uint64).max);
        return uint64(value);
    }

    function toUint32(uint256 value) internal pure returns (uint32) {
        if (value > type(uint32).max) revert Overflow(value, type(uint32).max);
        return uint32(value);
    }

    function toUint16(uint256 value) internal pure returns (uint16) {
        if (value > type(uint16).max) revert Overflow(value, type(uint16).max);
        return uint16(value);
    }

    function toUint8(uint256 value) internal pure returns (uint8) {
        if (value > type(uint8).max) revert Overflow(value, type(uint8).max);
        return uint8(value);
    }

    function toInt256(uint256 value) internal pure returns (int256) {
        if (value > uint256(type(int256).max)) revert Overflow(value, uint256(type(int256).max));
        return int256(value);
    }

    function toInt128(int256 value) internal pure returns (int128) {
        require(value >= type(int128).min && value <= type(int128).max, "SafeCast: int128 overflow");
        return int128(value);
    }

    function abs(int256 x) internal pure returns (uint256) {
        return x < 0 ? uint256(-x) : uint256(x);
    }
}