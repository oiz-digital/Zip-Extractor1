// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxGovernor.sol";

contract MockGovToken {
    mapping(address => uint256) private _bal;
    uint256 public totalSupply = 1_000_000 ether;

    function balanceOf(address a) external view returns (uint256) { return _bal[a]; }
    function getVotes(address a) external view returns (uint256)   { return _bal[a]; }
    function mint(address to, uint256 amt) external { _bal[to] += amt; }
    function delegate(address) external {}
}

contract MockTimelock {
    bool public called;
    function queue(address[] calldata, uint256[] calldata, bytes[] calldata, bytes32) external returns (bytes32) {
        called = true;
        return bytes32(0);
    }
    function execute(address[] calldata, uint256[] calldata, bytes[] calldata, bytes32) external {}
    function getMinDelay() external pure returns (uint256) { return 86400; }
}

contract ZbxGovernorTest is Test {
    ZbxGovernor gov;
    MockGovToken token;
    MockTimelock timelock;

    address alice = address(0xA11CE);
    address bob   = address(0xB0B);

    function setUp() public {
        token    = new MockGovToken();
        timelock = new MockTimelock();
        gov      = new ZbxGovernor(address(token), address(timelock));

        // Give alice enough tokens to propose
        token.mint(alice, 200e18);
        // Give bob votes for quorum
        token.mint(bob, 100_000e18);
    }

    function test_voting_params() public view {
        assertEq(gov.votingDelay(), 1);
        assertEq(gov.votingPeriod(), 50_400);
        assertGt(gov.proposalThreshold(), 0);
        assertGt(gov.quorumNumerator(), 0);
    }

    function test_propose_creates_proposal() public {
        address[] memory targets   = new address[](1);
        uint256[] memory values    = new uint256[](1);
        bytes[]   memory calldatas = new bytes[](1);
        targets[0] = address(0xDEAD);

        vm.prank(alice);
        uint256 id = gov.propose(targets, values, calldatas, "Test proposal");
        assertGt(id, 0);
    }

    function test_propose_below_threshold_reverts() public {
        address[] memory targets   = new address[](1);
        uint256[] memory values    = new uint256[](1);
        bytes[]   memory calldatas = new bytes[](1);

        vm.prank(address(0xPOOR));
        vm.expectRevert();
        gov.propose(targets, values, calldatas, "Spam");
    }

    function test_cast_vote() public {
        address[] memory targets   = new address[](1);
        uint256[] memory values    = new uint256[](1);
        bytes[]   memory calldatas = new bytes[](1);

        vm.prank(alice);
        uint256 id = gov.propose(targets, values, calldatas, "Vote test");

        vm.roll(block.number + gov.votingDelay() + 1);

        vm.prank(bob);
        gov.castVote(id, 1); // For

        (,,, uint256 forVotes,,) = gov.proposalVotes(id);
        assertGt(forVotes, 0);
    }

    function test_double_vote_reverts() public {
        address[] memory targets   = new address[](1);
        uint256[] memory values    = new uint256[](1);
        bytes[]   memory calldatas = new bytes[](1);

        vm.prank(alice);
        uint256 id = gov.propose(targets, values, calldatas, "Double vote");
        vm.roll(block.number + gov.votingDelay() + 1);

        vm.prank(bob);
        gov.castVote(id, 1);
        vm.prank(bob);
        vm.expectRevert();
        gov.castVote(id, 0);
    }

    function test_proposal_state_pending_initially() public {
        address[] memory targets   = new address[](1);
        uint256[] memory values    = new uint256[](1);
        bytes[]   memory calldatas = new bytes[](1);

        vm.prank(alice);
        uint256 id = gov.propose(targets, values, calldatas, "State test");
        assertEq(uint8(gov.state(id)), uint8(ZbxGovernor.ProposalState.Pending));
    }
}
