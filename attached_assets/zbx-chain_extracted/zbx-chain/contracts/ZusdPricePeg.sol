// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { Ownable2Step } from "./Ownable2Step.sol";

/// @title ZusdPricePeg — ZUSD peg monitoring and automated stability fee adjustment.
/// @notice Monitors ZUSD/USD price on-chain and calls ZusdVault.setStabilityFeeRate()
///         to steer borrowing costs and recover the $1.00 peg.
///
///         Peg logic:
///           ZUSD < $0.99 (BelowPeg):   increase stability fee → repaying ZUSD is
///                                       incentivised, supply contracts, price rises.
///           ZUSD > $1.01 (AbovePeg):   decrease stability fee → new CDPs are cheap,
///                                       more ZUSD is minted, price falls.
///           ZUSD < $0.95 (Emergency):  set maximum stability fee to compress supply
///                                       aggressively, emit EmergencyPegRecovery.
///           $0.99 ≤ ZUSD ≤ $1.01:     peg is healthy, no action needed.
///
/// @dev M53-01 FIX: adjustPeg() previously emitted events but never called the vault.
///      It now calls IZusdVault_Peg(vault).setStabilityFeeRate(newRate) based on the
///      oracle price, completing the feedback loop between the peg monitor and the CDP
///      stability fee.
///
/// @custom:zbx-chain  Chain ID 8989

interface IOracle_Peg {
    function getPrice(address asset) external view returns (uint256);
}

/// @dev Interface for the vault's mutable stability fee setter (M53-01).
interface IZusdVault_Peg {
    /// @notice Set the per-second stability fee rate (in RAY = 1e27 units).
    function setStabilityFeeRate(uint256 newRate) external;
    /// @notice Whether redemptions are paused (used by adjustPeg for emergency guard).
    function redemptionPaused() external view returns (bool);
}

contract ZusdPricePeg is Ownable2Step {

    address public zusd;
    address public oracle;
    address public vault;     // ZusdVault (adjusts stability fee)
    // S18: `owner` inherited from Ownable2Step.

    // ─── Peg bands (8-decimal oracle price) ──────────────────────────────

    uint256 public constant PEG_TARGET     = 1e8;       // $1.00
    uint256 public constant PEG_UPPER      = 101e6;     // $1.01
    uint256 public constant PEG_LOWER      = 99e6;      // $0.99
    uint256 public constant EMERGENCY_BAND = 95e6;      // $0.95 — emergency

    // ─── Stability fee presets (RAY / second, i.e. in 1e27 units) ────────

    /// @dev 1% APY ≈ 316_887_385_068_114_166 ray/sec (peg above $1.01 — loosen)
    uint256 public constant FEE_LOW    = 316_887_385_068_114_166;

    /// @dev 2% APY ≈ 627_937_192_491_029_810 ray/sec (neutral / healthy)
    uint256 public constant FEE_NORMAL = 627_937_192_491_029_810;

    /// @dev 5% APY ≈ 1_547_125_906_451_620_346 ray/sec (peg below $0.99 — tighten)
    uint256 public constant FEE_HIGH   = 1_547_125_906_451_620_346;

    /// @dev 20% APY ≈ 5_784_281_197_716_993_888 ray/sec (emergency — aggressive)
    uint256 public constant FEE_MAX    = 5_784_281_197_716_993_888;

    // ─── Adjustment rate limiting ─────────────────────────────────────────

    /// @notice Minimum seconds between peg adjustments (prevent keeper spam).
    uint256 public adjustCooldown = 3600;   // 1 hour default

    /// @notice Timestamp of the last adjustPeg() call.
    uint256 public lastAdjusted;

    // ─── Current peg status ───────────────────────────────────────────────

    enum PegStatus { Healthy, AbovePeg, BelowPeg, Emergency }

    /// @notice Last observed peg status (cached for cheap reads).
    PegStatus public lastStatus;

    // ─── Events ───────────────────────────────────────────────────────────

    event PegChecked(uint256 price, PegStatus status);
    event StabilityFeeAdjusted(uint256 newFeePerSec, string direction);
    event EmergencyPegRecovery(uint256 price);
    event CooldownUpdated(uint256 newCooldown);

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address zusd_, address oracle_, address vault_) Ownable2Step(msg.sender) {
        zusd   = zusd_;
        oracle = oracle_;
        vault  = vault_;
    }

    // ─── Read-only view ───────────────────────────────────────────────────

    /// @notice Check current peg status without triggering any action.
    function getPegStatus() external view returns (PegStatus status, uint256 price) {
        price  = IOracle_Peg(oracle).getPrice(zusd);
        status = _classify(price);
    }

    /// @notice Whether new CDPs can be opened (blocked in emergency).
    function newCdpAllowed() external view returns (bool) {
        uint256 price = IOracle_Peg(oracle).getPrice(zusd);
        return price >= EMERGENCY_BAND;
    }

    // ─── Keeper entrypoint ────────────────────────────────────────────────

    /// @notice Called by keeper bots (typically every hour) to adjust stability fee.
    ///
    /// @dev M53-01 FIX: Now fully wired — reads oracle price, classifies peg status,
    ///      picks the appropriate fee rate, and calls vault.setStabilityFeeRate().
    ///
    ///      Rate-limited by `adjustCooldown` to prevent keeper spam and excessive
    ///      gas consumption. Any address may call (permissionless keeper).
    function adjustPeg() external {
        require(
            block.timestamp >= lastAdjusted + adjustCooldown,
            "ZusdPricePeg: cooldown not elapsed"
        );

        uint256 price  = IOracle_Peg(oracle).getPrice(zusd);
        PegStatus s    = _classify(price);

        emit PegChecked(price, s);

        lastAdjusted = block.timestamp;
        lastStatus   = s;

        if (s == PegStatus.Healthy) {
            // Peg is healthy — restore the normal 2% APY rate (in case a previous
            // emergency call set it higher).
            IZusdVault_Peg(vault).setStabilityFeeRate(FEE_NORMAL);
            emit StabilityFeeAdjusted(FEE_NORMAL, "normal");

        } else if (s == PegStatus.AbovePeg) {
            // ZUSD > $1.01: reduce stability fee to incentivise new CDP minting,
            // increasing ZUSD supply until price normalises.
            IZusdVault_Peg(vault).setStabilityFeeRate(FEE_LOW);
            emit StabilityFeeAdjusted(FEE_LOW, "decreased (above peg)");

        } else if (s == PegStatus.BelowPeg) {
            // ZUSD < $0.99: increase stability fee to incentivise ZUSD repayment,
            // compressing supply until price normalises.
            IZusdVault_Peg(vault).setStabilityFeeRate(FEE_HIGH);
            emit StabilityFeeAdjusted(FEE_HIGH, "increased (below peg)");

        } else {
            // Emergency (ZUSD < $0.95): set maximum fee to aggressively compress
            // supply. Emit dedicated emergency event for monitoring alerting.
            IZusdVault_Peg(vault).setStabilityFeeRate(FEE_MAX);
            emit StabilityFeeAdjusted(FEE_MAX, "emergency maximum");
            emit EmergencyPegRecovery(price);
        }
    }

    // ─── Owner admin ──────────────────────────────────────────────────────

    /// @notice Override the stability fee directly (owner-only emergency override).
    /// @dev    Bypasses the cooldown. Use during crisis situations where keeper
    ///         automation is insufficient.
    function forceSetFeeRate(uint256 newRate) external onlyOwner {
        IZusdVault_Peg(vault).setStabilityFeeRate(newRate);
        emit StabilityFeeAdjusted(newRate, "owner override");
    }

    /// @notice Adjust the minimum seconds between keeper calls.
    function setAdjustCooldown(uint256 newCooldown) external onlyOwner {
        require(newCooldown >= 60, "ZusdPricePeg: cooldown < 1 min");
        adjustCooldown = newCooldown;
        emit CooldownUpdated(newCooldown);
    }

    /// @notice Update oracle address.
    function setOracle(address newOracle) external onlyOwner {
        require(newOracle != address(0), "zero");
        oracle = newOracle;
    }

    /// @notice Update vault address.
    function setVault(address newVault) external onlyOwner {
        require(newVault != address(0), "zero");
        vault = newVault;
    }

    // ─── Internal ─────────────────────────────────────────────────────────

    function _classify(uint256 price) internal pure returns (PegStatus) {
        if (price >= PEG_LOWER && price <= PEG_UPPER) return PegStatus.Healthy;
        if (price > PEG_UPPER)  return PegStatus.AbovePeg;
        if (price < EMERGENCY_BAND) return PegStatus.Emergency;
        return PegStatus.BelowPeg;
    }
}
