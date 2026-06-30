// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title FixedPoint — Fixed-point math library for ZBX Chain DeFi.
/// @notice 112.112 and 128.128 fixed-point arithmetic for AMM price calculations.
///         Avoids floating-point and minimises overflow/underflow risks.
///
/// @dev   Format: Q112.112 means 112 bits integer + 112 bits fractional.
///         Used in ZbxAMM for price accumulation (TWAP).

library FixedPoint {

    // ─── Q112.112 (used for price accumulators) ────────────────────────────

    uint8  public constant RESOLUTION = 112;
    uint256 public constant Q112      = 0x10000000000000000000000000000; // 2^112

    struct uq112x112 { uint224 _x; }
    struct uq144x112 { uint256 _x; }

    /// @dev Encode a uint112 as a UQ112.112.
    function encode(uint112 y) internal pure returns (uq112x112 memory) {
        return uq112x112(uint224(y) * uint224(Q112));
    }

    /// @dev Encode a uint144 as a UQ144.112.
    function encode144(uint144 y) internal pure returns (uq144x112 memory) {
        return uq144x112(uint256(y) * Q112);
    }

    /// @dev Divide a UQ112.112 by a uint112, returning a UQ112.112.
    function div(uq112x112 memory self, uint112 x) internal pure returns (uq112x112 memory) {
        require(x != 0, "FixedPoint: DIV_BY_ZERO");
        return uq112x112(self._x / uint224(x));
    }

    /// @dev Multiply a UQ112.112 by a uint, returning a UQ144.112.
    function mul(uq112x112 memory self, uint256 y) internal pure returns (uq144x112 memory) {
        uint256 z;
        require((z = uint256(self._x) * y) / y == uint256(self._x), "FixedPoint: MULTIPLICATION_OVERFLOW");
        return uq144x112(z);
    }

    /// @dev Returns the decoded integer part of a UQ112.112.
    function decode(uq112x112 memory self) internal pure returns (uint112) {
        return uint112(self._x >> RESOLUTION);
    }

    /// @dev Returns the decoded integer part of a UQ144.112.
    function decode144(uq144x112 memory self) internal pure returns (uint144) {
        return uint144(self._x >> RESOLUTION);
    }

    /// @dev Compute the square root of a UQ112.112 (for geometric mean price).
    function sqrt(uq112x112 memory self) internal pure returns (uq112x112 memory) {
        return uq112x112(_sqrt(uint256(self._x)));
    }

    function _sqrt(uint256 y) private pure returns (uint224) {
        if (y == 0) return 0;
        uint256 z = y;
        uint256 x = y / 2 + 1;
        while (x < z) { z = x; x = (y / x + x) / 2; }
        return uint224(z);
    }

    // ─── Q128.128 (used for precise fee/reward calculations) ──────────────

    uint256 public constant Q128 = 0x100000000000000000000000000000000; // 2^128

    function mulQ128(uint256 a, uint256 b) internal pure returns (uint256) {
        return (a * b) >> 128;
    }

    function divQ128(uint256 a, uint256 b) internal pure returns (uint256) {
        require(b > 0, "FixedPoint: DIV_BY_ZERO");
        return (a << 128) / b;
    }
}