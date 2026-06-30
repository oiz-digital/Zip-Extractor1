// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title  ZbxRandomBeacon — On-chain randomness via VRF precompile 0x0E
/// @author Zebvix Technologies Pvt Ltd
///
/// @notice Sample integration of the RFC 9381 ECVRF-EDWARDS25519-SHA512-ELL2
///         verifier exposed at precompile address `0x000...0E`. Operators
///         publish a (alpha, pi) pair signed off-chain by a known Ed25519
///         pubkey; the contract verifies the proof on-chain and stores the
///         resulting 64-byte beta as the canonical randomness for `epoch`.
///
///         Input  to precompile: pubkey:32 ‖ alpha:N ‖ pi:80
///         Output:               64-byte beta on valid; 32-byte zero on invalid.
///
///         Calling contracts MUST treat a 32-byte return as INVALID and
///         either revert or fall back to a safe default — the precompile
///         deliberately does NOT revert on bad proofs (matches ECRECOVER).
///
/// @dev    Status: the on-chain verifier is currently fail-closed (every
///         proof returns 32 zero bytes), so `commit()` will revert with
///         `VrfInvalidProof` until the verifier body lands. The wire
///         layout, gas cost, and output convention are stable across
///         that upgrade — no contract change required.
///
/// @custom:zbx-chain  Chain ID 8989 / 8990
contract ZbxRandomBeacon {
    /// VRF precompile address (RFC 9381 ECVRF-EDWARDS25519-SHA512-ELL2).
    address public constant VRF_PRECOMPILE = address(0x0E);

    /// Authorised Ed25519 pubkey of the off-chain VRF operator. Set once
    /// at construction; rotate via a separate governance contract if needed.
    bytes32 public immutable operatorPubkey;

    /// epoch => 64-byte beta (random output). Empty bytes means unset.
    mapping(uint256 => bytes) public randomnessOf;

    /// epoch => block number where it was committed (replay guard).
    mapping(uint256 => uint256) public committedAt;

    event RandomnessCommitted(
        uint256 indexed epoch,
        bytes32 alphaHash,
        bytes32 betaHash
    );

    error VrfInvalidProof();
    error EpochAlreadyCommitted(uint256 epoch);
    error EmptyAlpha();

    constructor(bytes32 _operatorPubkey) {
        operatorPubkey = _operatorPubkey;
    }

    /// @notice Commit a VRF beacon for `epoch`. Reverts on duplicate commit
    ///         or invalid proof. The 64-byte beta is stored verbatim and
    ///         can be reduced (e.g. `uint256(keccak256(beta))`) by callers.
    /// @param epoch  Monotonically-increasing beacon index (caller-defined).
    /// @param alpha  Variable-length VRF input. MUST be non-empty and
    ///               domain-separated by the caller (e.g. abi-encoded
    ///               (chainid, epoch, ...)).
    /// @param pi     80-byte VRF proof (Γ:32 ‖ c:16 ‖ s:32).
    function commit(uint256 epoch, bytes calldata alpha, bytes calldata pi) external {
        if (committedAt[epoch] != 0) revert EpochAlreadyCommitted(epoch);
        if (alpha.length == 0) revert EmptyAlpha();
        require(pi.length == 80, "ZBX-VRF: pi must be 80 bytes");

        // Build the precompile input: pubkey:32 ‖ alpha:N ‖ pi:80.
        bytes memory input = bytes.concat(
            abi.encodePacked(operatorPubkey),
            alpha,
            pi
        );

        (bool ok, bytes memory ret) = VRF_PRECOMPILE.staticcall(input);
        require(ok, "ZBX-VRF: precompile call failed");

        // Fail-soft: invalid proof returns 32 zero bytes.
        if (ret.length != 64) revert VrfInvalidProof();

        randomnessOf[epoch] = ret;
        committedAt[epoch] = block.number;

        emit RandomnessCommitted(epoch, keccak256(alpha), keccak256(ret));
    }

    /// @notice Helper: reduce stored beta to a single uint256 for typical
    ///         "pick a number" use cases. Reverts if epoch is uncommitted.
    function randomUint(uint256 epoch) external view returns (uint256) {
        bytes memory beta = randomnessOf[epoch];
        require(beta.length == 64, "ZBX-VRF: epoch not committed");
        return uint256(keccak256(beta));
    }
}
