// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title ZbxFlashLoan — Base contract for flash loan receivers.
/// @notice Inherit from this to build flash loan arbitrage / liquidation bots.
///
/// @dev   Flash loan flow:
///          1. Call ZbxLendingPool.flashLoan(address(this), asset, amount, data)
///          2. Pool transfers `amount` to this contract
///          3. Pool calls executeOperation() — your logic runs here
///          4. You must approve Pool for (amount + fee) before returning
///          5. Pool pulls back (amount + fee) — if not approved, tx reverts
///
/// @custom:zbx-chain  Chain ID 8989

import { ReentrancyGuard } from "./libraries/ReentrancyGuard.sol";

interface IZbxLendingPool {
    function flashLoan(address receiver, address asset, uint256 amount, bytes calldata params) external;
}

interface IZRC20Approve {
    function approve(address spender, uint256 amount) external returns (bool);
    function balanceOf(address) external view returns (uint256);
}

/// @dev SEC-2026-05-09 hardening: `executeOperation` is now `nonReentrant`.
///      Flash loans are *the* canonical reentrancy attack vector — Aave V2
///      lost funds to a similar pattern in 2020. While `onlyPool` already
///      restricts the caller, a malicious pool *could* nest calls through
///      the user's overridden `_executeFlashLoan` and re-enter mid-execution
///      to manipulate balances or accounting state. The guard makes that
///      structurally impossible regardless of pool implementation.
abstract contract ZbxFlashLoanReceiver is ReentrancyGuard {

    address public immutable LENDING_POOL;

    constructor(address pool) {
        LENDING_POOL = pool;
    }

    modifier onlyPool() {
        require(msg.sender == LENDING_POOL, "FlashLoan: caller not pool");
        _;
    }

    /// @notice Called by ZbxLendingPool after transferring flash loan funds.
    ///         Override this function with your arbitrage/liquidation logic.
    /// @param asset   Token borrowed.
    /// @param amount  Amount borrowed.
    /// @param fee     Fee to repay (in addition to `amount`).
    /// @param params  Custom params passed to flashLoan().
    function executeOperation(
        address asset,
        uint256 amount,
        uint256 fee,
        address initiator,
        bytes calldata params
    ) external onlyPool nonReentrant returns (bool) {
        // Override in subclass with your logic.
        // MUST approve pool for (amount + fee) before returning true.
        _executeFlashLoan(asset, amount, fee, initiator, params);
        IZRC20Approve(asset).approve(LENDING_POOL, amount + fee);
        return true;
    }

    /// @dev Override this with your flash loan logic.
    function _executeFlashLoan(
        address asset,
        uint256 amount,
        uint256 fee,
        address initiator,
        bytes calldata params
    ) internal virtual;

    /// @notice Convenience: initiate a flash loan from this contract.
    function _flashLoan(address asset, uint256 amount, bytes memory params) internal {
        IZbxLendingPool(LENDING_POOL).flashLoan(address(this), asset, amount, params);
    }
}

/// @title Example: Arbitrage flash loan between two ZbxAMM pairs.
contract ZbxArbitrageFlashLoan is ZbxFlashLoanReceiver {
    address public owner;

    constructor(address pool) ZbxFlashLoanReceiver(pool) {
        owner = msg.sender;
    }

    function arbitrage(
        address asset,
        uint256 amount,
        address pairA,
        address pairB,
        bytes calldata swapData
    ) external {
        require(msg.sender == owner, "Arb: not owner");
        bytes memory params = abi.encode(pairA, pairB, swapData);
        _flashLoan(asset, amount, params);
    }

    function _executeFlashLoan(
        address asset,
        uint256 amount,
        uint256 fee,
        address,
        bytes calldata params
    ) internal override {
        (address pairA, address pairB, bytes memory swapData) =
            abi.decode(params, (address, address, bytes));

        // 1. Swap on pairA (buy cheap)
        (bool ok1, ) = pairA.call(swapData);
        require(ok1, "Arb: pairA swap failed");

        // 2. Swap on pairB (sell expensive)
        // In production: encode the reverse swap calldata
        (bool ok2, ) = pairB.call(swapData);
        require(ok2, "Arb: pairB swap failed");

        // Profit stays in this contract; pool gets (amount + fee).
        uint256 profit = IZRC20Approve(asset).balanceOf(address(this)) - (amount + fee);
        require(profit > 0, "Arb: no profit");
    }
}