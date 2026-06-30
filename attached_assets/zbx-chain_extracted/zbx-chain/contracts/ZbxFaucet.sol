// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title ZbxFaucet — Testnet ZBX token faucet.
/// @notice Dispenses 100 ZBX per address per 24 hours for testnet development.
///         Funded by the ZBX testnet foundation allocation.
///
/// @custom:zbx-chain  Chain ID 8990 (testnet only)

import "./libraries/ReentrancyGuard.sol";

contract ZbxFaucet is ReentrancyGuard {

    uint256 public constant DRIP_AMOUNT  = 100 ether;    // 100 ZBX per request
    uint256 public constant COOLDOWN     = 24 hours;

    address public owner;
    mapping(address => uint256) public lastRequest;
    uint256 public totalDispensed;
    uint256 public requestCount;
    bool    public paused;

    /// @dev S25-Y2: inline `_locked` guard migrated to shared
    ///      `ReentrancyGuard` library (see contracts/libraries/ReentrancyGuard.sol).
    ///      Same `_NOT_ENTERED=1` / `_ENTERED=2` semantics; `nonReentrant`
    ///      modifier now inherited so audit surface is one file instead of seven.
    ///      Faucet already follows checks-effects-interactions (state updated
    ///      before any .call); the inherited guard remains belt-and-suspenders
    ///      so any future helper added below the .call also cannot be re-entered.
    ///      See AUDIT_2026-04-30.md C-18 (original) and S25 close block (migration).

    event Dispensed(address indexed to, uint256 amount);
    event Funded(address indexed from, uint256 amount);
    event Paused(bool status);

    constructor() payable {
        owner = msg.sender;
    }

    modifier onlyOwner() { require(msg.sender == owner, "Faucet: not owner"); _; }
    modifier notPaused() { require(!paused, "Faucet: paused"); _; }

    // ─── Request tokens ───────────────────────────────────────────────────

    /// @notice Request 100 ZBX tokens. Callable once every 24 hours.
    function request() external notPaused nonReentrant {
        require(block.timestamp >= lastRequest[msg.sender] + COOLDOWN,
                "Faucet: cooldown active");
        require(address(this).balance >= DRIP_AMOUNT, "Faucet: insufficient funds");

        lastRequest[msg.sender] = block.timestamp;
        totalDispensed += DRIP_AMOUNT;
        requestCount++;

        (bool ok, ) = payable(msg.sender).call{value: DRIP_AMOUNT}("");
        require(ok, "Faucet: transfer failed");
        emit Dispensed(msg.sender, DRIP_AMOUNT);
    }

    /// @notice Check how many seconds until address can request again.
    function cooldownRemaining(address user) external view returns (uint256) {
        uint256 nextTime = lastRequest[user] + COOLDOWN;
        if (block.timestamp >= nextTime) return 0;
        return nextTime - block.timestamp;
    }

    // ─── Admin ────────────────────────────────────────────────────────────

    function setPaused(bool status) external onlyOwner {
        paused = status;
        emit Paused(status);
    }

    /// @dev S19: migrated off `.transfer(...)` to `.call{value:...}("")` so
    ///      the immutable `owner` (commonly a multi-sig) can drain the
    ///      faucet regardless of EIP-2929 cold-account gas costs.
    ///      Reuses the file's existing inline `nonReentrant` guard
    ///      (already used on `request()`); no new library import needed.
    function withdraw() external onlyOwner nonReentrant {
        uint256 bal = address(this).balance;
        (bool ok, ) = payable(owner).call{value: bal}("");
        require(ok, "ZbxFaucet: withdraw failed");
    }

    /// @notice Fund the faucet with ZBX.
    receive() external payable {
        emit Funded(msg.sender, msg.value);
    }
}