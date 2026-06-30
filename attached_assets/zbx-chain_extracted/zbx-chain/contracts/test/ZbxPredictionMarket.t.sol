// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxPredictionMarket.sol";

contract MockPredToken {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    function mint(address to, uint256 amt) external { balanceOf[to] += amt; }
    function transfer(address to, uint256 amt) external returns (bool) {
        require(balanceOf[msg.sender] >= amt);
        balanceOf[msg.sender] -= amt;
        balanceOf[to] += amt;
        return true;
    }
    function transferFrom(address from, address to, uint256 amt) external returns (bool) {
        require(balanceOf[from] >= amt && allowance[from][msg.sender] >= amt);
        allowance[from][msg.sender] -= amt;
        balanceOf[from] -= amt;
        balanceOf[to] += amt;
        return true;
    }
    function approve(address s, uint256 a) external returns (bool) {
        allowance[msg.sender][s] = a;
        return true;
    }
}

contract ZbxPredictionMarketTest is Test {
    ZbxPredictionMarket market;
    MockPredToken token;

    address admin    = address(this);
    address creator  = address(0xC4EA704);
    address alice    = address(0xA11CE);
    address bob      = address(0xB0B);
    address resolver = address(0xE5017E4);

    uint256 constant PROTOCOL_FEE_BPS = 200; // 2%

    function setUp() public {
        token  = new MockPredToken();
        market = new ZbxPredictionMarket(admin, PROTOCOL_FEE_BPS);

        token.mint(alice,   100_000 ether);
        token.mint(bob,     100_000 ether);
        token.mint(creator, 100_000 ether);

        vm.prank(alice);   token.approve(address(market), type(uint256).max);
        vm.prank(bob);     token.approve(address(market), type(uint256).max);
        vm.prank(creator); token.approve(address(market), type(uint256).max);
    }

    function _createMarket() internal returns (uint256 id) {
        vm.prank(creator);
        id = market.createMarket(
            "Will ZBX reach $1 by 2027?",
            address(token),
            resolver,
            block.timestamp + 7 days
        );
    }

    // ── Create ────────────────────────────────────────────────────────────

    function test_create_market() public {
        uint256 id = _createMarket();
        assertGt(id, 0);
    }

    function test_create_with_zero_token_reverts() public {
        vm.prank(creator);
        vm.expectRevert();
        market.createMarket("Test?", address(0), resolver, block.timestamp + 1 days);
    }

    function test_create_deadline_in_past_reverts() public {
        vm.prank(creator);
        vm.expectRevert();
        market.createMarket("Past?", address(token), resolver, block.timestamp - 1);
    }

    // ── Bet ───────────────────────────────────────────────────────────────

    function test_bet_yes() public {
        uint256 id = _createMarket();
        vm.prank(alice);
        market.bet(id, true, 1_000 ether);
        assertGt(market.yesBets(id, alice), 0);
    }

    function test_bet_no() public {
        uint256 id = _createMarket();
        vm.prank(bob);
        market.bet(id, false, 1_000 ether);
        assertGt(market.noBets(id, bob), 0);
    }

    function test_bet_zero_reverts() public {
        uint256 id = _createMarket();
        vm.prank(alice);
        vm.expectRevert();
        market.bet(id, true, 0);
    }

    function test_bet_after_deadline_reverts() public {
        uint256 id = _createMarket();
        vm.warp(block.timestamp + 8 days);
        vm.prank(alice);
        vm.expectRevert();
        market.bet(id, true, 1_000 ether);
    }

    // ── Resolve ───────────────────────────────────────────────────────────

    function test_resolve_yes() public {
        uint256 id = _createMarket();
        vm.prank(alice); market.bet(id, true,  5_000 ether);
        vm.prank(bob);   market.bet(id, false, 1_000 ether);
        vm.warp(block.timestamp + 8 days);
        vm.prank(resolver);
        market.resolve(id, ZbxPredictionMarket.Outcome.YES);
        assertEq(uint8(market.outcome(id)), uint8(ZbxPredictionMarket.Outcome.YES));
    }

    function test_non_resolver_cannot_resolve() public {
        uint256 id = _createMarket();
        vm.warp(block.timestamp + 8 days);
        vm.prank(alice);
        vm.expectRevert();
        market.resolve(id, ZbxPredictionMarket.Outcome.YES);
    }

    function test_double_resolve_reverts() public {
        uint256 id = _createMarket();
        vm.warp(block.timestamp + 8 days);
        vm.prank(resolver);
        market.resolve(id, ZbxPredictionMarket.Outcome.YES);
        vm.prank(resolver);
        vm.expectRevert();
        market.resolve(id, ZbxPredictionMarket.Outcome.NO);
    }

    // ── Claim ─────────────────────────────────────────────────────────────

    function test_winner_claims_reward() public {
        uint256 id = _createMarket();
        vm.prank(alice); market.bet(id, true, 5_000 ether);
        vm.prank(bob);   market.bet(id, false, 1_000 ether);
        vm.warp(block.timestamp + 8 days);
        vm.prank(resolver); market.resolve(id, ZbxPredictionMarket.Outcome.YES);

        uint256 before = token.balanceOf(alice);
        vm.prank(alice);
        market.claim(id);
        assertGt(token.balanceOf(alice), before);
    }

    function test_loser_cannot_claim() public {
        uint256 id = _createMarket();
        vm.prank(alice); market.bet(id, true, 5_000 ether);
        vm.prank(bob);   market.bet(id, false, 1_000 ether);
        vm.warp(block.timestamp + 8 days);
        vm.prank(resolver); market.resolve(id, ZbxPredictionMarket.Outcome.YES);

        uint256 before = token.balanceOf(bob);
        vm.prank(bob);
        vm.expectRevert();
        market.claim(id);
    }

    function test_double_claim_reverts() public {
        uint256 id = _createMarket();
        vm.prank(alice); market.bet(id, true, 5_000 ether);
        vm.warp(block.timestamp + 8 days);
        vm.prank(resolver); market.resolve(id, ZbxPredictionMarket.Outcome.YES);
        vm.prank(alice); market.claim(id);
        vm.prank(alice);
        vm.expectRevert();
        market.claim(id);
    }

    // ── Void ──────────────────────────────────────────────────────────────

    function test_void_returns_all_bets() public {
        uint256 id = _createMarket();
        vm.prank(alice); market.bet(id, true, 5_000 ether);
        vm.prank(bob);   market.bet(id, false, 1_000 ether);
        vm.warp(block.timestamp + 8 days);
        vm.prank(resolver); market.resolve(id, ZbxPredictionMarket.Outcome.VOID);

        uint256 beforeAlice = token.balanceOf(alice);
        vm.prank(alice); market.claim(id);
        assertApproxEqRel(token.balanceOf(alice), beforeAlice + 5_000 ether, 1e15);
    }
}
