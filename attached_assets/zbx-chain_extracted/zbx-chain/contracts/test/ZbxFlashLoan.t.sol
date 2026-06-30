// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxFlashLoan.sol";

/// A flash loan receiver that repays immediately (no arbitrage)
contract GoodReceiver is IZbxFlashReceiver {
    function executeOperation(
        address asset, uint256 amount, uint256 fee, bytes calldata
    ) external override returns (bool) {
        // Repay amount + fee
        MockFlashToken(asset).mint(address(this), fee);
        IERC20Flash(asset).approve(msg.sender, amount + fee);
        return true;
    }
}

/// A flash loan receiver that does NOT repay (malicious)
contract BadReceiver is IZbxFlashReceiver {
    function executeOperation(address, uint256, uint256, bytes calldata) external override returns (bool) {
        return true; // lies — doesn't repay
    }
}

interface IERC20Flash {
    function approve(address, uint256) external returns (bool);
    function balanceOf(address) external view returns (uint256);
    function transfer(address, uint256) external returns (bool);
}

contract MockFlashToken {
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

contract ZbxFlashLoanTest is Test {
    ZbxFlashLoan  flashLoan;
    MockFlashToken token;
    GoodReceiver  good;
    BadReceiver   bad;

    address admin = address(this);

    function setUp() public {
        token    = new MockFlashToken();
        flashLoan = new ZbxFlashLoan(admin, 9);   // 9 bps = 0.09% fee
        good     = new GoodReceiver();
        bad      = new BadReceiver();

        // Fund flash pool
        token.mint(address(flashLoan), 1_000_000 ether);
    }

    // ── Basic flash loan ─────────────────────────────────────────────────

    function test_flash_loan_executes_and_repays() public {
        uint256 before = token.balanceOf(address(flashLoan));
        flashLoan.flashLoan(address(good), address(token), 10_000 ether, "");
        // Pool should have at least as much (fee captured)
        assertGe(token.balanceOf(address(flashLoan)), before);
    }

    function test_flash_loan_collects_fee() public {
        uint256 before = token.balanceOf(address(flashLoan));
        flashLoan.flashLoan(address(good), address(token), 100_000 ether, "");
        assertGt(token.balanceOf(address(flashLoan)), before);
    }

    function test_bad_receiver_reverts() public {
        vm.expectRevert();
        flashLoan.flashLoan(address(bad), address(token), 10_000 ether, "");
    }

    function test_exceed_pool_balance_reverts() public {
        vm.expectRevert();
        flashLoan.flashLoan(address(good), address(token), 2_000_000 ether, "");
    }

    function test_zero_amount_reverts() public {
        vm.expectRevert();
        flashLoan.flashLoan(address(good), address(token), 0, "");
    }

    // ── Fee configuration ─────────────────────────────────────────────────

    function test_set_fee_bps() public {
        flashLoan.setFeeBps(50); // 0.5%
        assertEq(flashLoan.feeBps(), 50);
    }

    function test_fee_too_high_reverts() public {
        vm.expectRevert();
        flashLoan.setFeeBps(10_001); // > 100%
    }

    function test_non_admin_cannot_set_fee() public {
        vm.prank(address(0xBAD));
        vm.expectRevert();
        flashLoan.setFeeBps(10);
    }
}

interface IZbxFlashReceiver {
    function executeOperation(address asset, uint256 amount, uint256 fee, bytes calldata params) external returns (bool);
}
