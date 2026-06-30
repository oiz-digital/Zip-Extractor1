// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxContractFactory.sol";

contract ZbxContractFactoryTest is Test {
    ZbxContractFactory factory;

    address owner = address(this);
    address alice = address(0xA11CE);
    address bob   = address(0xB0B);

    function setUp() public {
        factory = new ZbxContractFactory(owner);
    }

    // ── ERC-20 deployment ─────────────────────────────────────────────────

    function test_deploy_erc20() public {
        vm.prank(alice);
        address token = factory.deployERC20(
            "My Token", "MTK", 18, 1_000_000 ether
        );
        assertTrue(token != address(0));
    }

    function test_deploy_erc20_correct_supply() public {
        vm.prank(alice);
        address token = factory.deployERC20("T", "T", 18, 500 ether);
        assertEq(IERC20Min(token).balanceOf(alice), 500 ether);
    }

    function test_deploy_erc20_empty_name_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        factory.deployERC20("", "T", 18, 100 ether);
    }

    function test_deploy_erc20_empty_symbol_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        factory.deployERC20("Token", "", 18, 100 ether);
    }

    // ── ERC-721 deployment ────────────────────────────────────────────────

    function test_deploy_erc721() public {
        vm.prank(alice);
        address nft = factory.deployERC721(
            "My NFT", "MNFT", "ipfs://base/", 10_000
        );
        assertTrue(nft != address(0));
    }

    function test_deploy_erc721_records_deployer() public {
        vm.prank(alice);
        address nft = factory.deployERC721("NFT", "NFT", "", 100);
        assertEq(factory.deployerOf(nft), alice);
    }

    // ── Tracking ──────────────────────────────────────────────────────────

    function test_deployed_contracts_tracked() public {
        vm.prank(alice);
        factory.deployERC20("T", "T", 18, 100 ether);
        vm.prank(alice);
        factory.deployERC20("T2", "T2", 18, 200 ether);
        assertEq(factory.deployedByUser(alice).length, 2);
    }

    function test_total_deployed_count() public {
        vm.prank(alice);
        factory.deployERC20("T1", "T1", 18, 100 ether);
        vm.prank(bob);
        factory.deployERC20("T2", "T2", 18, 100 ether);
        assertEq(factory.totalDeployed(), 2);
    }

    // ── Fee collection ────────────────────────────────────────────────────

    function test_deploy_with_fee() public {
        factory.setDeployFee(0.01 ether);
        vm.deal(alice, 1 ether);
        vm.prank(alice);
        factory.deployERC20{value: 0.01 ether}("T", "T", 18, 100 ether);
        assertGt(address(factory).balance, 0);
    }

    function test_deploy_insufficient_fee_reverts() public {
        factory.setDeployFee(0.01 ether);
        vm.deal(alice, 1 ether);
        vm.prank(alice);
        vm.expectRevert();
        factory.deployERC20{value: 0.001 ether}("T", "T", 18, 100 ether);
    }

    function test_owner_withdraw_fees() public {
        factory.setDeployFee(0.01 ether);
        vm.deal(alice, 1 ether);
        vm.prank(alice);
        factory.deployERC20{value: 0.01 ether}("T", "T", 18, 100 ether);
        uint256 before = owner.balance;
        factory.withdrawFees(payable(owner));
        assertGt(owner.balance, before);
    }
}

interface IERC20Min {
    function balanceOf(address) external view returns (uint256);
}
