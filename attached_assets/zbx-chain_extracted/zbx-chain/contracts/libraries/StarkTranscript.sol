// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { GoldilocksField } from "./GoldilocksField.sol";

/// @title  StarkTranscript — Keccak-based Fiat-Shamir transcript.
/// @notice The Fiat-Shamir transform turns a public-coin interactive proof
///         into a non-interactive one by sourcing the verifier's "random
///         challenges" from a hash of the prover's prior messages. Every
///         message the prover commits to MUST be `absorb`-ed before the
///         next challenge is `squeeze`-d, otherwise the prover could
///         influence its own challenges (a soundness break).
///
/// @dev    Transcript state is a single `bytes32` digest (keccak duplex).
///         Each `absorb(x)` updates state to `keccak256(state || x)`.
///         Each `squeeze()` returns `keccak256(state || counter)` and bumps
///         the counter, so back-to-back challenges are independent.
///
///         Why duplex-via-keccak rather than a native sponge?
///         - keccak256 is the only hash with a precompile-quality cost on
///           EVM (30 + 6·words gas).
///         - The duplex construction is what `merlin` and StarkWare's Cairo
///           verifier use; soundness bound is the standard random-oracle
///           argument.
///
///         The transcript is passed by value (memory struct) so that
///         downstream library functions can fork it without aliasing.
///         To preserve linear-history Fiat-Shamir semantics, callers MUST
///         either thread the returned struct through their call chain or
///         use the `Transcript storage` overload below.
///
/// @custom:s27  Added in S27 — STARK verifier framework
library StarkTranscript {
    struct Transcript {
        bytes32 state;
        uint64  counter; // increments on each `squeeze`
    }

    /// @notice Initialise a fresh transcript bound to a domain-separation tag.
    function init(bytes memory dst) internal pure returns (Transcript memory t) {
        t.state   = keccak256(abi.encodePacked("ZBX-STARK-v1::", dst));
        t.counter = 0;
    }

    // ─── Absorb ───────────────────────────────────────────────────────────

    /// @notice Absorb a 32-byte commitment (e.g. a Merkle root).
    function absorbBytes32(Transcript memory t, bytes32 x) internal pure {
        t.state   = keccak256(abi.encodePacked(t.state, x));
        t.counter = 0; // new commit invalidates challenge counter
    }

    /// @notice Absorb a single field element. We absorb the canonical 32-byte
    ///         left-padded representation so the wire format is unambiguous.
    function absorbFelt(Transcript memory t, uint256 x) internal pure {
        t.state   = keccak256(abi.encodePacked(t.state, x));
        t.counter = 0;
    }

    /// @notice Absorb a vector of field elements (e.g. public inputs, OOD
    ///         evaluations). Compact ABI-encoding keeps the wire format
    ///         deterministic.
    function absorbFelts(Transcript memory t, uint256[] memory xs) internal pure {
        t.state   = keccak256(abi.encodePacked(t.state, abi.encodePacked(xs)));
        t.counter = 0;
    }

    /// @notice Absorb arbitrary bytes (e.g. proof header).
    function absorbBytes(Transcript memory t, bytes memory x) internal pure {
        t.state   = keccak256(abi.encodePacked(t.state, x));
        t.counter = 0;
    }

    // ─── Squeeze ──────────────────────────────────────────────────────────

    /// @notice Produce a fresh challenge as a Goldilocks field element.
    /// @dev    `keccak256(state || counter)` reduced mod p. Counter prevents
    ///         duplicate challenges within a single absorb epoch.
    function challengeFelt(Transcript memory t) internal pure returns (uint256) {
        bytes32 c = keccak256(abi.encodePacked(t.state, t.counter));
        unchecked { t.counter += 1; }
        return GoldilocksField.reduce(uint256(c));
    }

    /// @notice Produce a vector of `n` independent field-element challenges.
    function challengeFelts(Transcript memory t, uint256 n)
        internal
        pure
        returns (uint256[] memory out)
    {
        out = new uint256[](n);
        for (uint256 i = 0; i < n; ++i) {
            out[i] = challengeFelt(t);
        }
    }

    /// @notice Produce a query index in `[0, domainSize)`.
    /// @dev    `domainSize` MUST be a power of two — STARK domains always
    ///         are. Using bitmask instead of `mod` preserves uniformity
    ///         (no modulo bias) and saves a `div`.
    function challengeQueryIndex(Transcript memory t, uint256 domainSize)
        internal
        pure
        returns (uint256)
    {
        require(domainSize != 0 && (domainSize & (domainSize - 1)) == 0,
                "transcript: domain not power of 2");
        bytes32 c = keccak256(abi.encodePacked(t.state, t.counter));
        unchecked { t.counter += 1; }
        return uint256(c) & (domainSize - 1);
    }
}
