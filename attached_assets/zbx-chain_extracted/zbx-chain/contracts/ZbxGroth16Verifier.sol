// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { Ownable2Step } from "./Ownable2Step.sol";

/// @title  ZbxGroth16Verifier — On-chain Groth16 proof verifier (BN254).
/// @author Zebvix Labs
/// @notice Verifies Groth16 zk-SNARK proofs over the BN254 curve using the
///         EVM precompiles defined by EIP-196 (ecAdd / ecMul) and EIP-197
///         (ecPairing). Layout follows the canonical Groth16 verifying-key
///         shape produced by snarkjs / circom / arkworks tooling.
///
/// @dev    Pairing equation checked (Groth16, see Groth 2016, §3.2):
///
///             e(A, B) · e(αG1, βG2)⁻¹ · e(L, γG2)⁻¹ · e(C, δG2)⁻¹ == 1
///
///         where L = IC[0] + Σᵢ pubInputs[i] · IC[i+1] (MSM in G1).
///
///         The precompile at 0x08 returns 1 when the product of pairings
///         equals one in the target group; we negate `A` so the equation
///         becomes a single multi-pairing call.
///
///         Gas (rough): ~250k–300k per verify with 1–3 public inputs;
///         dominated by the four-pairing precompile call (~181k) plus an
///         MSM that scales linearly with the number of public inputs.
///
/// @custom:zbx-chain  Chain ID 8989 (mainnet) / 8990 (testnet+devnet)
/// @custom:s26        Added in S26 hardening — companion to zbx-zk crate
contract ZbxGroth16Verifier is Ownable2Step {
    // ─── Curve constants (BN254) ────────────────────────────────────────────

    /// @dev Field modulus of BN254's base field Fq.
    uint256 internal constant Q =
        21888242871839275222246405745257275088696311157297823662689037894645226208583;

    /// @dev Order of BN254's scalar field Fr (== order of G1 / G2).
    uint256 internal constant R =
        21888242871839275222246405745257275088548364400416034343698204186575808495617;

    // ─── Verifying-key types ────────────────────────────────────────────────

    struct G1Point { uint256 X; uint256 Y; }

    /// @dev G2 points encode (Fq2 = a + bu) as (X[0]=b, X[1]=a) per EIP-197.
    struct G2Point { uint256[2] X; uint256[2] Y; }

    struct VerifyingKey {
        G1Point   alpha1;
        G2Point   beta2;
        G2Point   gamma2;
        G2Point   delta2;
        G1Point[] IC;          // IC.length == numPublicInputs + 1
    }

    /// @notice Active verifying key. Owner-settable so multiple circuits or
    ///         post-trusted-setup re-deployments are supported without a
    ///         contract migration.
    VerifyingKey internal _vk;

    /// @notice Whether the VK has been initialised. `verifyProof` reverts
    ///         until this is true to avoid the all-zero "everything passes"
    ///         class of bug seen in many naïve verifier deployments.
    bool public vkInitialised;

    // ─── Events ─────────────────────────────────────────────────────────────

    event VerifyingKeyUpdated(uint256 numPublicInputs);
    event ProofVerified(address indexed by, bytes32 indexed publicInputsHash, bool ok);

    // ─── Errors ─────────────────────────────────────────────────────────────

    error VKNotInitialised();
    error VKLengthMismatch(uint256 expectedICLength, uint256 publicInputsLen);
    error InputOutOfRange(uint256 index, uint256 value);
    error PairingPrecompileFailed();
    error ScalarMulFailed();
    error PointAddFailed();

    // ─── Construction ───────────────────────────────────────────────────────

    constructor(address initialOwner) Ownable2Step(initialOwner) {}

    // ─── VK admin ───────────────────────────────────────────────────────────

    /// @notice Replace the active verifying key. Caller must be owner.
    /// @param  alpha1 Groth16 αG1 from the trusted-setup output.
    /// @param  beta2  Groth16 βG2 from the trusted-setup output.
    /// @param  gamma2 Groth16 γG2 from the trusted-setup output.
    /// @param  delta2 Groth16 δG2 from the trusted-setup output.
    /// @param  ic     Linear-combination commitments. Length MUST be
    ///                `numPublicInputs + 1`; `ic[0]` is the constant term.
    function setVerifyingKey(
        G1Point   calldata alpha1,
        G2Point   calldata beta2,
        G2Point   calldata gamma2,
        G2Point   calldata delta2,
        G1Point[] calldata ic
    ) external onlyOwner {
        require(ic.length >= 1, "ZbxGroth16: IC empty");
        // Wipe the previous IC array before assigning the new one.
        delete _vk.IC;
        _vk.alpha1 = alpha1;
        _vk.beta2  = beta2;
        _vk.gamma2 = gamma2;
        _vk.delta2 = delta2;
        for (uint256 i = 0; i < ic.length; ++i) {
            _vk.IC.push(ic[i]);
        }
        vkInitialised = true;
        emit VerifyingKeyUpdated(ic.length - 1);
    }

    /// @notice Number of public inputs the active VK expects.
    function numPublicInputs() external view returns (uint256) {
        if (!vkInitialised) revert VKNotInitialised();
        return _vk.IC.length - 1;
    }

    // ─── Public verify ──────────────────────────────────────────────────────

    /// @notice Verify a Groth16 proof against the active VK.
    /// @param  a            Proof element A in G1.
    /// @param  b            Proof element B in G2.
    /// @param  c            Proof element C in G1.
    /// @param  publicInputs Public-input scalars (mod R). MUST have length
    ///                      `vk.IC.length - 1`.
    /// @return ok           True iff the pairing equation holds.
    function verifyProof(
        G1Point   calldata a,
        G2Point   calldata b,
        G1Point   calldata c,
        uint256[] calldata publicInputs
    ) external returns (bool ok) {
        if (!vkInitialised) revert VKNotInitialised();
        if (publicInputs.length + 1 != _vk.IC.length) {
            revert VKLengthMismatch(_vk.IC.length, publicInputs.length);
        }

        // Range-check public inputs against the scalar field order.
        for (uint256 i = 0; i < publicInputs.length; ++i) {
            if (publicInputs[i] >= R) revert InputOutOfRange(i, publicInputs[i]);
        }

        // Compute L = IC[0] + Σᵢ publicInputs[i] · IC[i+1] in G1.
        G1Point memory L = _vk.IC[0];
        for (uint256 i = 0; i < publicInputs.length; ++i) {
            G1Point memory term = _ecMul(_vk.IC[i + 1], publicInputs[i]);
            L = _ecAdd(L, term);
        }

        // Verify e(-A, B) · e(αG1, βG2) · e(L, γG2) · e(C, δG2) == 1.
        G1Point memory negA = G1Point({ X: a.X, Y: (a.Y == 0 ? 0 : Q - (a.Y % Q)) });
        ok = _pairing4(negA, b, _vk.alpha1, _vk.beta2, L, _vk.gamma2, c, _vk.delta2);

        emit ProofVerified(msg.sender, keccak256(abi.encode(publicInputs)), ok);
    }

    // ─── BN254 helpers (precompile wrappers) ────────────────────────────────

    /// @dev EIP-196 ecAdd at 0x06.
    function _ecAdd(G1Point memory p, G1Point memory r) internal view returns (G1Point memory out) {
        uint256[4] memory input = [p.X, p.Y, r.X, r.Y];
        uint256[2] memory ret;
        bool success;
        assembly {
            success := staticcall(gas(), 0x06, input, 0x80, ret, 0x40)
        }
        if (!success) revert PointAddFailed();
        out = G1Point({ X: ret[0], Y: ret[1] });
    }

    /// @dev EIP-196 ecMul at 0x07.
    function _ecMul(G1Point memory p, uint256 s) internal view returns (G1Point memory out) {
        uint256[3] memory input = [p.X, p.Y, s];
        uint256[2] memory ret;
        bool success;
        assembly {
            success := staticcall(gas(), 0x07, input, 0x60, ret, 0x40)
        }
        if (!success) revert ScalarMulFailed();
        out = G1Point({ X: ret[0], Y: ret[1] });
    }

    /// @dev EIP-197 ecPairing at 0x08 with four pairs.
    function _pairing4(
        G1Point memory a1, G2Point memory a2,
        G1Point memory b1, G2Point memory b2,
        G1Point memory c1, G2Point memory c2,
        G1Point memory d1, G2Point memory d2
    ) internal view returns (bool ok) {
        uint256[24] memory input;
        // a
        input[0]  = a1.X; input[1]  = a1.Y;
        input[2]  = a2.X[0]; input[3] = a2.X[1];
        input[4]  = a2.Y[0]; input[5] = a2.Y[1];
        // b
        input[6]  = b1.X; input[7]  = b1.Y;
        input[8]  = b2.X[0]; input[9] = b2.X[1];
        input[10] = b2.Y[0]; input[11] = b2.Y[1];
        // c
        input[12] = c1.X; input[13] = c1.Y;
        input[14] = c2.X[0]; input[15] = c2.X[1];
        input[16] = c2.Y[0]; input[17] = c2.Y[1];
        // d
        input[18] = d1.X; input[19] = d1.Y;
        input[20] = d2.X[0]; input[21] = d2.X[1];
        input[22] = d2.Y[0]; input[23] = d2.Y[1];

        uint256[1] memory out;
        bool success;
        assembly {
            success := staticcall(gas(), 0x08, input, 0x300, out, 0x20)
        }
        if (!success) revert PairingPrecompileFailed();
        ok = out[0] == 1;
    }
}
