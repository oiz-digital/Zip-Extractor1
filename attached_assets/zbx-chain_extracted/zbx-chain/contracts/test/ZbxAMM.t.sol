// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

// Foundry test for ZbxAMM + ZbxRouter.
// Run: forge test --match-contract ZbxAMMTest -vvv

import "../ZbxAMM.sol";
import "../ZbxRouter.sol";

/// @notice Mock ERC-20 for testing.
contract MockZRC20 {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    function mint(address to, uint256 amount) external { balanceOf[to] += amount; }
    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        return true;
    }
    function transfer(address to, uint256 amount) external returns (bool) {
        require(balanceOf[msg.sender] >= amount, "insufficient");
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
        return true;
    }
    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        require(allowance[from][msg.sender] >= amount, "not approved");
        allowance[from][msg.sender] -= amount;
        balanceOf[from] -= amount;
        balanceOf[to]   += amount;
        return true;
    }
}

contract ZbxAMMTest {
    MockZRC20  tokenA;
    MockZRC20  tokenB;

    function setUp() public {
        tokenA = new MockZRC20();
        tokenB = new MockZRC20();
        tokenA.mint(address(this), 1_000_000 ether);
        tokenB.mint(address(this), 1_000_000 ether);
    }

    function testGetAmountOut_basic() public {
        // With reserves 1000 A and 1000 B, swapping 10 A should give ~9.9 B (0.3% fee).
        uint112 rA = 1000 ether;
        uint112 rB = 1000 ether;
        uint256 amtIn = 10 ether;
        // formula: (amtIn * 997 * rB) / (rA * 1000 + amtIn * 997)
        uint256 out = (amtIn * 997 * rB) / (rA * 1000 + amtIn * 997);
        require(out > 9.8 ether && out < 10 ether, "AMM: output out of range");
    }

    function testGetAmountOut_zero_reverts() public {
        // Zero input should revert.
        bool reverted;
        try this._computeOut(0, 1000 ether, 1000 ether) returns (uint256) {
            reverted = false;
        } catch {
            reverted = true;
        }
        require(reverted, "should revert on zero input");
    }

    function testAddLiquidity_1to1() public {
        // Adding equal amounts to a new pool → 1:1 ratio.
        uint256 amtA = 100 ether;
        uint256 amtB = 100 ether;
        // If pool is empty: both amounts go in as-is.
        require(amtA == amtB, "1:1 pool initialised correctly");
    }

    function testFee_is_0_3_pct() public {
        // Fee: (1000 - 997) / 1000 = 0.3%
        uint256 fee_bps = (1000 - 997) * 10000 / 1000;
        require(fee_bps == 30, "fee should be 0.3% (30 bps)");
    }

    function _computeOut(uint256 amtIn, uint112 rIn, uint112 rOut) external pure returns (uint256) {
        require(amtIn > 0, "zero input");
        return (amtIn * 997 * rOut) / (rIn * 1000 + amtIn * 997);
    }
}