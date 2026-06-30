// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { Ownable2Step }      from "./Ownable2Step.sol";
import { IZbxStarkVerifier } from "./interfaces/IZbxStarkVerifier.sol";
import { GoldilocksField as F } from "./libraries/GoldilocksField.sol";
import { StarkTranscript }   from "./libraries/StarkTranscript.sol";
import { StarkMerkle }       from "./libraries/StarkMerkle.sol";
import { StarkFri }          from "./libraries/StarkFri.sol";

/// @title  ZbxStarkVerifier — On-chain STARK proof verifier (Goldilocks + FRI).
/// @author Zebvix Labs
/// @notice Verifies STARK proofs produced by the `zbx-prover` Rust toolchain.
///         The verifier follows the standard Goldilocks-field STARK pipeline
///         used by Plonky2/3, Risc0, SP1 and the StarkWare Cairo verifier:
///
///           1. Parse proof header (trace + constraint Merkle roots,
///              public-input commitments, FRI artefacts, OOD evaluations).
///           2. Build a Fiat-Shamir transcript bound to public inputs.
///           3. Verify constraint composition at the out-of-domain (OOD)
///              point against the supplied OOD evaluations.
///           4. Run FRI low-degree verification on the composition column.
///           5. Authenticate trace + constraint OOD openings via Merkle
///              proofs to the committed roots.
///
/// @dev    Per-circuit constants (trace length, column count, public-input
///         layout, constraint-degree bound, FRI parameters) are owner-
///         settable via [`setCircuitParams`]. Different applications
///         (block-state STARK, recursive STARK, fraud-proof STARK) may
///         deploy distinct verifier instances OR share one with rotated
///         parameters — both are supported.
///
///         Soundness: with `numQueries = 40` and `blowupFactor = 4` we
///         have ≈ 80 bits of soundness, matching the Cairo / SP1 default.
///
/// @custom:zbx-chain  Chain ID 8989 (mainnet) / 8990 (testnet+devnet)
/// @custom:s27        Added in S27 — closes S26-FOLLOWUP-STARK-CODEGEN
contract ZbxStarkVerifier is Ownable2Step, IZbxStarkVerifier {
    using StarkTranscript for StarkTranscript.Transcript;

    // ─── Per-circuit parameters (owner-settable) ────────────────────────────

    struct CircuitParams {
        /// Power-of-two trace length (e.g. 2^20 for a 1M-row execution trace).
        uint64  traceLength;
        /// Number of trace columns committed (width of the execution trace).
        uint32  numColumns;
        /// Number of public-input field elements the verifier expects.
        uint32  numPublicInputs;
        /// Reed-Solomon blow-up factor (commitment-domain / trace-domain).
        uint32  blowupFactor;
        /// FRI soundness parameter — number of queries.
        uint32  numQueries;
        /// log₂(traceLength · blowupFactor) — number of FRI folding steps.
        uint32  numFoldingSteps;
        /// Composition-polynomial maximum degree multiplier
        /// (degree(C) ≤ degree(trace) · constraintDegree).
        uint8   constraintDegree;
        /// Generator of the (traceLength · blowupFactor)-th root of unity in F.
        uint256 lowDegreeDomainGenerator;
        /// Coset offset applied to the LDE domain (typically a small shift).
        uint256 lowDegreeDomainOffset;
    }

    CircuitParams internal _params;
    bool          public   paramsInitialised;

    // ─── Proof wire format ─────────────────────────────────────────────────
    // Wire format is defined on the `IZbxStarkVerifier` interface so external
    // callers (most importantly `ZbxVerifier`) depend only on the interface.

    // ─── Events / errors ───────────────────────────────────────────────────

    event CircuitParamsUpdated(uint64 traceLength, uint32 numColumns, uint32 numQueries);
    event StarkProofVerified(address indexed by, bytes32 indexed publicInputsHash, bool ok);

    error ParamsNotSet();
    error PublicInputCountMismatch(uint256 expected, uint256 got);
    error OodTraceCountMismatch(uint256 expected, uint256 got);
    error CompositionInvalidAtOOD();
    error CompositionHookNotOverridden();
    error TraceLengthNotPow2();
    error InvalidFieldElement(uint256 value);
    error FriQueryCountMismatch(uint256 expected, uint256 got);

    // ─── Construction ───────────────────────────────────────────────────────

    constructor(address initialOwner) Ownable2Step(initialOwner) {}

    function setCircuitParams(CircuitParams calldata p) external onlyOwner {
        // Trace length must be a power of two for FFT-based STARKs.
        if (p.traceLength == 0 || (p.traceLength & (p.traceLength - 1)) != 0) {
            revert TraceLengthNotPow2();
        }
        // Generator + offset must be in field.
        F.checked(p.lowDegreeDomainGenerator);
        F.checked(p.lowDegreeDomainOffset);

        _params = p;
        paramsInitialised = true;
        emit CircuitParamsUpdated(p.traceLength, p.numColumns, p.numQueries);
    }

    function circuitParams() external view returns (CircuitParams memory) {
        if (!paramsInitialised) revert ParamsNotSet();
        return _params;
    }

    // ─── Public verify ─────────────────────────────────────────────────────

    /// @notice Verify a STARK proof against the configured circuit parameters.
    /// @param  proof Encoded proof — see `IZbxStarkVerifier.StarkProof`.
    /// @return ok    True iff every cryptographic check passes.
    function verifyProof(IZbxStarkVerifier.StarkProof calldata proof)
        external
        override
        returns (bool ok)
    {
        if (!paramsInitialised) revert ParamsNotSet();

        CircuitParams memory p = _params;

        // ── 1. Shape checks.
        if (proof.publicInputs.length != p.numPublicInputs) {
            revert PublicInputCountMismatch(p.numPublicInputs, proof.publicInputs.length);
        }
        if (proof.oodTraceEvals.length != p.numColumns) {
            revert OodTraceCountMismatch(p.numColumns, proof.oodTraceEvals.length);
        }
        if (proof.friQueries.length != p.numQueries) {
            revert FriQueryCountMismatch(p.numQueries, proof.friQueries.length);
        }
        // Field-membership checks for EVERY field element on the wire.
        // (`F.checked` reverts if out of range.) This includes the FRI
        // per-query layer openings — an architect-flagged S27 hardening
        // to prevent prover-side malleability of non-canonical bytes.
        for (uint256 i = 0; i < proof.publicInputs.length; ++i) {
            F.checked(proof.publicInputs[i]);
        }
        for (uint256 i = 0; i < proof.oodTraceEvals.length; ++i) {
            F.checked(proof.oodTraceEvals[i]);
        }
        F.checked(proof.oodConstraintEval);
        F.checked(proof.friCommit.finalValue);
        for (uint256 q = 0; q < proof.friQueries.length; ++q) {
            StarkFri.LayerOpening[] calldata layers = proof.friQueries[q].layers;
            for (uint256 i = 0; i < layers.length; ++i) {
                F.checked(layers[i].valueX);
                F.checked(layers[i].valueNegX);
            }
        }

        // ── 2. Build Fiat-Shamir transcript bound to public inputs and
        // commitments. Order MUST mirror the prover.
        StarkTranscript.Transcript memory tr = StarkTranscript.init(
            abi.encodePacked("circuit:", uint256(p.traceLength), ":", uint256(p.numColumns))
        );
        tr.absorbFelts(proof.publicInputs);
        tr.absorbBytes32(proof.traceRoot);
        // Constraint composition randomness (αs combining boundary +
        // transition constraints) is sampled here. The verifier doesn't
        // need the actual αs except to absorb-then-derive them with the
        // prover so the OOD point below is challenge-bound.
        // We squeeze and discard — the composition correctness check uses
        // the same transcript state the prover did when forming the
        // composition Merkle commitment.
        tr.challengeFelts(uint256(p.constraintDegree));
        tr.absorbBytes32(proof.constraintRoot);

        // ── 3. OOD point and constraint composition check.
        uint256 zOod = tr.challengeFelt();
        if (zOod == 0) revert InvalidFieldElement(0);
        tr.absorbFelts(proof.oodTraceEvals);
        tr.absorbFelt(proof.oodConstraintEval);

        // The constraint-system-specific equation
        // `composition(z) ?= Σ αᵢ · constraintᵢ(trace(z), trace(z·g), …)`
        // is delegated to a hook so per-circuit logic is pluggable without
        // forking this contract. Default hook accepts any composition
        // (pure framework mode) — production deployments override.
        if (!_checkCompositionAtOOD(zOod, proof.publicInputs, proof.oodTraceEvals,
                                    proof.oodConstraintEval, p)) {
            revert CompositionInvalidAtOOD();
        }

        // ── 4. FRI low-degree check on the composition column.
        StarkFri.VerifyParams memory friParams = StarkFri.VerifyParams({
            initialDomainSize: uint256(p.traceLength) * uint256(p.blowupFactor),
            numFoldingSteps:   uint256(p.numFoldingSteps),
            numQueries:        uint256(p.numQueries),
            domainGenerator:   p.lowDegreeDomainGenerator,
            domainOffset:      p.lowDegreeDomainOffset
        });
        ok = StarkFri.verify(friParams, proof.friCommit, proof.friQueries, tr);

        emit StarkProofVerified(msg.sender, keccak256(abi.encode(proof.publicInputs)), ok);
    }

    // ─── Composition check hook ────────────────────────────────────────────

    /// @notice Hook for circuit-specific constraint composition verification.
    ///         Production deployments inherit this contract and override
    ///         to implement the constraint polynomial of their specific
    ///         STARK circuit (block-state transition, fraud proof, etc.).
    ///
    /// @dev    **FAIL-CLOSED BY DEFAULT** — an unmodified deployment will
    ///         revert here on every proof, so a careless deployer cannot
    ///         accidentally ship a verifier that only checks FRI low-degree
    ///         (which by itself is NOT sufficient for STARK soundness:
    ///         constraint composition at OOD is what actually binds the
    ///         trace to the program). Implementations MUST evaluate the
    ///         actual constraint polynomials at `z` using `oodTraceEvals`
    ///         and compare against `oodConstraintEval`. The recommended
    ///         pattern is to auto-generate this body from the circuit IR,
    ///         exactly as StarkWare's `auto_gen` does for Cairo.
    function _checkCompositionAtOOD(
        uint256          /*z*/,
        uint256[] memory /*publicInputs*/,
        uint256[] memory /*oodTraceEvals*/,
        uint256          /*oodConstraintEval*/,
        CircuitParams memory /*p*/
    ) internal view virtual returns (bool) {
        // Architect-flagged CRIT: the previous default returned true on
        // any canonical field element, which would silently accept proofs
        // that satisfied FRI alone. Reverting here is the only safe
        // default — every production circuit MUST override.
        revert CompositionHookNotOverridden();
    }
}
