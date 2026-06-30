// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { ZbxAMM } from "./ZbxAMM.sol";

/// @title ZbxAMMFactory — Permissionless Uniswap-V2-style pair factory
/// @author Zebvix Technologies Pvt Ltd
/// @notice Deploys one canonical ZbxAMM pair per (token0, token1) pair.
///         Anyone can call `createPair` — there is no admin gating, no
///         fee, no upgrade key. The pair contract itself is also fully
///         decentralised (no owner, no pause).
///
/// @dev    CREATE2-deterministic addresses: pair address depends only on
///         (factory, token0, token1) so every observer can independently
///         reconstruct the pair address without reading factory storage.
///
///         Tokens are sorted (token0 < token1) so the pair has a single
///         canonical orientation; `getPair(A,B) == getPair(B,A)`.
///
/// @custom:zbx-chain  Chain ID 8989
contract ZbxAMMFactory {

    // ─── Errors ───────────────────────────────────────────────────────────

    error IdenticalAddresses();
    error ZeroAddress();
    error PairExists();
    error PairCreateFailed();

    // ─── Events ───────────────────────────────────────────────────────────

    event PairCreated(
        address indexed token0,
        address indexed token1,
        address         pair,
        uint256         pairIndex
    );

    // ─── Storage ──────────────────────────────────────────────────────────

    /// @notice Sorted-pair lookup: getPair[token0][token1] == getPair[token1][token0].
    mapping(address => mapping(address => address)) public getPair;

    /// @notice Append-only registry of every pair ever created.
    address[] public allPairs;

    // ─── Pair creation ────────────────────────────────────────────────────

    /// @notice Deploy a new ZbxAMM pair for (tokenA, tokenB).
    /// @dev Deterministic address via CREATE2 with salt = keccak(token0,token1).
    ///      Reverts if a pair already exists for the (sorted) pair.
    function createPair(address tokenA, address tokenB) external returns (address pair) {
        if (tokenA == tokenB) revert IdenticalAddresses();
        (address token0, address token1) = tokenA < tokenB
            ? (tokenA, tokenB)
            : (tokenB, tokenA);
        if (token0 == address(0)) revert ZeroAddress();
        if (getPair[token0][token1] != address(0)) revert PairExists();

        bytes memory bytecode = abi.encodePacked(
            type(ZbxAMM).creationCode,
            abi.encode(token0, token1)
        );
        bytes32 salt = keccak256(abi.encodePacked(token0, token1));
        // S25-Y3 assembly: CREATE2 deploy of new ZbxAMM pair contract.
        // - `add(bytecode, 32)` skips the 32-byte length prefix to point at code start.
        // - `mload(bytecode)` reads that length prefix as the deploy size.
        // - Deterministic salt = keccak256(token0,token1) → same pair address on every chain.
        // Solidity has no equivalent high-level CREATE2 with dynamic bytecode; assembly required.
        assembly {
            pair := create2(0, add(bytecode, 32), mload(bytecode), salt)
        }
        if (pair == address(0)) revert PairCreateFailed();

        // Populate both lookup directions so callers don't have to sort.
        getPair[token0][token1] = pair;
        getPair[token1][token0] = pair;
        allPairs.push(pair);

        emit PairCreated(token0, token1, pair, allPairs.length - 1);
    }

    // ─── Views ────────────────────────────────────────────────────────────

    function allPairsLength() external view returns (uint256) {
        return allPairs.length;
    }

    /// @notice Returns the keccak256 of the ZbxAMM creation bytecode.
    ///         Off-chain tools (subgraphs, explorers) use this to verify
    ///         deterministic pair addresses without reading factory storage.
    ///         Must be recomputed after any ZbxAMM bytecode change.
    function getInitHash() external pure returns (bytes32) {
        return keccak256(type(ZbxAMM).creationCode);
    }

    /// @notice Predict the deterministic address a pair would deploy to.
    /// @dev Off-chain analytics use this to reconstruct addresses without
    ///      reading factory storage. Caller is responsible for sorting.
    function predictPair(address tokenA, address tokenB) external view returns (address) {
        (address token0, address token1) = tokenA < tokenB
            ? (tokenA, tokenB)
            : (tokenB, tokenA);
        bytes memory bytecode = abi.encodePacked(
            type(ZbxAMM).creationCode,
            abi.encode(token0, token1)
        );
        bytes32 initHash = keccak256(bytecode);
        bytes32 salt     = keccak256(abi.encodePacked(token0, token1));
        bytes32 h        = keccak256(abi.encodePacked(bytes1(0xff), address(this), salt, initHash));
        return address(uint160(uint256(h)));
    }
}
