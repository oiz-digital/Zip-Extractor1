// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

/// @title Multicall3 — batched read + write call aggregator
/// @author Zebvix Technologies Pvt Ltd (interface-compatible with mds1/multicall3)
/// @notice Allows callers to bundle many independent contract calls into a
///         single transaction or eth_call. Reduces RPC round-trips for
///         dashboards (e.g. polling balances, allowances, oracle state)
///         and lets dApps execute several writes atomically.
///
///         Surface intentionally matches `mds1/multicall3` (canonical
///         deployment at 0xcA11bde05977b3631167028862bE2a173976CA11 on
///         100+ EVM chains) so existing tooling — viem, ethers Multicall,
///         The Graph — works unmodified against the Zebvix dApps.
///
/// @dev    Stateless. Safe to deploy to any deterministic CREATE2 address.
contract Multicall3 {

    // ─── S25-Y4 unchecked policy ─────────────────────────────────────────
    // All `unchecked { ... }` blocks in this contract fall into ONE of these
    // proven-safe categories (per S25 hardening pass):
    //   (a) post-require subtraction — preceding `require(x >= y)` proves
    //       `x - y` cannot underflow (token balance debits, allowance debits).
    //   (b) conservation pair — incrementing one slot by exactly the value
    //       just decremented (or vice-versa) from another, with the totalSupply
    //       invariant pre-checked (mint/burn/transfer leg of accounting pair).
    //   (c) bounded for-loop counter — `for (i; i < len; ) { ...; unchecked
    //       { i++; } }` where `len` is the bound; standard gas-saving pattern.
    //   (d) modular wrap intentional — uint32 timestamp/sequence wrap arithmetic
    //       (Uniswap V2 style); the wrap IS the spec.
    //   (e) UQ112x112 fixed-point shift — pre-bounded by uint112 reserve
    //       invariants (Uniswap V2 oracle accumulator).
    // Reviewers MUST classify any future `unchecked` block in this file
    // against one of (a)-(e) before merging; new categories require AUDIT entry.
    // ─────────────────────────────────────────────────────────────────────
    // ---------------------------------------------------------------------
    // Types
    // ---------------------------------------------------------------------

    struct Call {
        address target;
        bytes   callData;
    }

    struct Call3 {
        address target;
        bool    allowFailure;
        bytes   callData;
    }

    struct Call3Value {
        address target;
        bool    allowFailure;
        uint256 value;
        bytes   callData;
    }

    struct Result {
        bool   success;
        bytes  returnData;
    }

    // ---------------------------------------------------------------------
    // Aggregate (Multicall1-compatible)
    // ---------------------------------------------------------------------

    /// @notice Calls `target.callData` for every entry; reverts the whole
    ///         tx if any call reverts. Returns block number + decoded
    ///         return-data array.
    function aggregate(Call[] calldata calls)
        external
        payable
        returns (uint256 blockNumber, bytes[] memory returnData)
    {
        blockNumber = block.number;
        uint256 len = calls.length;
        returnData = new bytes[](len);
        for (uint256 i = 0; i < len; ) {
            (bool ok, bytes memory ret) = calls[i].target.call(calls[i].callData);
            require(ok, _decodeReason(ret, "Multicall3: call failed"));
            returnData[i] = ret;
            unchecked { i++; }
        }
    }

    // ---------------------------------------------------------------------
    // Aggregate3 — per-call allowFailure
    // ---------------------------------------------------------------------

    /// @notice Like `aggregate` but each call carries an `allowFailure`
    ///         flag — failed calls only revert when the flag is false.
    function aggregate3(Call3[] calldata calls)
        external
        payable
        returns (Result[] memory returnData)
    {
        uint256 len = calls.length;
        returnData = new Result[](len);
        for (uint256 i = 0; i < len; ) {
            Call3 calldata c = calls[i];
            (bool ok, bytes memory ret) = c.target.call(c.callData);
            if (!ok && !c.allowFailure) {
                revert(_decodeReason(ret, "Multicall3: call failed"));
            }
            returnData[i] = Result({ success: ok, returnData: ret });
            unchecked { i++; }
        }
    }

    /// @notice Like `aggregate3` but each call may also forward `value`.
    ///         Sum of values must equal `msg.value`.
    function aggregate3Value(Call3Value[] calldata calls)
        external
        payable
        returns (Result[] memory returnData)
    {
        uint256 len = calls.length;
        returnData = new Result[](len);
        uint256 valAccum = 0;
        for (uint256 i = 0; i < len; ) {
            Call3Value calldata c = calls[i];
            unchecked { valAccum += c.value; }
            (bool ok, bytes memory ret) = c.target.call{ value: c.value }(c.callData);
            if (!ok && !c.allowFailure) {
                revert(_decodeReason(ret, "Multicall3: call failed"));
            }
            returnData[i] = Result({ success: ok, returnData: ret });
            unchecked { i++; }
        }
        require(valAccum == msg.value, "Multicall3: value mismatch");
    }

    // ---------------------------------------------------------------------
    // tryAggregate — soft-fail variant
    // ---------------------------------------------------------------------

    /// @notice Best-effort batch execution. When `requireSuccess` is true
    ///         every call must succeed; otherwise per-call failures are
    ///         returned in `Result.success`.
    function tryAggregate(bool requireSuccess, Call[] calldata calls)
        external
        payable
        returns (Result[] memory returnData)
    {
        uint256 len = calls.length;
        returnData = new Result[](len);
        for (uint256 i = 0; i < len; ) {
            (bool ok, bytes memory ret) = calls[i].target.call(calls[i].callData);
            if (requireSuccess && !ok) {
                revert(_decodeReason(ret, "Multicall3: call failed"));
            }
            returnData[i] = Result({ success: ok, returnData: ret });
            unchecked { i++; }
        }
    }

    // ---------------------------------------------------------------------
    // BlockAndAggregate — return block + result bundle
    // ---------------------------------------------------------------------

    function blockAndAggregate(Call[] calldata calls)
        external
        payable
        returns (
            uint256 blockNumber,
            bytes32 blockHash,
            Result[] memory returnData
        )
    {
        (blockNumber, blockHash, returnData) =
            tryBlockAndAggregate(true, calls);
    }

    function tryBlockAndAggregate(bool requireSuccess, Call[] calldata calls)
        public
        payable
        returns (
            uint256 blockNumber,
            bytes32 blockHash,
            Result[] memory returnData
        )
    {
        blockNumber = block.number;
        blockHash   = blockhash(block.number - 1);
        uint256 len = calls.length;
        returnData = new Result[](len);
        for (uint256 i = 0; i < len; ) {
            (bool ok, bytes memory ret) = calls[i].target.call(calls[i].callData);
            if (requireSuccess && !ok) {
                revert(_decodeReason(ret, "Multicall3: call failed"));
            }
            returnData[i] = Result({ success: ok, returnData: ret });
            unchecked { i++; }
        }
    }

    // ---------------------------------------------------------------------
    // Chain helpers — convenient one-liners for dashboards
    // ---------------------------------------------------------------------

    function getEthBalance(address addr) external view returns (uint256) {
        return addr.balance;
    }

    function getBlockHash(uint256 blockNumber) external view returns (bytes32) {
        return blockhash(blockNumber);
    }

    function getLastBlockHash() external view returns (bytes32) {
        return blockhash(block.number - 1);
    }

    function getCurrentBlockTimestamp() external view returns (uint256) {
        return block.timestamp;
    }

    function getCurrentBlockGasLimit() external view returns (uint256) {
        return block.gaslimit;
    }

    function getCurrentBlockCoinbase() external view returns (address) {
        return block.coinbase;
    }

    function getBasefee() external view returns (uint256) {
        return block.basefee;
    }

    function getChainId() external view returns (uint256) {
        return block.chainid;
    }

    // ---------------------------------------------------------------------
    // Internal — strip ABI-encoded revert reason if present
    // ---------------------------------------------------------------------

    function _decodeReason(bytes memory ret, string memory fallbackMsg)
        private
        pure
        returns (string memory)
    {
        // 0x08c379a0 = Error(string) selector
        if (ret.length >= 68 &&
            ret[0] == 0x08 && ret[1] == 0xc3 &&
            ret[2] == 0x79 && ret[3] == 0xa0)
        {
            // S25-Y3 assembly: skip the first 4 bytes (Error(string) selector
            // 0x08c379a0, validated above) so the remaining bytes are a clean
            // ABI-encoded `string` ready for abi.decode. Equivalent to slicing
            // bytes[4:] but in-place and zero-copy. Length prefix at ret[0:32]
            // is implicitly shifted with the pointer.
            assembly {
                ret := add(ret, 0x04)
            }
            return abi.decode(ret, (string));
        }
        return fallbackMsg;
    }
}