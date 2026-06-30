// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

// Foundry test for ZbxEntryPoint (ERC-4337).
// Run: forge test --match-contract ZbxEntryPointTest -vvv

import "../ZbxEntryPoint.sol";
import "../ZbxSmartWallet.sol";

contract ZbxEntryPointTest {
    ZbxEntryPoint entryPoint;
    address constant BENEFICIARY = address(0xBEEF);

    function setUp() public {
        entryPoint = new ZbxEntryPoint();
    }

    function testDeposit() public {
        entryPoint.depositTo{value: 1 ether}(address(this));
        require(entryPoint.balanceOf(address(this)) == 1 ether, "deposit failed");
    }

    function testWithdraw() public {
        entryPoint.depositTo{value: 2 ether}(address(this));
        entryPoint.withdrawTo(payable(address(this)), 1 ether);
        require(entryPoint.balanceOf(address(this)) == 1 ether, "withdraw failed");
    }

    function testGetUserOpHash_deterministic() public {
        ZbxEntryPoint.UserOperation memory op = _dummyOp();
        bytes32 hash1 = entryPoint.getUserOpHash(op);
        bytes32 hash2 = entryPoint.getUserOpHash(op);
        require(hash1 == hash2, "hash must be deterministic");
    }

    function testGetUserOpHash_differs_on_nonce() public {
        ZbxEntryPoint.UserOperation memory op1 = _dummyOp();
        ZbxEntryPoint.UserOperation memory op2 = _dummyOp();
        op2.nonce = 1;
        require(entryPoint.getUserOpHash(op1) != entryPoint.getUserOpHash(op2),
                "hash must differ with different nonce");
    }

    function _dummyOp() internal view returns (ZbxEntryPoint.UserOperation memory) {
        return ZbxEntryPoint.UserOperation({
            sender: address(this),
            nonce: 0,
            initCode: "",
            callData: "",
            callGasLimit: 200_000,
            verificationGasLimit: 100_000,
            preVerificationGas: 21_000,
            maxFeePerGas: 1e9,
            maxPriorityFeePerGas: 1e8,
            paymasterAndData: "",
            signature: ""
        });
    }

    receive() external payable {}
}