// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title  GoldilocksField — Modular arithmetic over the Goldilocks prime.
/// @author Zebvix Labs
/// @notice The Goldilocks prime `p = 2^64 - 2^32 + 1 = 0xFFFFFFFF00000001`
///         is the standard scalar field for modern zkVM-style provers
///         (Plonky2/3, Risc0, SP1) and is the field used by the Zebvix
///         `zbx-prover` STARK pipeline.
///
/// @dev    Why Goldilocks (not BN254 Fr)?
///         - 64-bit field elements fit cleanly in `uint256`, so we never
///           need 256-bit modmul; native `mulmod` is safe.
///         - Sparse modulus enables fast pseudo-Mersenne reduction off-chain
///           (we don't exploit it on-chain — Solidity `mulmod` is already
///           constant-time and the EVM lacks 128-bit muls anyway).
///         - Matches the framework's STARK toolchain so prover + verifier
///           speak the same field with no marshalling glue.
///
///         All operations operate on canonical representatives in `[0, p)`.
///         Inputs are assumed to be already reduced; `reduce` is provided
///         for cases where inputs come from `uint256` chunks (e.g. Fiat-
///         Shamir challenges where the raw 256-bit hash is reduced mod p).
///
/// @custom:s27  Added in S27 — STARK verifier framework
library GoldilocksField {
    /// @dev Goldilocks prime: 2^64 - 2^32 + 1.
    uint256 internal constant P = 0xFFFFFFFF00000001;

    /// @dev p − 2 — used for Fermat's-little-theorem inverse: a^(p−2) ≡ a^(−1).
    uint256 internal constant P_MINUS_2 = 0xFFFFFFFEFFFFFFFF;

    error NotInField(uint256 value);
    error InverseOfZero();

    /// @notice Returns `true` iff `x < p`.
    function isCanonical(uint256 x) internal pure returns (bool) {
        return x < P;
    }

    /// @notice Reduces an arbitrary `uint256` to its canonical representative
    ///         in `[0, p)`. Use after Fiat-Shamir to convert keccak output to
    ///         a field element.
    /// @dev    Uses `mulmod(x, 1, P) == x mod P`, the cheapest non-trivial
    ///         reduction available in EVM. (`mod` opcode is also fine; the
    ///         compiler folds both equivalently.)
    function reduce(uint256 x) internal pure returns (uint256) {
        return x % P;
    }

    /// @notice Modular addition: `(a + b) mod p`.
    function add(uint256 a, uint256 b) internal pure returns (uint256) {
        return addmod(a, b, P);
    }

    /// @notice Modular subtraction: `(a − b) mod p`.
    /// @dev    `(a + (p − b)) mod p` keeps everything in `[0, 2p)` before the
    ///         final reduction, which `addmod` handles in one step.
    function sub(uint256 a, uint256 b) internal pure returns (uint256) {
        if (b == 0) return a;
        return addmod(a, P - b, P);
    }

    /// @notice Modular negation: `(p − a) mod p`.
    function neg(uint256 a) internal pure returns (uint256) {
        if (a == 0) return 0;
        return P - a;
    }

    /// @notice Modular multiplication: `(a · b) mod p`.
    function mul(uint256 a, uint256 b) internal pure returns (uint256) {
        return mulmod(a, b, P);
    }

    /// @notice Modular exponentiation by square-and-multiply.
    /// @dev    Loop terminates in ≤ 64 iterations because the field exponent
    ///         is bounded by `p − 1 < 2^64`. We do NOT use the EVM `expmod`
    ///         precompile (0x05) — it is calibrated for >= 32-byte exponents
    ///         and would be wasteful for our 64-bit-bounded exponents.
    function pow(uint256 base, uint256 exp) internal pure returns (uint256 result) {
        result = 1;
        uint256 b = base % P;
        uint256 e = exp;
        while (e != 0) {
            if (e & 1 == 1) {
                result = mulmod(result, b, P);
            }
            b = mulmod(b, b, P);
            e >>= 1;
        }
    }

    /// @notice Modular inverse via Fermat's little theorem: `a^(p−2) ≡ a^(−1)`.
    /// @dev    Reverts on input `0` (no inverse exists). Cost ≈ 64 mulmods.
    function inv(uint256 a) internal pure returns (uint256) {
        if (a == 0) revert InverseOfZero();
        return pow(a, P_MINUS_2);
    }

    /// @notice Modular division: `a / b ≡ a · b^(−1) (mod p)`.
    function div(uint256 a, uint256 b) internal pure returns (uint256) {
        return mulmod(a, inv(b), P);
    }

    /// @notice Asserts that `x` is in canonical form `[0, p)` and returns it.
    function checked(uint256 x) internal pure returns (uint256) {
        if (x >= P) revert NotInField(x);
        return x;
    }

    /// @notice Decode 8 bytes (big-endian) as a field element. The result is
    ///         already in `[0, 2^64)` so we only need to reject the unique
    ///         non-canonical encoding `2^64 - 2^32` ≤ x < 2^64 that lies
    ///         outside `[0, p)` once you account for `p = 2^64 - 2^32 + 1`.
    function fromBytes8BE(bytes8 b) internal pure returns (uint256 out) {
        out = uint64(b);
        if (out >= P) revert NotInField(out);
    }
}
