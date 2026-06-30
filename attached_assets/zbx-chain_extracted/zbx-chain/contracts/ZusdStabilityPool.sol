// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { ReentrancyGuard } from "./libraries/ReentrancyGuard.sol";
import { Ownable2Step }    from "./Ownable2Step.sol";

/// @title ZusdStabilityPool — ZUSD liquidation buffer (Liquity-style).
/// @notice ZUSD holders deposit here to earn ZBX collateral from liquidations.
///
/// @dev   How it works:
///           1. Users deposit ZUSD into the stability pool.
///           2. When a CDP is liquidated, the pool absorbs the debt:
///              - Pool burns ZUSD equal to the debt
///              - Pool receives the ZBX collateral (+ 10% bonus)
///           3. Depositors earn:
///              - ZBX gains (from absorbed collateral)
///              - ZBX rewards (emission from the protocol)
///
///        Example:
///           Pool has 1,000,000 ZUSD deposited.
///           CDP: 100,000 ZUSD debt, 150 ZBX collateral @ $700 = $105K
///           Liquidation:
///             - Pool burns 100,000 ZUSD
///             - Pool receives 150 ZBX ($105K / $700 = 150 ZBX + 10% bonus)
///           Depositors split 150 ZBX pro-rata.
///
/// @custom:zbx-chain  Chain ID 8989

interface IZUSD_SP {
    function balanceOf(address) external view returns (uint256);
    function transferFrom(address, address, uint256) external returns (bool);
    function transfer(address, uint256) external returns (bool);
    function burn(address, uint256) external;
}

interface IZBX_SP {
    function transfer(address, uint256) external returns (bool);
    function balanceOf(address) external view returns (uint256);
}

contract ZusdStabilityPool is ReentrancyGuard, Ownable2Step {

    // ─── State ────────────────────────────────────────────────────────────

    address public zusd;
    address public zbx;
    address public vault;    // ZusdVault — only it can trigger liquidation absorption
    // S18: `owner` inherited from Ownable2Step.

    uint256 public totalZusdDeposits;
    uint256 public totalZbxGains;

    /// Depositor state.
    mapping(address => uint256) public deposits;        // ZUSD deposited
    mapping(address => uint256) public zbxGain;         // ZBX earned from liquidations
    mapping(address => uint256) public depositSnapshot; // for proportional gain tracking

    /// Global gain tracking (P: product, S: sum of ZBX gains per unit ZUSD).
    uint256 public P = 1e18;  // product factor (starts at 1)
    uint256 public S;         // cumulative ZBX gain sum (per unit ZUSD)

    mapping(address => uint256) public snapshots_P;
    mapping(address => uint256) public snapshots_S;

    // ─── Events ───────────────────────────────────────────────────────────

    event ZusdDeposited(address indexed user, uint256 amount);
    event ZusdWithdrawn(address indexed user, uint256 amount);
    event ZbxGainClaimed(address indexed user, uint256 zbxAmount);
    event LiquidationAbsorbed(uint256 zusdAbsorbed, uint256 zbxReceived);

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address zusd_, address zbx_) Ownable2Step(msg.sender) {
        zusd  = zusd_;
        zbx   = zbx_;
    }

    // S18: replaced inline `require(msg.sender == owner, ...)` with the
    //      inherited `onlyOwner` modifier from Ownable2Step.
    function setVault(address vault_) external onlyOwner {
        vault = vault_;
    }

    // ─── Deposit ──────────────────────────────────────────────────────────

    /// @notice Deposit ZUSD into the stability pool.
    ///         Earns ZBX when liquidations occur.
    function deposit(uint256 amount) external nonReentrant {
        require(amount > 0, "SP: zero amount");

        // Claim any pending ZBX gains first.
        _claimGains(msg.sender);

        require(IZUSD_SP(zusd).transferFrom(msg.sender, address(this), amount),
                "SP: ZUSD transfer failed");

        deposits[msg.sender]        += amount;
        totalZusdDeposits           += amount;

        // Snapshot current P and S.
        snapshots_P[msg.sender] = P;
        snapshots_S[msg.sender] = S;

        emit ZusdDeposited(msg.sender, amount);
    }

    // ─── Withdraw ─────────────────────────────────────────────────────────

    /// @notice Withdraw ZUSD from the stability pool.
    ///         Your ZUSD may have been partially consumed by liquidations.
    function withdraw(uint256 amount) external nonReentrant {
        _claimGains(msg.sender);

        uint256 available = _compoundedDeposit(msg.sender);
        uint256 toWithdraw = amount > available ? available : amount;
        require(toWithdraw > 0, "SP: nothing to withdraw");

        deposits[msg.sender]  = available - toWithdraw;
        totalZusdDeposits    -= toWithdraw;

        // Burn withdrawn ZUSD from pool and send back.
        IZUSD_SP(zusd).transfer(msg.sender, toWithdraw);

        emit ZusdWithdrawn(msg.sender, toWithdraw);
    }

    // ─── Claim ZBX gains ──────────────────────────────────────────────────

    /// @notice Claim accumulated ZBX gains from liquidations.
    function claimZbxGain() external nonReentrant {
        _claimGains(msg.sender);
    }

    // ─── Liquidation absorption (called by ZusdVault) ─────────────────────

    /// @notice Absorb a liquidation: burn ZUSD, receive ZBX collateral.
    function absorbLiquidation(uint256 zusdDebt, uint256 zbxCollateral) external nonReentrant {
        require(msg.sender == vault, "SP: not vault");
        require(totalZusdDeposits >= zusdDebt, "SP: insufficient deposits");

        // Update cumulative ZBX gain per unit ZUSD.
        uint256 zbxPerZusd = zbxCollateral * 1e18 / totalZusdDeposits;
        S += zbxPerZusd;

        // Update product factor P (deposits shrink proportionally).
        uint256 lossPerZusd = zusdDebt * 1e18 / totalZusdDeposits;
        P = P * (1e18 - lossPerZusd) / 1e18;
        if (P == 0) P = 1; // avoid full zeroing

        totalZusdDeposits -= zusdDebt;
        totalZbxGains     += zbxCollateral;

        emit LiquidationAbsorbed(zusdDebt, zbxCollateral);
    }

    // ─── Views ────────────────────────────────────────────────────────────

    /// @notice Get your compounded ZUSD deposit (may be less than original).
    function getDeposit(address user) external view returns (uint256) {
        return _compoundedDeposit(user);
    }

    /// @notice Get your pending ZBX gain from liquidations.
    function getPendingZbxGain(address user) external view returns (uint256) {
        return _pendingZbxGain(user);
    }

    function totalDeposits() external view returns (uint256) {
        return totalZusdDeposits;
    }

    // ─── Internal ─────────────────────────────────────────────────────────

    function _claimGains(address user) internal {
        uint256 gain = _pendingZbxGain(user);
        if (gain > 0) {
            zbxGain[user] = 0;
            require(IZBX_SP(zbx).transfer(user, gain), "SP: ZBX transfer failed");
            emit ZbxGainClaimed(user, gain);
        }
        snapshots_P[user] = P;
        snapshots_S[user] = S;
    }

    function _compoundedDeposit(address user) internal view returns (uint256) {
        if (deposits[user] == 0) return 0;
        if (snapshots_P[user] == 0) return deposits[user];
        return deposits[user] * P / snapshots_P[user];
    }

    function _pendingZbxGain(address user) internal view returns (uint256) {
        if (deposits[user] == 0) return 0;
        uint256 gainPerUnit = S - snapshots_S[user];
        return deposits[user] * gainPerUnit / 1e18 + zbxGain[user];
    }
}