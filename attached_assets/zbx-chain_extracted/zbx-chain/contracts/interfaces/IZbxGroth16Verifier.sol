// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxGroth16Verifier — Interface for ZbxGroth16Verifier Groth16 ZK-SNARK verifier.
interface IZbxGroth16Verifier {
    function verifyProof(
        uint256[2]  calldata pA,
        uint256[2][2] calldata pB,
        uint256[2]  calldata pC,
        uint256[]   calldata pubSignals
    ) external view returns (bool);
}
