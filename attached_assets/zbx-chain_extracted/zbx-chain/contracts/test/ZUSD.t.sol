// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

// Foundry tests for ZUSD stablecoin + ZusdVault.
// Run: forge test --match-contract ZUSDTest -vvv

import "../ZUSD.sol";

contract ZUSDTest {
    ZUSD zusd;

    function setUp() public {
        zusd = new ZUSD();
        zusd.setVault(address(this)); // test contract acts as vault
    }

    function testMintAndBurn() public {
        zusd.mint(address(this), 1000e18);
        require(zusd.totalSupply()          == 1000e18, "total supply");
        require(zusd.balanceOf(address(this)) == 1000e18, "balance");

        zusd.burn(address(this), 400e18);
        require(zusd.totalSupply()          == 600e18, "after burn supply");
        require(zusd.balanceOf(address(this)) == 600e18, "after burn balance");
    }

    function testTransfer() public {
        zusd.mint(address(this), 500e18);
        zusd.transfer(address(0xBEEF), 200e18);
        require(zusd.balanceOf(address(0xBEEF)) == 200e18, "recipient");
        require(zusd.balanceOf(address(this))   == 300e18, "sender");
    }

    function testOnlyVaultCanMint() public {
        bool reverted;
        // address(1) is not the vault
        (bool ok, ) = address(zusd).call(
            abi.encodeWithSignature("mint(address,uint256)", address(this), 1e18)
        );
        // Called from test contract which IS the vault — should succeed
        require(ok, "vault can mint");

        // Change vault to someone else
        zusd.setVault(address(0x9999));
        (ok, ) = address(zusd).call(
            abi.encodeWithSignature("mint(address,uint256)", address(this), 1e18)
        );
        require(!ok, "non-vault cannot mint");
    }

    function testDecimals() public {
        require(zusd.decimals() == 18, "18 decimals");
    }

    function testSymbol() public {
        require(
            keccak256(abi.encodePacked(zusd.symbol())) ==
            keccak256(abi.encodePacked("ZUSD")),
            "symbol is ZUSD"
        );
    }

    function testApproveAndTransferFrom() public {
        zusd.mint(address(this), 1000e18);
        zusd.approve(address(0xCAFE), 500e18);
        require(zusd.allowance(address(this), address(0xCAFE)) == 500e18, "allowance set");
    }
}