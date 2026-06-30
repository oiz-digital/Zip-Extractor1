// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

/// @title  GovernorVotesIntegration.t.sol — S22a HEVM tests
/// @notice Asserts ZbxGovernor's `_getVotes` / `_totalSupplyAt` now delegate to
///         ZBXGov real checkpoints (no longer stub-zero), and the off-by-one
///         snapshot fix (`startBlock - 1`) prevents the "first Active block
///         vote reverts" trap.
///
/// @dev    NOT executable in the Replit sandbox. Mandatory off-sandbox:
///         `forge test --match-path contracts/test/GovernorVotesIntegration.t.sol -vvv`
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:since      S22a

import { ZbxGovernor } from "../ZbxGovernor.sol";
import { ZBXGov }      from "../tokens/ZBXGov.sol";

/// @dev Local cheatcode interface — this codebase's existing test files
///      declare only the subset they need. S22a needs `roll` and `warp`
///      to simulate the propose/vote flow across multiple blocks.
interface Hevm {
    function roll(uint256)                external;
    function warp(uint256)                external;
    function prank(address)               external;
    function expectRevert(bytes calldata) external;
    function expectRevert(bytes4)         external;
}

/// @dev Empty sentinel — ZbxGovernor's constructor only requires non-zero
///      addresses for `token` and `timelock`. The Governor never calls
///      back into the timelock from the test paths exercised here.
contract MockTimelock {}

contract GovernorVotesIntegrationTest {
    Hevm constant hevm = Hevm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);

    ZBXGov       public gov;
    ZbxGovernor  public governor;
    MockTimelock public timelock;

    /// @dev Test contract IS the staking contract (only address that can
    ///      mint/burn ZBXGov). This is the canonical pattern used by
    ///      ZbxTvlOracle.t.sol and other in-repo HEVM tests.
    constructor() {
        gov      = new ZBXGov(address(this));   // staking == this
        timelock = new MockTimelock();
        governor = new ZbxGovernor(address(gov), address(timelock));
    }

    // ────────────────────────────────────────────────────────────────────────
    //  ZBXGov.totalSupplyAt — direct unit tests
    // ────────────────────────────────────────────────────────────────────────

    /// 1. totalSupplyAt(blockNumber) MUST revert when blockNumber == current.
    function test_TotalSupplyAt_RevertsAtCurrentBlock() public {
        hevm.expectRevert(bytes("ZBXGov: not yet determined"));
        gov.totalSupplyAt(block.number);
    }

    /// 2. totalSupplyAt of any past block returns 0 before any mint.
    function test_TotalSupplyAt_ZeroBeforeAnyMint() public {
        hevm.roll(block.number + 5);
        require(gov.totalSupplyAt(block.number - 1) == 0, "expected zero pre-mint");
    }

    /// 3. totalSupplyAt reflects the supply written by mint().
    function test_TotalSupplyAt_TracksMint() public {
        gov.mint(address(0xA1), 1_000e18);
        uint256 mintBlock = block.number;
        hevm.roll(block.number + 1);
        require(gov.totalSupplyAt(mintBlock) == 1_000e18, "tracks mint");
    }

    /// 4. totalSupplyAt reflects the supply written by burn().
    function test_TotalSupplyAt_TracksBurn() public {
        gov.mint(address(0xA1), 1_000e18);
        gov.burn(address(0xA1),   400e18);
        uint256 burnBlock = block.number;
        hevm.roll(block.number + 1);
        require(gov.totalSupplyAt(burnBlock) == 600e18, "tracks burn");
    }

    /// 5. Binary-search across several block-separated checkpoints.
    function test_TotalSupplyAt_BinarySearchAcrossBlocks() public {
        gov.mint(address(0xA1), 100e18);  uint256 b1 = block.number;
        hevm.roll(block.number + 3);
        gov.mint(address(0xA2), 200e18);  uint256 b2 = block.number;
        hevm.roll(block.number + 5);
        gov.mint(address(0xA3), 400e18);  uint256 b3 = block.number;
        hevm.roll(block.number + 1);

        require(gov.totalSupplyAt(b1)     == 100e18, "snapshot at b1");
        require(gov.totalSupplyAt(b1 + 1) == 100e18, "no-op block returns prior");
        require(gov.totalSupplyAt(b2)     == 300e18, "snapshot at b2");
        require(gov.totalSupplyAt(b3)     == 700e18, "snapshot at b3");
    }

    /// 6. Two writes in the SAME block produce ONE checkpoint (overwrite).
    function test_TotalSupplyAt_SameBlockOverwrite() public {
        gov.mint(address(0xA1), 100e18);
        gov.mint(address(0xA2), 250e18);
        uint256 sameBlock = block.number;
        hevm.roll(block.number + 1);
        require(gov.totalSupplyAt(sameBlock) == 350e18, "overwrite same-block");
    }

    // ────────────────────────────────────────────────────────────────────────
    //  ZbxGovernor wiring — S22a closes the stub-zero governance gap
    // ────────────────────────────────────────────────────────────────────────

    /// 7. KEY FIX: castVote at the first Active block (block.number ==
    ///    startBlock) MUST NOT revert. Pre-S22a fix this called
    ///    getPriorVotes(voter, startBlock) which fails the
    ///    `blockNumber < block.number` guard. S22a uses startBlock - 1.
    function test_Governor_CastVote_AtFirstActiveBlock_NoRevert() public {
        address proposer = address(0xA1);

        // Block N: mint 200 ZBXGov to proposer (auto-delegates to self
        // because delegates[proposer] == address(0)).
        gov.mint(proposer, 200e18);

        // Block N+1: propose. propose() reads _getVotes(self, block.number-1)
        // which queries the checkpoint at block N (= 200e18 ≥ 100e18 threshold).
        hevm.roll(block.number + 1);

        address[] memory targets   = new address[](1); targets[0]   = address(this);
        uint256[] memory values    = new uint256[](1); values[0]    = 0;
        bytes[]   memory calldatas = new bytes[](1);   calldatas[0] = "";

        hevm.prank(proposer);
        uint256 id = governor.propose(targets, values, calldatas, "S22a-active-vote");

        // Block N+2 == startBlock (votingDelay=1). state() should be Active.
        hevm.roll(block.number + 1);
        require(governor.state(id) == ZbxGovernor.ProposalState.Active, "must be Active");

        // KEY: this castVote MUST NOT revert. Snapshot is startBlock-1 (= N+1)
        // which IS strictly past, so getPriorVotes succeeds and returns 200e18.
        hevm.prank(proposer);
        uint256 weight = governor.castVote(id, 1);
        require(weight == 200e18, "expected snapshot weight 200e18");
    }

    /// 8. ProposalThreshold is enforced against REAL votes (was stub-zero).
    function test_Governor_ProposalThreshold_EnforcedWithRealVotes() public {
        address tooSmall = address(0xB1);
        gov.mint(tooSmall, 50e18);     // less than 100e18 threshold
        hevm.roll(block.number + 1);

        address[] memory targets   = new address[](1); targets[0]   = address(this);
        uint256[] memory values    = new uint256[](1); values[0]    = 0;
        bytes[]   memory calldatas = new bytes[](1);   calldatas[0] = "";

        hevm.prank(tooSmall);
        hevm.expectRevert(bytes("Governor: below proposal threshold"));
        governor.propose(targets, values, calldatas, "below-threshold");
    }

    /// 9. Quorum uses snapshot supply (= startBlock - 1), NOT current supply.
    ///    Mint MORE supply at startBlock — quorum denominator must remain at
    ///    the pre-vote snapshot, not be inflated by post-snapshot mints.
    function test_Governor_Quorum_UsesSnapshotSupply_NotCurrent() public {
        address proposer = address(0xA1);
        gov.mint(proposer, 200e18);             // block N: snapshot baseline
        hevm.roll(block.number + 1);            // block N+1

        address[] memory targets   = new address[](1); targets[0]   = address(this);
        uint256[] memory values    = new uint256[](1); values[0]    = 0;
        bytes[]   memory calldatas = new bytes[](1);   calldatas[0] = "";

        hevm.prank(proposer);
        uint256 id = governor.propose(targets, values, calldatas, "snapshot-quorum");
        // votingDelay = 1, so startBlock = block.number + 1 ⇒ snapshot
        // (= startBlock - 1) = current block.number.
        uint256 snapshotBlock = block.number;

        hevm.roll(block.number + 1);            // block N+2 == startBlock

        // Mint 1_000_000e18 MORE at startBlock. Snapshot is startBlock-1
        // (=N+1) so quorum must still see only 200e18 of supply.
        gov.mint(address(0xDEAD), 1_000_000e18);

        // Cast 200e18 For-vote. forVotes = 200e18.
        hevm.prank(proposer);
        governor.castVote(id, 1);

        // Quorum at snapshot block = 4% of 200e18 = 8e18 (not 4% of
        // 1_000_200e18 from the post-snapshot mint).
        require(governor.quorum(snapshotBlock) == 8e18, "quorum 4% of 200e18");

        // Roll past endBlock and assert Succeeded — proves quorum check
        // (which uses startBlock - 1 internally) does not revert and
        // resolves true with the snapshot supply.
        hevm.roll(block.number + governor.votingPeriod() + 1);
        require(governor.state(id) == ZbxGovernor.ProposalState.Succeeded, "succeeded");
    }

    /// 10. _getVotes is no longer hard-coded zero — propose() now succeeds
    ///     when the proposer holds enough ZBXGov, which would have been
    ///     impossible if _getVotes still returned 0.
    function test_Governor_GetVotes_NoLongerStubZero() public {
        address proposer = address(0xA1);
        gov.mint(proposer, 100e18);     // exactly threshold
        hevm.roll(block.number + 1);

        address[] memory targets   = new address[](1); targets[0]   = address(this);
        uint256[] memory values    = new uint256[](1); values[0]    = 0;
        bytes[]   memory calldatas = new bytes[](1);   calldatas[0] = "";

        hevm.prank(proposer);
        uint256 id = governor.propose(targets, values, calldatas, "exact-threshold");
        require(id == 1, "first proposal");
    }

    /// 11. Vote weight is taken at the SNAPSHOT, not at vote-cast time.
    ///     Mint more votes AFTER the snapshot — they MUST NOT count.
    function test_Governor_VoteWeight_FromSnapshotNotCurrent() public {
        address proposer = address(0xA1);
        gov.mint(proposer, 150e18);        // snapshot baseline: 150e18
        hevm.roll(block.number + 1);

        address[] memory targets   = new address[](1); targets[0]   = address(this);
        uint256[] memory values    = new uint256[](1); values[0]    = 0;
        bytes[]   memory calldatas = new bytes[](1);   calldatas[0] = "";

        hevm.prank(proposer);
        uint256 id = governor.propose(targets, values, calldatas, "snapshot-weight");

        hevm.roll(block.number + 1);       // block.number == startBlock
        gov.mint(proposer, 999_000e18);    // post-snapshot mint — must NOT count

        hevm.prank(proposer);
        uint256 weight = governor.castVote(id, 1);
        require(weight == 150e18, "snapshot weight, not inflated");
    }

    // ────────────────────────────────────────────────────────────────────────
    //  S22a-fix1 regression — Compound boundary guards in checkpoint readers.
    //  These tests would all FAIL pre-S22a-fix1 because the binary-search
    //  final-return would wrongly return the first checkpoint's value when
    //  the queried block is strictly before any checkpoint exists.
    // ────────────────────────────────────────────────────────────────────────

    /// 12. getPriorVotes for a block BEFORE the first checkpoint must return 0
    ///     — NOT the first checkpoint's votes. Pre-fix this returned 100e18.
    function test_GetPriorVotes_QueryBeforeFirstCheckpoint_ReturnsZero() public {
        hevm.roll(block.number + 10);                 // advance so we have a "past"
        gov.mint(address(0xC1), 100e18);              // first (and only) checkpoint here
        uint256 mintBlock = block.number;
        hevm.roll(block.number + 1);

        // Query 5 blocks BEFORE the only checkpoint.
        require(
            gov.getPriorVotes(address(0xC1), mintBlock - 5) == 0,
            "S22a-fix1: pre-first-checkpoint must be 0 (was bug: returned cp[0].votes)"
        );
        // Sanity: at the mint block itself returns 100e18.
        require(
            gov.getPriorVotes(address(0xC1), mintBlock) == 100e18,
            "at-mint returns minted"
        );
    }

    /// 13. totalSupplyAt for a block BEFORE the first checkpoint must return 0.
    function test_TotalSupplyAt_QueryBeforeFirstCheckpoint_ReturnsZero() public {
        hevm.roll(block.number + 10);
        gov.mint(address(0xC1), 100e18);
        uint256 mintBlock = block.number;
        hevm.roll(block.number + 1);

        require(
            gov.totalSupplyAt(mintBlock - 5) == 0,
            "S22a-fix1: pre-first-checkpoint must be 0"
        );
        require(
            gov.totalSupplyAt(mintBlock) == 100e18,
            "at-mint returns supply"
        );
    }

    /// 14. KEY SECURITY TEST — governance bypass via post-snapshot mint.
    ///     Voter has ZERO checkpoints at the snapshot block; later mints
    ///     them voting power; voter tries to vote on the proposal whose
    ///     snapshot was BEFORE the mint. Weight MUST be 0 (the snapshot
    ///     correctly says they had no votes).
    function test_Governor_PostSnapshotMint_VoterGetsZeroWeight() public {
        address proposer = address(0xA1);
        address attacker = address(0xC1);             // never minted to before snapshot

        gov.mint(proposer, 200e18);                    // proposer meets threshold
        hevm.roll(block.number + 1);

        address[] memory targets   = new address[](1); targets[0]   = address(this);
        uint256[] memory values    = new uint256[](1); values[0]    = 0;
        bytes[]   memory calldatas = new bytes[](1);   calldatas[0] = "";

        hevm.prank(proposer);
        uint256 id = governor.propose(targets, values, calldatas, "post-snapshot-mint-attack");
        // snapshotBlock = block.number now (votingDelay = 1)

        hevm.roll(block.number + 1);                   // block.number == startBlock

        // POST-SNAPSHOT MINT: attacker had no checkpoints at snapshot;
        // mint now gives them 1_000_000e18 (would dwarf forVotes if it counted).
        gov.mint(attacker, 1_000_000e18);

        // attacker votes For — pre-S22a-fix1 the buggy getPriorVotes
        // would return 1_000_000e18 (their first-and-only checkpoint at
        // block.number, which is AFTER snapshot). Post-fix returns 0.
        hevm.prank(attacker);
        uint256 weight = governor.castVote(id, 1);
        require(weight == 0, "S22a-fix1: post-snapshot first-mint must yield 0 weight");
    }

    // Helpers section removed in self-review: the auto-generated public
    // mapping getter for `proposals(uint256)` returns 10 fields (id,
    // proposer, description, startBlock, endBlock, forVotes, againstVotes,
    // abstainVotes, cancelled, executed — dynamic arrays are skipped but
    // string is included), making position-based destructuring fragile.
    // Tests 9 and 11 instead track the snapshot block inline using the
    // votingDelay-1 invariant (snapshotBlock = block.number after propose).
}
