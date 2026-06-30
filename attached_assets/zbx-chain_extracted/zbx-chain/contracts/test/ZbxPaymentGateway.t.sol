// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxPaymentGateway.sol";

contract MockPayToken {
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

contract MockSwapRouter {
    function swapTokensForExactTokens(
        uint256 amountOut, uint256, address[] calldata path, address to, uint256
    ) external returns (uint256[] memory amounts) {
        amounts = new uint256[](path.length);
        amounts[path.length - 1] = amountOut;
        // Simulate transferring output
        MockPayToken(path[path.length - 1]).mint(to, amountOut);
    }
}

contract ZbxPaymentGatewayTest is Test {
    ZbxPaymentGateway gateway;
    MockPayToken      zbx;
    MockPayToken      usdt;
    MockSwapRouter    router;

    address admin    = address(this);
    address merchant = address(0xAB);
    address customer = address(0xCD);
    bytes32 merchantId;

    function setUp() public {
        zbx    = new MockPayToken();
        usdt   = new MockPayToken();
        router = new MockSwapRouter();

        gateway = new ZbxPaymentGateway(admin, address(router));

        zbx.mint(customer, 100_000 ether);
        usdt.mint(customer, 100_000 ether);
        vm.prank(customer);
        zbx.approve(address(gateway), type(uint256).max);
        vm.prank(customer);
        usdt.approve(address(gateway), type(uint256).max);
    }

    // ── Merchant registration ────────────────────────────────────────────

    function test_register_merchant() public {
        vm.prank(merchant);
        merchantId = gateway.registerMerchant("ZBX Store", merchant);
        assertTrue(merchantId != bytes32(0));
    }

    function test_register_merchant_zero_payout_reverts() public {
        vm.prank(merchant);
        vm.expectRevert();
        gateway.registerMerchant("Store", address(0));
    }

    function test_update_payout_address() public {
        vm.prank(merchant);
        merchantId = gateway.registerMerchant("Store", merchant);
        address newPayout = address(0xNEW);
        vm.prank(merchant);
        gateway.updatePayoutAddress(merchantId, newPayout);
        assertEq(gateway.merchantPayout(merchantId), newPayout);
    }

    function test_non_merchant_cannot_update_payout() public {
        vm.prank(merchant);
        merchantId = gateway.registerMerchant("Store", merchant);
        vm.prank(customer);
        vm.expectRevert();
        gateway.updatePayoutAddress(merchantId, customer);
    }

    // ── Invoice ───────────────────────────────────────────────────────────

    function test_create_invoice() public {
        vm.prank(merchant);
        merchantId = gateway.registerMerchant("Store", merchant);
        vm.prank(merchant);
        bytes32 invId = gateway.createInvoice(
            merchantId, address(zbx), 1_000 ether, "Order #1", block.timestamp + 1 days
        );
        assertTrue(invId != bytes32(0));
    }

    function test_create_invoice_expired_deadline_reverts() public {
        vm.prank(merchant);
        merchantId = gateway.registerMerchant("Store", merchant);
        vm.prank(merchant);
        vm.expectRevert();
        gateway.createInvoice(merchantId, address(zbx), 1_000 ether, "X", block.timestamp - 1);
    }

    // ── Pay ───────────────────────────────────────────────────────────────

    function test_pay_invoice() public {
        vm.prank(merchant);
        merchantId = gateway.registerMerchant("Store", merchant);
        vm.prank(merchant);
        bytes32 invId = gateway.createInvoice(
            merchantId, address(zbx), 1_000 ether, "Order", block.timestamp + 1 days
        );
        uint256 before = zbx.balanceOf(merchant);
        vm.prank(customer);
        gateway.pay(invId, 1_000 ether);
        assertGt(zbx.balanceOf(merchant), before);
    }

    function test_pay_twice_reverts() public {
        vm.prank(merchant);
        merchantId = gateway.registerMerchant("Store", merchant);
        vm.prank(merchant);
        bytes32 invId = gateway.createInvoice(
            merchantId, address(zbx), 100 ether, "Order", block.timestamp + 1 days
        );
        vm.prank(customer);
        gateway.pay(invId, 100 ether);
        vm.prank(customer);
        vm.expectRevert();
        gateway.pay(invId, 100 ether);
    }

    // ── Cancel ────────────────────────────────────────────────────────────

    function test_cancel_invoice() public {
        vm.prank(merchant);
        merchantId = gateway.registerMerchant("Store", merchant);
        vm.prank(merchant);
        bytes32 invId = gateway.createInvoice(
            merchantId, address(zbx), 100 ether, "X", block.timestamp + 1 days
        );
        vm.prank(merchant);
        gateway.cancelInvoice(invId);
        // Paying cancelled invoice should revert
        vm.prank(customer);
        vm.expectRevert();
        gateway.pay(invId, 100 ether);
    }
}
