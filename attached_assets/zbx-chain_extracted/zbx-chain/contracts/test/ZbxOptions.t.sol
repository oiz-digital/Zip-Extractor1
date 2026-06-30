// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxOptions.sol";

contract MockOptionsOracle {
    mapping(address => uint256) private _prices;
    function setPrice(address asset, uint256 price) external { _prices[asset] = price; }
    function getPrice(address asset) external view returns (uint256) { return _prices[asset]; }
}

contract MockOptToken {
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

contract ZbxOptionsTest is Test {
    ZbxOptions options;
    MockOptionsOracle oracle;
    MockOptToken collateral;
    address underlying = address(0xU);

    address admin   = address(this);
    address writer  = address(0xBEEF);
    address buyer   = address(0xFACE);

    function setUp() public {
        oracle     = new MockOptionsOracle();
        collateral = new MockOptToken();
        options    = new ZbxOptions(admin, address(oracle));

        oracle.setPrice(underlying, 1_000 * 1e18); // $1000 per unit

        collateral.mint(writer, 100_000 ether);
        collateral.mint(buyer, 100_000 ether);
        vm.prank(writer); collateral.approve(address(options), type(uint256).max);
        vm.prank(buyer);  collateral.approve(address(options), type(uint256).max);
    }

    // ── Write call option ─────────────────────────────────────────────────

    function test_write_call_option() public {
        vm.prank(writer);
        uint256 id = options.writeCall(
            underlying,
            address(collateral),
            1 ether,           // size
            1_200 * 1e18,      // strike $1200
            block.timestamp + 7 days,
            0.01 ether         // premium
        );
        assertGt(id, 0);
    }

    function test_write_put_option() public {
        vm.prank(writer);
        uint256 id = options.writePut(
            underlying,
            address(collateral),
            1 ether,
            800 * 1e18,        // strike $800
            block.timestamp + 7 days,
            0.01 ether
        );
        assertGt(id, 0);
    }

    // ── Buy option ────────────────────────────────────────────────────────

    function test_buy_call_option() public {
        vm.prank(writer);
        uint256 id = options.writeCall(
            underlying, address(collateral),
            1 ether, 1_200 * 1e18,
            block.timestamp + 7 days, 0.01 ether
        );
        vm.prank(buyer);
        options.buy(id);
        assertEq(options.holder(id), buyer);
    }

    function test_cannot_buy_expired_option() public {
        vm.prank(writer);
        uint256 id = options.writeCall(
            underlying, address(collateral),
            1 ether, 1_200 * 1e18,
            block.timestamp + 1 days, 0.01 ether
        );
        vm.warp(block.timestamp + 2 days);
        vm.prank(buyer);
        vm.expectRevert();
        options.buy(id);
    }

    // ── Exercise ──────────────────────────────────────────────────────────

    function test_exercise_itm_call() public {
        vm.prank(writer);
        uint256 id = options.writeCall(
            underlying, address(collateral),
            1 ether, 1_200 * 1e18,
            block.timestamp + 7 days, 0.01 ether
        );
        vm.prank(buyer);
        options.buy(id);

        // Price rises above strike
        oracle.setPrice(underlying, 1_500 * 1e18);

        uint256 before = collateral.balanceOf(buyer);
        vm.prank(buyer);
        options.exercise(id);
        assertGt(collateral.balanceOf(buyer), before);
    }

    function test_exercise_otm_call_reverts() public {
        vm.prank(writer);
        uint256 id = options.writeCall(
            underlying, address(collateral),
            1 ether, 1_200 * 1e18,
            block.timestamp + 7 days, 0.01 ether
        );
        vm.prank(buyer);
        options.buy(id);

        // Price stays below strike
        oracle.setPrice(underlying, 900 * 1e18);

        vm.prank(buyer);
        vm.expectRevert();
        options.exercise(id);
    }

    // ── Expire ────────────────────────────────────────────────────────────

    function test_writer_reclaims_expired_collateral() public {
        vm.prank(writer);
        uint256 id = options.writeCall(
            underlying, address(collateral),
            1 ether, 1_200 * 1e18,
            block.timestamp + 7 days, 0.01 ether
        );
        // No buyer
        vm.warp(block.timestamp + 8 days);
        uint256 before = collateral.balanceOf(writer);
        vm.prank(writer);
        options.expire(id);
        assertGt(collateral.balanceOf(writer), before);
    }

    function test_expire_before_expiry_reverts() public {
        vm.prank(writer);
        uint256 id = options.writeCall(
            underlying, address(collateral),
            1 ether, 1_200 * 1e18,
            block.timestamp + 7 days, 0.01 ether
        );
        vm.prank(writer);
        vm.expectRevert();
        options.expire(id);
    }
}
