// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxLaunchpad.sol";

contract MockSaleToken {
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

contract ZbxLaunchpadTest is Test {
    ZbxLaunchpad launchpad;
    MockSaleToken saleToken;    // token being sold
    MockSaleToken raiseToken;   // token used to buy

    address admin   = address(this);
    address project = address(0xA);
    address alice   = address(0xB);
    address bob     = address(0xC);

    function setUp() public {
        launchpad  = new ZbxLaunchpad(admin);
        saleToken  = new MockSaleToken();
        raiseToken = new MockSaleToken();

        // Fund project with sale tokens
        saleToken.mint(project,  1_000_000 ether);
        raiseToken.mint(alice,   100_000 ether);
        raiseToken.mint(bob,     100_000 ether);

        vm.prank(project);
        saleToken.approve(address(launchpad), type(uint256).max);
        vm.prank(alice);
        raiseToken.approve(address(launchpad), type(uint256).max);
        vm.prank(bob);
        raiseToken.approve(address(launchpad), type(uint256).max);
    }

    function _createSale() internal returns (uint256 id) {
        vm.prank(project);
        id = launchpad.createSale(
            address(saleToken),
            address(raiseToken),
            1_000_000 ether,     // tokensForSale
            1e15,                // price: 0.001 raiseToken per saleToken
            block.timestamp + 1 hours,   // startTime
            block.timestamp + 7 days,    // endTime
            block.timestamp + 7 days + 30 days,  // cliff
            180 days,            // vestingDuration
            ZbxLaunchpad.SaleMode.FCFS
        );
    }

    // ── Create ────────────────────────────────────────────────────────────

    function test_create_sale() public {
        uint256 id = _createSale();
        assertGt(id, 0);
    }

    function test_create_sale_transfers_tokens_to_launchpad() public {
        _createSale();
        assertEq(saleToken.balanceOf(address(launchpad)), 1_000_000 ether);
    }

    function test_create_with_zero_tokens_reverts() public {
        vm.prank(project);
        vm.expectRevert();
        launchpad.createSale(
            address(saleToken), address(raiseToken),
            0, 1e15,
            block.timestamp + 1 hours, block.timestamp + 7 days,
            block.timestamp + 7 days + 30 days, 180 days,
            ZbxLaunchpad.SaleMode.FCFS
        );
    }

    // ── Whitelist ─────────────────────────────────────────────────────────

    function test_whitelist_users() public {
        uint256 id = _createSale();
        address[] memory users = new address[](2);
        users[0] = alice;
        users[1] = bob;
        vm.prank(project);
        launchpad.updateWhitelist(id, users, true);
        assertTrue(launchpad.isWhitelisted(id, alice));
        assertTrue(launchpad.isWhitelisted(id, bob));
    }

    function test_non_project_cannot_update_whitelist() public {
        uint256 id = _createSale();
        address[] memory users = new address[](1);
        users[0] = alice;
        vm.prank(alice);
        vm.expectRevert();
        launchpad.updateWhitelist(id, users, true);
    }

    // ── Participate ───────────────────────────────────────────────────────

    function test_whitelisted_user_can_participate() public {
        uint256 id = _createSale();
        address[] memory users = new address[](1);
        users[0] = alice;
        vm.prank(project);
        launchpad.updateWhitelist(id, users, true);

        vm.warp(block.timestamp + 2 hours); // within sale window
        vm.prank(alice);
        launchpad.participate(id, 100 ether); // spend 100 raiseToken
        assertGt(launchpad.participated(id, alice), 0);
    }

    function test_non_whitelisted_cannot_participate() public {
        uint256 id = _createSale();
        vm.warp(block.timestamp + 2 hours);
        vm.prank(alice);
        vm.expectRevert();
        launchpad.participate(id, 100 ether);
    }

    function test_participate_before_start_reverts() public {
        uint256 id = _createSale();
        address[] memory users = new address[](1);
        users[0] = alice;
        vm.prank(project);
        launchpad.updateWhitelist(id, users, true);
        // Not yet started
        vm.prank(alice);
        vm.expectRevert();
        launchpad.participate(id, 100 ether);
    }

    function test_participate_after_end_reverts() public {
        uint256 id = _createSale();
        address[] memory users = new address[](1);
        users[0] = alice;
        vm.prank(project);
        launchpad.updateWhitelist(id, users, true);
        vm.warp(block.timestamp + 8 days); // after sale end
        vm.prank(alice);
        vm.expectRevert();
        launchpad.participate(id, 100 ether);
    }

    // ── Finalize + Claim ─────────────────────────────────────────────────

    function test_finalize_and_claim_vested_tokens() public {
        uint256 id = _createSale();
        address[] memory users = new address[](1);
        users[0] = alice;
        vm.prank(project);
        launchpad.updateWhitelist(id, users, true);
        vm.warp(block.timestamp + 2 hours);
        vm.prank(alice);
        launchpad.participate(id, 1_000 ether);

        vm.warp(block.timestamp + 8 days); // after end
        vm.prank(project);
        launchpad.finalize(id);

        // Warp past cliff
        vm.warp(block.timestamp + 8 days + 30 days + 90 days); // 50% vested

        uint256 before = saleToken.balanceOf(alice);
        vm.prank(alice);
        launchpad.claim(id);
        assertGt(saleToken.balanceOf(alice), before);
    }
}
