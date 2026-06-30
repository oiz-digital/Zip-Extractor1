// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { GoldilocksField as F } from "./GoldilocksField.sol";
import { StarkMerkle }          from "./StarkMerkle.sol";
import { StarkTranscript }      from "./StarkTranscript.sol";

/// @title  StarkFri — FRI (Fast Reed-Solomon Interactive Oracle) verifier.
/// @notice FRI is the low-degree-test underpinning STARK soundness. Given a
///         Merkle commitment to a vector of evaluations of a function `f`
///         over a domain `D`, FRI proves that `f` is close (in Hamming
///         distance) to a polynomial of degree `< d`. The verifier need
///         only authenticate `O(λ · log d)` openings, regardless of `d`.
///
/// @dev    Protocol (folding factor 2, the standard / cheapest choice):
///
///         **Commit phase (off-chain, replayed via transcript here):**
///           1. f₀ := original committed evaluations on D (size N = blowup·d)
///           2. for i in 0..numFoldingSteps:
///              - α_i ← transcript.challenge()                 [random fold pt]
///              - f_{i+1}(y) := (f_i(x) + f_i(−x))/2
///                            + α_i · (f_i(x) − f_i(−x))/(2x)   where y = x²
///              - commit Merkle root of f_{i+1}'s evaluations
///           3. final layer: commit a single field element (constant poly)
///
///         **Query phase (verifier challenges + opens):**
///           For each of `numQueries` random indices q ∈ [0, N):
///             - prover supplies authentication paths for f_i(x) and f_i(−x)
///               at every layer i (folded index halves each step)
///             - verifier checks Merkle path against commit-phase root
///             - verifier checks the folding equation between consecutive
///               layers using the layer's α challenge
///             - final-layer value MUST equal the committed constant
///
///         Soundness ≈ 2^{−numQueries · log₂(blowupFactor)} for honest-prover
///         + small-list-decoding bound. With our defaults
///         `numQueries = 40`, `blowupFactor = 4` ⇒ ≈ 2⁻⁸⁰ soundness error.
///
/// @custom:s27  Added in S27 — STARK verifier framework
library StarkFri {
    using StarkTranscript for StarkTranscript.Transcript;

    // ─── Wire-format types ────────────────────────────────────────────────

    struct CommitPhase {
        /// Merkle roots of the folded layers (length = numFoldingSteps).
        bytes32[] layerRoots;
        /// Constant value of the final (degree-0) layer.
        uint256   finalValue;
    }

    /// One opening per query, per layer. `valueX` is f_i(x), `valueNegX`
    /// is f_i(−x); paths authenticate them against `layerRoots[i]`.
    struct LayerOpening {
        uint256     valueX;
        uint256     valueNegX;
        bytes32[]   pathX;
        bytes32[]   pathNegX;
    }

    struct QueryProof {
        /// Per-layer openings. Length = numFoldingSteps.
        LayerOpening[] layers;
    }

    // ─── Errors ───────────────────────────────────────────────────────────

    error LayerCountMismatch(uint256 expected, uint256 got);
    error MerklePathInvalid(uint256 query, uint256 layer);
    error FoldingMismatch(uint256 query, uint256 layer);
    error FinalLayerMismatch(uint256 query, uint256 expected, uint256 got);
    error DomainShrinkBug();

    // ─── Verify ───────────────────────────────────────────────────────────

    struct VerifyParams {
        uint256 initialDomainSize;  // N (power of 2)
        uint256 numFoldingSteps;    // log₂(N / finalDomainSize)
        uint256 numQueries;         // soundness parameter
        uint256 domainGenerator;    // primitive N-th root of unity in F
        uint256 domainOffset;       // coset offset (1 if no shift)
    }

    /// @notice Run the full FRI verification pipeline.
    /// @param  params   Domain + soundness parameters (per-circuit).
    /// @param  commit   Layer roots + final constant (committed off-chain).
    /// @param  queries  One QueryProof per query index (length = numQueries).
    /// @param  transcript Mutable Fiat-Shamir transcript already seeded with
    ///                    the trace + constraint roots; on return its
    ///                    counter has consumed all FRI challenges.
    /// @return ok       True iff all checks pass.
    function verify(
        VerifyParams memory params,
        CommitPhase  memory commit,
        QueryProof[] calldata queries,
        StarkTranscript.Transcript memory transcript
    ) internal pure returns (bool ok) {
        if (commit.layerRoots.length != params.numFoldingSteps) {
            revert LayerCountMismatch(params.numFoldingSteps, commit.layerRoots.length);
        }
        if (queries.length != params.numQueries) {
            revert LayerCountMismatch(params.numQueries, queries.length);
        }

        // ── Replay commit-phase transcript to derive the folding challenges.
        uint256[] memory alphas = new uint256[](params.numFoldingSteps);
        for (uint256 i = 0; i < params.numFoldingSteps; ++i) {
            alphas[i] = transcript.challengeFelt();
            transcript.absorbBytes32(commit.layerRoots[i]);
        }
        transcript.absorbFelt(commit.finalValue);

        // ── Query phase.
        uint256 domain = params.initialDomainSize;
        for (uint256 q = 0; q < params.numQueries; ++q) {
            uint256 idx = transcript.challengeQueryIndex(domain);
            _verifyOneQuery(params, commit, queries[q], alphas, idx);
        }
        ok = true;
    }

    /// @dev Per-query verification. Walks each folding layer:
    ///        - authenticate f_i(x) and f_i(−x) against layerRoots[i]
    ///        - compute next-layer expected value via folding equation
    ///        - check it matches f_{i+1}(idx>>1) supplied in next layer
    ///        - at the last layer, check folded value == finalValue
    function _verifyOneQuery(
        VerifyParams memory params,
        CommitPhase  memory commit,
        QueryProof   calldata qp,
        uint256[]    memory alphas,
        uint256      idx
    ) private pure {
        if (qp.layers.length != params.numFoldingSteps) {
            revert LayerCountMismatch(params.numFoldingSteps, qp.layers.length);
        }

        uint256 domain = params.initialDomainSize;
        uint256 g      = params.domainGenerator;
        uint256 offset = params.domainOffset;

        // x_at_idx = offset · g^idx — the actual evaluation point.
        // We track g^idx incrementally to avoid a fresh `pow` per query.
        uint256 xLayer = F.mul(offset, F.pow(g, idx));

        for (uint256 i = 0; i < params.numFoldingSteps; ++i) {
            StarkFri.LayerOpening calldata op = qp.layers[i];

            uint256 layerSize = domain;
            uint256 halfSize  = layerSize >> 1;

            // Sibling index of x in current layer is idx ⊕ halfSize (the
            // two cosets of size 2 in a power-of-two domain are {x, −x},
            // and −x corresponds to flipping the high bit of the index).
            uint256 siblingIdx = idx ^ halfSize;

            // Authenticate both openings against this layer's commitment.
            bytes32 lh1 = StarkMerkle.leafHashSingle(op.valueX);
            bytes32 lh2 = StarkMerkle.leafHashSingle(op.valueNegX);

            if (!StarkMerkle.verify(commit.layerRoots[i], idx,        lh1, op.pathX)) {
                revert MerklePathInvalid(idx, i);
            }
            if (!StarkMerkle.verify(commit.layerRoots[i], siblingIdx, lh2, op.pathNegX)) {
                revert MerklePathInvalid(siblingIdx, i);
            }

            // Folding equation:
            //   f_{i+1}(y) = (f(x) + f(−x))/2 + α · (f(x) − f(−x))/(2x)
            // where y = x². We compute the RHS and use it as the expected
            // f_{i+1}(idx>>1) value for the next layer.
            uint256 sumHalf  = F.div(F.add(op.valueX, op.valueNegX), 2);
            uint256 diffOver = F.div(F.sub(op.valueX, op.valueNegX),
                                      F.mul(2, xLayer));
            uint256 folded   = F.add(sumHalf, F.mul(alphas[i], diffOver));

            // Advance index, point, domain to the next layer.
            idx     = idx & (halfSize - 1);   // halve the index
            xLayer  = F.mul(xLayer, xLayer);  // x ← x²
            domain  = halfSize;
            if (domain == 0) revert DomainShrinkBug();

            // If we're at the LAST folding step, the folded value must
            // equal the committed constant final value.
            if (i + 1 == params.numFoldingSteps) {
                if (folded != commit.finalValue) {
                    revert FinalLayerMismatch(idx, commit.finalValue, folded);
                }
            } else {
                // Otherwise, the next-layer opening MUST match the folded
                // value. The next-layer prover commits f_{i+1} on a domain
                // of size halfSize and authenticates the opening at the
                // (already-halved) index `idx` as `next.valueX`. So the
                // parent→child binding is unambiguous: folded == next.valueX.
                // (Architect-flagged S27 CRIT: the parity-branch the prior
                // version used had no cryptographic basis and could have
                // accepted malformed proofs at odd indices.)
                StarkFri.LayerOpening calldata next = qp.layers[i + 1];
                if (next.valueX != folded) {
                    revert FoldingMismatch(idx, i);
                }
            }
        }
    }
}
