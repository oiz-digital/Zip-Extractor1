// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxSpotOrderBook.sol";

contract MockOBToken {
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
    function approve(address s, uint256 a) external returns (bool) { allowance[msg.sender][s] = a; return true; }
}

contract ZbxSpotOrderBookTest is Test {
    ZbxSpotOrderBook book;
    MockOBToken base;   // e.g. ZBX
    MockOBToken quote;  // e.g. USDT

    address admin = address(this);
    address alice = address(0xA11CE);
    address bob   = address(0xB0B);

    function setUp() public {
        base  = new MockOBToken();
        quote = new MockOBToken();
        book  = new ZbxSpotOrderBook(admin, address(base), address(quote), 10, 20); // 0.10% maker, 0.20% taker

        base.mint(alice, 1_000_000 ether);
        quote.mint(bob, 1_000_000 ether);

        vm.prank(alice); base.approve(address(book), type(uint256).max);
        vm.prank(bob);   quote.approve(address(book), type(uint256).max);
    }

    // ── Place order ───────────────────────────────────────────────────────

    function test_place_sell_order() public {
        vm.prank(alice);
        bytes32 id = book.placeOrder(
            ZbxSpotOrderBook.Side.SELL,
            10 ether,        // 10 ZBX
            500 * 1e18,      // at $500/ZBX
            block.timestamp + 1 days
        );
        assertTrue(id != bytes32(0));
        assertEq(book.orderStatus(id), ZbxSpotOrderBook.OrderStatus.Open);
    }

    function test_place_buy_order() public {
        vm.prank(bob);
        bytes32 id = book.placeOrder(
            ZbxSpotOrderBook.Side.BUY,
            10 ether,        // 10 ZBX worth
            500 * 1e18,
            block.timestamp + 1 days
        );
        assertTrue(id != bytes32(0));
    }

    function test_place_zero_amount_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        book.placeOrder(ZbxSpotOrderBook.Side.SELL, 0, 500 * 1e18, block.timestamp + 1 days);
    }

    function test_place_zero_price_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        book.placeOrder(ZbxSpotOrderBook.Side.SELL, 10 ether, 0, block.timestamp + 1 days);
    }

    // ── Cancel order ──────────────────────────────────────────────────────

    function test_cancel_order_refunds() public {
        vm.prank(alice);
        bytes32 id = book.placeOrder(
            ZbxSpotOrderBook.Side.SELL, 10 ether, 500 * 1e18, block.timestamp + 1 days
        );
        uint256 before = base.balanceOf(alice);
        vm.prank(alice);
        book.cancelOrder(id);
        assertEq(base.balanceOf(alice), before + 10 ether);
        assertEq(book.orderStatus(id), ZbxSpotOrderBook.OrderStatus.Cancelled);
    }

    function test_non_owner_cannot_cancel() public {
        vm.prank(alice);
        bytes32 id = book.placeOrder(
            ZbxSpotOrderBook.Side.SELL, 10 ether, 500 * 1e18, block.timestamp + 1 days
        );
        vm.prank(bob);
        vm.expectRevert();
        book.cancelOrder(id);
    }

    // ── Fill order ────────────────────────────────────────────────────────

    function test_fill_sell_order() public {
        vm.prank(alice);
        bytes32 id = book.placeOrder(
            ZbxSpotOrderBook.Side.SELL, 10 ether, 500 * 1e18, block.timestamp + 1 days
        );
        // Bob fills partial: 5 ZBX
        uint256 quoteCost = 5 * 500 * 1e18;
        vm.prank(bob);
        book.fillOrder{value: 0}(id, 5 ether);
        assertEq(book.remainingAmount(id), 5 ether);
    }

    // ── Expire order ──────────────────────────────────────────────────────

    function test_expire_order_after_deadline() public {
        vm.prank(alice);
        bytes32 id = book.placeOrder(
            ZbxSpotOrderBook.Side.SELL, 10 ether, 500 * 1e18, block.timestamp + 1 days
        );
        vm.warp(block.timestamp + 2 days);
        book.expireOrder(id);
        assertEq(book.orderStatus(id), ZbxSpotOrderBook.OrderStatus.Expired);
    }

    function test_expire_non_expired_reverts() public {
        vm.prank(alice);
        bytes32 id = book.placeOrder(
            ZbxSpotOrderBook.Side.SELL, 10 ether, 500 * 1e18, block.timestamp + 1 days
        );
        vm.expectRevert();
        book.expireOrder(id);
    }

    // ── Fee collection ────────────────────────────────────────────────────

    function test_fee_withdrawal() public {
        vm.prank(alice);
        bytes32 id = book.placeOrder(
            ZbxSpotOrderBook.Side.SELL, 10 ether, 500 * 1e18, block.timestamp + 1 days
        );
        vm.prank(bob);
        book.fillOrder(id, 5 ether);
        // Withdraw collected fees
        book.withdrawFees(address(base));
        book.withdrawFees(address(quote));
    }
}
