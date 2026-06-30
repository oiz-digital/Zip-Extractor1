// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxVerifier — Interface for ZbxVerifier on-chain ZK proof verifier.
interface IZbxVerifier {
    event ProofVerified(bytes32 indexed proofId, bool valid);

    function verify(bytes calldata proof, bytes32[] calldata publicInputs) external view returns (bool);
    function verifyAndStore(bytes calldata proof, bytes32[] calldata publicInputs) external returns (bytes32 proofId);
    function isVerified(bytes32 proofId) external view returns (bool);
}
