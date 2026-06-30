// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { StarkFri } from "../libraries/StarkFri.sol";

/// @title  IZbxStarkVerifier — strongly-typed verifier surface.
/// @notice Used by `ZbxVerifier` to delegate STARK verification through a
///         **typed** interface call (NOT a raw `address.call(bytes)`). The
///         typed call guarantees the called selector is `verifyProof` and
///         that the supplied calldata is shape-checked by the ABI decoder
///         before reaching the verifier — preventing a class of
///         verification-bypass attacks where an attacker would otherwise
///         be free to invoke any function on the verifier contract that
///         happens to return a truthy bool.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:s27        Added during S27 hardening (architect-flagged CRIT)
interface IZbxStarkVerifier {
    /// Mirrors `ZbxStarkVerifier.StarkProof` exactly. Kept in this
    /// interface so consumers depend on a small, stable surface rather
    /// than the full implementation contract.
    struct StarkProof {
        bytes32                  traceRoot;
        bytes32                  constraintRoot;
        uint256[]                publicInputs;
        uint256[]                oodTraceEvals;
        uint256                  oodConstraintEval;
        StarkFri.CommitPhase     friCommit;
        StarkFri.QueryProof[]    friQueries;
    }

    function verifyProof(StarkProof calldata proof) external returns (bool);
}
