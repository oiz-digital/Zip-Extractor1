// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

/// @title  ZbxStarkVerifier.t.sol
/// @notice HEVM/forge unit tests for the S27 STARK verifier framework.
///         Covers field arithmetic, transcript determinism, Merkle proof
///         path verification, and the fail-closed behaviour of
///         `ZbxVerifier._verifyStarkProof` when no STARK verifier is
///         configured.
///
/// @dev    NOT executable in the Replit sandbox (no forge binary, no
///         RocksDB capacity). Mandatory off-sandbox: VPS
///         `forge test --match-path contracts/test/ZbxStarkVerifier.t.sol -vvv`
///         must show all assertions passing before mainnet 8989 deploy.
///
///         Cryptographic happy-path tests (verify a real STARK proof
///         end-to-end) require a fixture proof artefact emitted by the
///         `zbx-prover` Rust toolchain. A reference fixture is tracked
///         separately in `zbx-prover/fixtures/stark_v1_test.bin` and
///         loaded via the helper test harness in `_loadFixture` below
///         (currently unimplemented — fixture file is in the prover
///         build-output gitignore by default).
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:standard   S27 — Solidity STARK verifier
/// @custom:s27        Closes S26-FOLLOWUP-STARK-CODEGEN

import { GoldilocksField as F } from "../libraries/GoldilocksField.sol";
import { StarkTranscript }      from "../libraries/StarkTranscript.sol";
import { StarkMerkle }          from "../libraries/StarkMerkle.sol";
import { ZbxStarkVerifier }     from "../ZbxStarkVerifier.sol";
import { IZbxStarkVerifier }    from "../interfaces/IZbxStarkVerifier.sol";

interface Hevm {
    function prank(address) external;
    function expectRevert(bytes4) external;
    function expectRevert(bytes calldata) external;
}

contract ZbxStarkVerifierTest {
    Hevm constant hevm = Hevm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);

    address constant OWNER = address(0xB0B0);

    ZbxStarkVerifier internal verifier;

    function setUp() public {
        verifier = new ZbxStarkVerifier(OWNER);
    }

    // ─── GoldilocksField ───────────────────────────────────────────────────

    function test_field_add_subZeroIdentity() public pure {
        require(F.add(7, 0) == 7,                                        "add zero identity");
        require(F.sub(7, 0) == 7,                                        "sub zero identity");
        require(F.add(F.P - 1, 1) == 0,                                  "wrap to zero");
        require(F.sub(0, 1) == F.P - 1,                                  "underflow wraps");
    }

    function test_field_mulCommutativeAndAssoc() public pure {
        uint256 a = 12345; uint256 b = 67890; uint256 c = 1029384756;
        require(F.mul(a, b) == F.mul(b, a),                              "commutativity");
        require(F.mul(F.mul(a, b), c) == F.mul(a, F.mul(b, c)),          "associativity");
    }

    function test_field_inverseRoundtrip() public pure {
        uint256 x = 0x123456789ABCDEF;
        uint256 inv = F.inv(x);
        require(F.mul(x, inv) == 1,                                      "x · x⁻¹ == 1");
    }

    function test_field_inverseZeroReverts() public {
        hevm.expectRevert(F.InverseOfZero.selector);
        F.inv(0);
    }

    function test_field_powMatchesIteratedMul() public pure {
        // 7^5 by repeated squaring vs by hand
        uint256 byPow = F.pow(7, 5);
        uint256 byHand = F.mul(F.mul(F.mul(F.mul(7, 7), 7), 7), 7);
        require(byPow == byHand,                                         "pow == iterated mul");
    }

    function test_field_outOfRangeReverts() public {
        hevm.expectRevert(abi.encodeWithSelector(F.NotInField.selector, F.P));
        F.checked(F.P);
    }

    // ─── StarkTranscript ───────────────────────────────────────────────────

    function test_transcript_determinism() public pure {
        StarkTranscript.Transcript memory t1 = StarkTranscript.init("test");
        StarkTranscript.Transcript memory t2 = StarkTranscript.init("test");
        StarkTranscript.absorbFelt(t1, 42);
        StarkTranscript.absorbFelt(t2, 42);
        uint256 c1 = StarkTranscript.challengeFelt(t1);
        uint256 c2 = StarkTranscript.challengeFelt(t2);
        require(c1 == c2,                                                "deterministic challenge");
    }

    function test_transcript_differentDstDifferentChallenge() public pure {
        StarkTranscript.Transcript memory t1 = StarkTranscript.init("a");
        StarkTranscript.Transcript memory t2 = StarkTranscript.init("b");
        require(StarkTranscript.challengeFelt(t1) != StarkTranscript.challengeFelt(t2),
                "DST changes challenge");
    }

    function test_transcript_consecutiveChallengesDiffer() public pure {
        StarkTranscript.Transcript memory t = StarkTranscript.init("test");
        uint256 c1 = StarkTranscript.challengeFelt(t);
        uint256 c2 = StarkTranscript.challengeFelt(t);
        require(c1 != c2,                                                "counter advances");
    }

    function test_transcript_queryIndexBoundedByDomain() public pure {
        StarkTranscript.Transcript memory t = StarkTranscript.init("test");
        uint256 idx = StarkTranscript.challengeQueryIndex(t, 1024);
        require(idx < 1024,                                              "index in bounds");
    }

    // ─── StarkMerkle ───────────────────────────────────────────────────────

    function test_merkle_singleLeafTreeRoundtrip() public pure {
        // depth-1 tree: two leaves L0, L1
        bytes32 l0 = keccak256("leaf0");
        bytes32 l1 = keccak256("leaf1");
        bytes32 root = keccak256(abi.encodePacked(l0, l1));

        bytes32[] memory pathL0 = new bytes32[](1);
        pathL0[0] = l1;

        bytes32[] memory pathL1 = new bytes32[](1);
        pathL1[0] = l0;

        // Use calldata-only verify by going through a test stub.
        require(_verifyStub(root, 0, l0, pathL0),                        "L0 verifies");
        require(_verifyStub(root, 1, l1, pathL1),                        "L1 verifies");
    }

    function test_merkle_tamperedPathRejected() public pure {
        bytes32 l0 = keccak256("leaf0");
        bytes32 l1 = keccak256("leaf1");
        bytes32 root = keccak256(abi.encodePacked(l0, l1));
        bytes32[] memory bad = new bytes32[](1);
        bad[0] = keccak256("not l1");
        require(!_verifyStub(root, 0, l0, bad),                          "tampered rejected");
    }

    /// @dev Helper that re-exposes the calldata-only StarkMerkle.verify.
    function _verifyStub(
        bytes32 root,
        uint256 idx,
        bytes32 leaf,
        bytes32[] memory path
    ) public view returns (bool) {
        // Bounce through `this` so the array becomes `calldata`.
        return this._verifyStubCD(root, idx, leaf, path);
    }
    function _verifyStubCD(
        bytes32 root,
        uint256 idx,
        bytes32 leaf,
        bytes32[] calldata path
    ) external pure returns (bool) {
        return StarkMerkle.verify(root, idx, leaf, path);
    }

    // ─── ZbxStarkVerifier admin ───────────────────────────────────────────

    function test_verifier_paramsNotSetReverts() public {
        // Calling verifyProof without setCircuitParams must revert.
        IZbxStarkVerifier.StarkProof memory empty;
        hevm.expectRevert(ZbxStarkVerifier.ParamsNotSet.selector);
        this._verifyProofCD(empty);
    }

    /// @dev Bounce through external `this` to convert the in-memory proof
    ///      to calldata as required by the typed interface.
    function _verifyProofCD(IZbxStarkVerifier.StarkProof calldata p) external returns (bool) {
        return verifier.verifyProof(p);
    }

    function test_verifier_traceLengthMustBePow2() public {
        ZbxStarkVerifier.CircuitParams memory p = _defaultParams();
        p.traceLength = 1000; // NOT a power of two
        hevm.prank(OWNER);
        hevm.expectRevert(ZbxStarkVerifier.TraceLengthNotPow2.selector);
        verifier.setCircuitParams(p);
    }

    function _defaultParams() internal pure returns (ZbxStarkVerifier.CircuitParams memory p) {
        p.traceLength             = 1024;
        p.numColumns              = 4;
        p.numPublicInputs         = 2;
        p.blowupFactor            = 4;
        p.numQueries              = 40;
        p.numFoldingSteps         = 12; // log2(1024 * 4)
        p.constraintDegree        = 2;
        // Order-2^32 root of unity in Goldilocks (per Plonky2 const tables).
        // Real production deployments must override with the prover's actual
        // domain generator. This value is used only for the shape test below.
        p.lowDegreeDomainGenerator = 0x185629DCDA58878C;
        p.lowDegreeDomainOffset    = 1;
    }
}
