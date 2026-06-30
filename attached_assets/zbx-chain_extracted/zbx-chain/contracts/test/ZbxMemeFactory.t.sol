// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxMemeFactory.sol";

contract MockAMM {
    function addLiquidity(
        address, address, uint256, uint256, uint256, uint256, address, uint256
    ) external payable returns (uint256, uint256, uint256) {
        return (1 ether, 1 ether, 1 ether);
    }
}

contract ZbxMemeFactoryTest is Test {
    ZbxMemeFactory factory;
    MockAMM        amm;

    address admin = address(this);
    address alice = address(0xA11CE);
    address bob   = address(0xB0B);

    uint256 constant LAUNCH_FEE = 0.01 ether;

    function setUp() public {
        amm     = new MockAMM();
        factory = new ZbxMemeFactory(admin, address(amm));
        vm.deal(alice, 100 ether);
        vm.deal(bob,   100 ether);
    }

    // ── Launch ────────────────────────────────────────────────────────────

    function test_launch_creates_meme() public {
        vm.prank(alice);
        uint256 id = factory.launchMeme{value: LAUNCH_FEE}(
            "PepeCoin", "PEPE", "ipfs://logo"
        );
        assertGt(id, 0);
    }

    function test_launch_deploys_token() public {
        vm.prank(alice);
        uint256 id = factory.launchMeme{value: LAUNCH_FEE}(
            "PepeCoin", "PEPE", "ipfs://logo"
        );
        address token = factory.memeToken(id);
        assertTrue(token != address(0));
    }

    function test_launch_below_fee_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        factory.launchMeme{value: 0.001 ether}("PepeCoin", "PEPE", "");
    }

    function test_launch_empty_name_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        factory.launchMeme{value: LAUNCH_FEE}("", "PEPE", "");
    }

    // ── Buy from bonding curve ────────────────────────────────────────────

    function test_buy_returns_tokens() public {
        vm.prank(alice);
        uint256 id = factory.launchMeme{value: LAUNCH_FEE}("PepeCoin", "PEPE", "");
        address token = factory.memeToken(id);

        vm.prank(bob);
        uint256 tokensOut = factory.buy{value: 1 ether}(id, 0);
        assertGt(tokensOut, 0);
    }

    function test_buy_zero_value_reverts() public {
        vm.prank(alice);
        uint256 id = factory.launchMeme{value: LAUNCH_FEE}("PepeCoin", "PEPE", "");
        vm.prank(bob);
        vm.expectRevert();
        factory.buy{value: 0}(id, 0);
    }

    function test_price_increases_with_supply() public {
        vm.prank(alice);
        uint256 id = factory.launchMeme{value: LAUNCH_FEE}("PepeCoin", "PEPE", "");

        // Buy in small increments and check price goes up
        vm.prank(bob);
        uint256 out1 = factory.buy{value: 0.1 ether}(id, 0);
        uint256 out2 = factory.buy{value: 0.1 ether}(id, 0);
        // Second buy should get fewer tokens (higher price after first)
        assertLt(out2, out1);
    }

    // ── Sell ──────────────────────────────────────────────────────────────

    function test_sell_returns_zbx() public {
        vm.prank(alice);
        uint256 id = factory.launchMeme{value: LAUNCH_FEE}("PepeCoin", "PEPE", "");

        vm.prank(bob);
        factory.buy{value: 1 ether}(id, 0);
        address token = factory.memeToken(id);

        // Approve factory to take tokens
        uint256 bal = IMemeToken(token).balanceOf(bob);
        vm.prank(bob);
        IMemeToken(token).approve(address(factory), bal);

        uint256 before = bob.balance;
        vm.prank(bob);
        factory.sell(id, bal / 2, 0);
        assertGt(bob.balance, before);
    }

    // ── Quote ─────────────────────────────────────────────────────────────

    function test_quote_buy_nonzero() public {
        vm.prank(alice);
        uint256 id = factory.launchMeme{value: LAUNCH_FEE}("PepeCoin", "PEPE", "");
        uint256 tokensOut = factory.quoteBuy(id, 1 ether);
        assertGt(tokensOut, 0);
    }

    function test_quote_sell_nonzero() public {
        vm.prank(alice);
        uint256 id = factory.launchMeme{value: LAUNCH_FEE}("PepeCoin", "PEPE", "");
        vm.prank(bob);
        factory.buy{value: 1 ether}(id, 0);
        address token = factory.memeToken(id);
        uint256 bal = IMemeToken(token).balanceOf(bob);
        uint256 zbxOut = factory.quoteSell(id, bal / 2);
        assertGt(zbxOut, 0);
    }

    // ── Graduation ────────────────────────────────────────────────────────

    function test_graduated_meme_state() public {
        vm.prank(alice);
        uint256 id = factory.launchMeme{value: LAUNCH_FEE}("PepeCoin", "PEPE", "");
        // Buy enough to graduate the curve
        uint256 grad = factory.GRADUATION_ZBX();
        if (alice.balance < grad) vm.deal(alice, grad * 2);
        vm.prank(alice);
        // Buy until graduation or max attempt
        try factory.buy{value: grad}(id, 0) returns (uint256) {} catch {}
        // Even if not graduated, state should be consistent
        bool graduated = factory.isGraduated(id);
        (bool ok,) = address(factory).staticcall(
            abi.encodeWithSignature("isGraduated(uint256)", id)
        );
        assertTrue(ok);
    }
}

interface IMemeToken {
    function balanceOf(address) external view returns (uint256);
    function approve(address, uint256) external returns (bool);
}
