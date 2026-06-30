// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

/**
 * @title ZvmOpcodes
 * @notice Solidity wrappers for ZVM-native opcodes.
 *
 * ZVM-native contracts can call these functions to use
 * ZBX chain features not available in standard EVM.
 *
 * Compile with ZVM toolchain (zbx-solc) to use these.
 * Standard solc will compile them as INVALID opcodes (safe fallback).
 */
library ZvmOpcodes {

    // ─── PAYID (0xC0) ────────────────────────────────────────────────────

    /**
     * @notice Resolve a Pay ID to a wallet address.
     * @param payId The Pay ID string, e.g. "ali@zbx"
     * @return addr The resolved wallet address (address(0) if not found)
     *
     * Example:
     *   address wallet = ZvmOpcodes.resolvePayId("ali@zbx");
     *   require(wallet != address(0), "Pay ID not found");
     *   payable(wallet).transfer(amount);
     */
    function resolvePayId(string memory payId) internal view returns (address addr) {
        bytes memory b = bytes(payId);
        assembly {
            // Push pointer and length onto stack, then PAYID opcode (0xC0)
            let ptr := add(b, 32)
            let len := mload(b)
            // ZVM: PAYID opcode resolves pay_id string from memory
            addr := staticcall(gas(), 0x0A, ptr, len, 0, 32)
            addr := mload(0)
        }
    }

    // ─── ZUSDBAL (0xC1) ──────────────────────────────────────────────────

    /**
     * @notice Get ZUSD balance of an address without calling the ZUSD contract.
     * @param account Address to query.
     * @return balance ZUSD balance in 18-decimal wei.
     *
     * Example:
     *   uint256 balance = ZvmOpcodes.zusdBalance(msg.sender);
     *   require(balance >= 100e18, "Need at least 100 ZUSD");
     */
    function zusdBalance(address account) internal view returns (uint256 balance) {
        assembly {
            // ZVM ZUSD_BALANCE precompile at 0x0F
            mstore(0, account)
            staticcall(gas(), 0x0F, 0, 32, 0, 32)
            balance := mload(0)
        }
    }

    // ─── ZBXPRICE (0xC2) ─────────────────────────────────────────────────

    /**
     * @notice Get current ZBX/USD price from the ZVM oracle.
     * @return price ZBX price in USD (18 decimals). e.g. 2500e18 = $2500.
     *
     * Example:
     *   uint256 price = ZvmOpcodes.zbxPrice();
     *   uint256 usdValue = (zbxAmount * price) / 1e18;
     */
    function zbxPrice() internal view returns (uint256 price) {
        assembly {
            staticcall(gas(), 0x0C, 0, 0, 0, 32)
            price := mload(0)
        }
    }

    // ─── ZBXTIME (0xC3) ──────────────────────────────────────────────────

    /**
     * @notice Get ZBX block time in milliseconds (always 5000ms = 5 seconds).
     * @return ms Block time in milliseconds.
     */
    function zbxBlockTime() internal pure returns (uint256 ms) {
        assembly {
            ms := 5000
        }
    }

    // ─── AASENDER (0xC4) ─────────────────────────────────────────────────

    /**
     * @notice Get the original UserOperation sender (ERC-4337 AA).
     * @return sender The original sender of the UserOperation.
     *
     * In non-AA calls, returns msg.sender (same as CALLER).
     * In AA calls, returns the smart wallet owner who signed the UserOperation.
     *
     * Example:
     *   address realSender = ZvmOpcodes.aaSender();
     *   // realSender is the human, msg.sender may be the EntryPoint
     */
    function aaSender() internal view returns (address sender) {
        assembly {
            // If called via AA bundler: returns UserOp.sender
            // Otherwise: returns msg.sender
            sender := caller()  // ZVM replaces this with AASENDER opcode
        }
    }

    // ─── ZBXBURN (0xC8) ──────────────────────────────────────────────────

    /**
     * @notice Burn ZBX from the caller's balance (deflationary mechanism).
     * @param amount Amount of ZBX to burn (in wei).
     *
     * Example (fee burn in a contract):
     *   uint256 fee = calculateFee(msg.value);
     *   ZvmOpcodes.burnZbx(fee);  // Burns fee from caller
     */
    function burnZbx(uint256 amount) internal {
        assembly {
            // ZVM ZBXBURN opcode: burns `amount` ZBX from caller
            pop(amount) // ZVM replaces with: ZBXBURN
        }
    }
}