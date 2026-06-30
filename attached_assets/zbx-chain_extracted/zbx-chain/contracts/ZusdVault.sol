// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { Ownable2Step } from "./Ownable2Step.sol";
import { ReentrancyGuard } from "./libraries/ReentrancyGuard.sol";

/// @title ZusdVault — Collateralised Debt Position (CDP) vault.
/// @notice Lock ZBX collateral → mint ZUSD (up to 50% of collateral value).
///         If ZBX price drops 50%, position is instantly liquidatable.
///
/// @dev   Inspired by MakerDAO (CDPs) and Liquity (liquidation efficiency).
///
///        Key parameters:
///          MIN_COLLATERAL_RATIO  200%  — mint max 50% of collateral value
///          LIQUIDATION_RATIO     100%  — 50% price drop = instant liquidation
///          LIQUIDATION_BONUS     10%   — extra collateral for liquidators
///          STABILITY_FEE         2% APY — charged on outstanding ZUSD debt
///          REDEMPTION_FEE        0.5%  — floor arbitrage mechanism
///
///        ZBX collateral flow:
///          User → lock ZBX → vault holds ZBX → vault mints ZUSD to user
///          User → repay ZUSD → vault burns ZUSD → vault unlocks ZBX to user
///
/// @custom:zbx-chain  Chain ID 8989

interface IZUSD_Mint {
    function mint(address to, uint256 amount) external;
    function burn(address from, uint256 amount) external;
    function balanceOf(address) external view returns (uint256);
}

interface IZBX_Transfer {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
    function balanceOf(address) external view returns (uint256);
}

interface IOracle_Price {
    /// SEC-2026-05-09 Pass-15 (HIGH-S09): the underlying ZbxOracle
    /// already enforces staleness inside its own getPrice — but only
    /// because we're depending on a specific implementation. Calling
    /// the AggregatorV3-shaped variant lets us defensively
    /// re-validate updatedAt at the vault layer too, so a future
    /// oracle swap with a non-revertting getPrice() can't silently
    /// expose CDPs to liquidations against minute-old prices on a
    /// stale feed.
    function getPrice(address asset) external view returns (uint256);
    function latestRoundData(address asset) external view returns (
        uint80 roundId, int256 answer,
        uint256 startedAt, uint256 updatedAt, uint80 answeredInRound
    );
}

/// SEC-2026-05-09 Pass-15 (HIGH-S09) — defensive staleness assertion
/// run at every CDP-touching call site (mintMore / liquidate / redeem
/// etc.). 1-hour matches the Oracle's MAX_STALENESS default.
library OracleFreshness {
    uint256 internal constant VAULT_MAX_STALENESS = 3600;
    function assertFresh(address oracle, address asset) internal view {
        try IOracle_Price(oracle).latestRoundData(asset) returns (
            uint80, int256 answer, uint256, uint256 updatedAt, uint80
        ) {
            require(answer > 0, "Vault: oracle non-positive");
            require(updatedAt > 0, "Vault: oracle has no price");
            require(
                block.timestamp - updatedAt <= VAULT_MAX_STALENESS,
                "Vault: oracle stale"
            );
        } catch {
            // Oracle without latestRoundData (legacy interface) — fall
            // back to trusting getPrice's internal check. This keeps
            // the vault working in dev/test against minimal oracles
            // while production deployments get the defensive check.
        }
    }
}

contract ZusdVault is Ownable2Step, ReentrancyGuard {

    /// SEC-2026-05-09 Pass-15 (HIGH-S09): centralised fresh-price
    /// helper — every CDP-touching site goes through this so a future
    /// oracle swap or interface drift only needs to be addressed in
    /// one place. Calls `OracleFreshness.assertFresh` first, which
    /// reverts on stale / missing / non-positive prices, then returns
    /// the canonical uint256 price.
    function _freshPrice(address asset) internal view returns (uint256) {
        OracleFreshness.assertFresh(oracle, asset);
        return IOracle_Price(oracle).getPrice(asset);
    }

    // ─── Constants ────────────────────────────────────────────────────────

    uint256 public constant MIN_COLLATERAL_RATIO  = 200_00;  // 200% in bps → max mint = 50% of collateral
    uint256 public constant LIQUIDATION_RATIO     = 100_00;  // 100% in bps → 50% price drop = liquidation
    uint256 public constant LIQUIDATION_BONUS     =  10_00;  // 10%  in bps
    uint256 public constant REDEMPTION_FEE_BPS    =     50;  // 0.5%
    /// @notice Per-second compounding rate for the 2 % APY stability fee, in
    ///         ray (1e27) units. Derived as (1.02)^(1/31_536_000) - 1 ≈
    ///         6.28e-10 per second, scaled by 1e27. Audit-2026-05-01 S6-V3:
    ///         the previous value (634_195_839) was ~9 orders of magnitude
    ///         too small (linear-units mistake), making the fee effectively
    ///         zero. Replaced with the canonical MakerDAO ray/sec constant.
    /// @dev M53-01 FIX: Changed from `constant` to a mutable storage variable so
    ///      ZusdPricePeg.adjustPeg() can tighten/loosen the fee based on peg status.
    ///      Default value is 2% APY in ray/sec (same as the previous constant).
    ///      Owner can call `setStabilityFeeRate(newRate)` to adjust.
    uint256 public stabilityFeePerSec = 627_937_192_491_029_810;  // 2% APY in ray/sec
    uint256 public constant RAY                   = 1e27;
    uint256 public constant BPS                   = 10_000;
    uint256 public constant MIN_ZUSD_MINT         = 100e18;  // min 100 ZUSD per CDP

    // ─── Reentrancy guard ─────────────────────────────────────────────────
    // OZ-style single-slot lock. Costs 5k gas per call (warm SSTORE) but
    // makes every CDP-mutating function safe against cross-contract
    // re-entry — important because ZBX is an ERC-20 that may itself call
    // back via hooks, and the oracle is upgradable.
    // SEC-2026-05-09: migrated to libraries/ReentrancyGuard.sol.

    // ─── CDP (Collateralised Debt Position) ───────────────────────────────

    struct CDP {
        uint256 collateral;      // ZBX locked (in wei)
        uint256 debt;            // ZUSD minted (in wei), before stability fee
        uint256 lastFeeIndex;    // stability fee accumulator snapshot
        uint256 openedAt;        // block timestamp
    }

    mapping(address => CDP) public cdps;

    // ─── State ────────────────────────────────────────────────────────────

    address public zusd;     // ZUSD token contract
    address public zbx;      // ZBX token (collateral)
    address public oracle;   // price oracle
    // S18: `owner` inherited from Ownable2Step.
    address public stabilityPool;  // ZusdStabilityPool

    uint256 public totalCollateral;  // total ZBX locked
    uint256 public totalDebt;        // total ZUSD minted
    uint256 public feeIndex;         // global stability fee accumulator (ray)
    uint256 public lastFeeUpdate;    // timestamp of last fee update

    // ─── Redemption state (S15-P2 — re-enabled per audit S6-V2-FIXED) ─────
    //
    // The S6-V2 stub-and-revert decision is replaced by a hint-based,
    // monotonicity-checked redemption that REQUIRES per-CDP record
    // updates (the original bug). See ZEP-003-ZUSD-REDEMPTION.md.
    bool    public redemptionPaused;        // emergency-stop switch (owner)
    address public feeRecipient;            // ZBX fee sink; defaults to owner if zero
    uint256 public totalRedeemed;           // lifetime ZUSD burned via redemption
    uint256 public totalRedemptionFees;     // lifetime ZBX collected as redemption fee

    /// @notice Minimum ZUSD per redemption call (anti-spam).
    uint256 public constant MIN_REDEEM_AMOUNT  = 10e18;
    /// @notice Maximum CDPs traversable per redemption call (gas bound).
    uint256 public constant MAX_REDEEM_ITER    = 50;

    // ─── Events ───────────────────────────────────────────────────────────

    event CDPOpened(address indexed owner, uint256 collateral, uint256 debt);
    event CollateralAdded(address indexed owner, uint256 amount);
    event ZusdMinted(address indexed owner, uint256 amount);
    event ZusdRepaid(address indexed owner, uint256 amount);
    event CollateralWithdrawn(address indexed owner, uint256 amount);
    event CDPLiquidated(
        address indexed owner, address indexed liquidator,
        uint256 debtRepaid, uint256 collateralSeized, uint256 bonus
    );
    event Redeemed(address indexed redeemer, uint256 zusdAmount, uint256 zbxReceived);
    event RedeemedFromCDP(address indexed cdpOwner, uint256 zusdRedeemed, uint256 zbxTaken);
    event CDPClosedByRedemption(address indexed cdpOwner, uint256 leftoverCollateralReturned);
    event RedemptionPausedToggled(address indexed by, bool paused);
    event FeeRecipientUpdated(address indexed newRecipient);
    /// @notice M53-01: emitted when the owner or ZusdPricePeg adjusts the fee rate.
    event StabilityFeeRateUpdated(uint256 indexed oldRate, uint256 newRate);

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address zusd_, address zbx_, address oracle_) Ownable2Step(msg.sender) {
        zusd       = zusd_;
        zbx        = zbx_;
        oracle     = oracle_;
        feeIndex   = RAY;       // starts at 1.0 (no fees accumulated)
        lastFeeUpdate = block.timestamp;
    }

    // ─── Open CDP ─────────────────────────────────────────────────────────

    /// @notice Lock ZBX and mint ZUSD in one transaction.
    /// @param collateralAmount  ZBX to lock (in wei)
    /// @param zusdAmount        ZUSD to mint (must be <= 66.6% of collateral value)
    function openCDP(uint256 collateralAmount, uint256 zusdAmount) external nonReentrant {
        require(cdps[msg.sender].collateral == 0, "Vault: CDP already open");
        require(collateralAmount > 0,             "Vault: zero collateral");
        require(zusdAmount >= MIN_ZUSD_MINT,      "Vault: below min mint");

        _accrueStabilityFee();

        // Check collateral ratio.
        uint256 zbxPrice     = _freshPrice(zbx);
        uint256 colValueUSD  = collateralAmount * zbxPrice / 1e18;
        uint256 requiredCR   = zusdAmount * MIN_COLLATERAL_RATIO / BPS;
        require(colValueUSD >= requiredCR, "Vault: collateral ratio too low");

        // Transfer ZBX from user to vault.
        require(IZBX_Transfer(zbx).transferFrom(msg.sender, address(this), collateralAmount),
                "Vault: ZBX transfer failed");

        // Record CDP.
        cdps[msg.sender] = CDP({
            collateral:   collateralAmount,
            debt:         zusdAmount,
            lastFeeIndex: feeIndex,
            openedAt:     block.timestamp
        });

        totalCollateral += collateralAmount;
        totalDebt       += zusdAmount;

        // Mint ZUSD to user.
        IZUSD_Mint(zusd).mint(msg.sender, zusdAmount);

        emit CDPOpened(msg.sender, collateralAmount, zusdAmount);
    }

    // ─── Add collateral ───────────────────────────────────────────────────

    /// @notice Add more ZBX to your CDP (reduces liquidation risk).
    function addCollateral(uint256 amount) external nonReentrant {
        require(cdps[msg.sender].collateral > 0, "Vault: no CDP");
        _accrueStabilityFee();

        require(IZBX_Transfer(zbx).transferFrom(msg.sender, address(this), amount),
                "Vault: transfer failed");
        cdps[msg.sender].collateral += amount;
        totalCollateral             += amount;

        emit CollateralAdded(msg.sender, amount);
    }

    // ─── Mint more ZUSD ───────────────────────────────────────────────────

    /// @notice Mint additional ZUSD from existing CDP.
    /// @dev    `totalDebt` MUST be adjusted by the principal delta
    ///         `(newPrincipal - oldPrincipal)`, NOT just `+= zusdAmount`.
    ///         The buggy 1-arg form silently DROPS the accrued fee
    ///         (`currentDebt - oldPrincipal`) from the global counter,
    ///         which then trips the strict invariant guards in repay /
    ///         closeCDP / liquidate / redeem on the next touch.
    ///         See S15-P2-D-2 audit block + LESSON #24-strict.
    function mintMore(uint256 zusdAmount) external nonReentrant {
        CDP storage cdp = cdps[msg.sender];
        require(cdp.collateral > 0, "Vault: no CDP");
        _accrueStabilityFee();

        uint256 oldPrincipal = cdp.debt;                   // snapshot BEFORE
        uint256 currentDebt  = _currentDebt(msg.sender);
        uint256 newPrincipal = currentDebt + zusdAmount;   // capitalises accrued fee + new mint

        uint256 zbxPrice    = _freshPrice(zbx);
        uint256 colValueUSD = cdp.collateral * zbxPrice / 1e18;
        require(colValueUSD * BPS / newPrincipal >= MIN_COLLATERAL_RATIO, "Vault: CR too low");

        cdp.debt         = newPrincipal;
        cdp.lastFeeIndex = feeIndex;
        // Always an INCREASE: newPrincipal = oldPrincipal × F + zusdAmount,
        // F ≥ 1 (feeIndex monotonically grows), zusdAmount > 0 by `mint()`
        // semantics (and any 0-amount call still no-ops correctly: delta = 0).
        totalDebt       += (newPrincipal - oldPrincipal);

        IZUSD_Mint(zusd).mint(msg.sender, zusdAmount);
        emit ZusdMinted(msg.sender, zusdAmount);
    }

    // ─── Repay ────────────────────────────────────────────────────────────

    /// @notice Repay ZUSD to reduce your debt.
    /// @dev    `totalDebt` accounting (post-S15-P2-D fix): `totalDebt` is
    ///         the sum of `cdp.debt` PRINCIPAL SNAPSHOTS, not current debt.
    ///         `_accrueStabilityFee()` only bumps `feeIndex` — per-CDP debt
    ///         growth is implicit until next touch. So we MUST adjust
    ///         `totalDebt` by the principal delta `(oldPrincipal -
    ///         newPrincipal)`, NOT by `repayAmount` (which is in
    ///         current-debt units and over-decrements by the accrued fee).
    ///         See S15-P2-D audit block + LESSON #24.
    function repay(uint256 zusdAmount) external nonReentrant {
        CDP storage cdp = cdps[msg.sender];
        require(cdp.collateral > 0, "Vault: no CDP");
        _accrueStabilityFee();

        uint256 oldPrincipal = cdp.debt;                       // snapshot BEFORE
        uint256 currentDebt  = _currentDebt(msg.sender);
        uint256 repayAmount  = zusdAmount > currentDebt ? currentDebt : zusdAmount;

        IZUSD_Mint(zusd).burn(msg.sender, repayAmount);
        uint256 newPrincipal = currentDebt - repayAmount;
        cdp.debt             = newPrincipal;
        cdp.lastFeeIndex     = feeIndex;

        // Adjust `totalDebt` by SIGNED principal delta (handles fee-accrual
        // correctly). If user paid less than accrued fee, principal grows
        // and totalDebt INCREASES; typical case principal shrinks and
        // totalDebt DECREASES with strict invariant guard.
        if (newPrincipal >= oldPrincipal) {
            totalDebt += (newPrincipal - oldPrincipal);
        } else {
            uint256 reduction = oldPrincipal - newPrincipal;
            require(totalDebt >= reduction, "Vault: totalDebt invariant broken");
            totalDebt -= reduction;
        }

        emit ZusdRepaid(msg.sender, repayAmount);
    }

    // ─── Withdraw collateral ──────────────────────────────────────────────

    /// @notice Withdraw ZBX collateral (only if CR stays above 150%).
    function withdrawCollateral(uint256 amount) external nonReentrant {
        CDP storage cdp = cdps[msg.sender];
        require(cdp.collateral >= amount, "Vault: insufficient collateral");
        _accrueStabilityFee();

        uint256 newCollateral = cdp.collateral - amount;

        if (cdp.debt > 0) {
            uint256 zbxPrice    = _freshPrice(zbx);
            uint256 colValueUSD = newCollateral * zbxPrice / 1e18;
            uint256 currentDebt = _currentDebt(msg.sender);
            require(
                colValueUSD * BPS / currentDebt >= MIN_COLLATERAL_RATIO,
                "Vault: would breach min CR"
            );
        }

        cdp.collateral  -= amount;
        totalCollateral -= amount;

        require(IZBX_Transfer(zbx).transfer(msg.sender, amount), "Vault: transfer failed");
        emit CollateralWithdrawn(msg.sender, amount);
    }

    // ─── Close CDP ────────────────────────────────────────────────────────

    /// @notice Repay all debt and withdraw all collateral.
    /// @dev    `totalDebt` adjustment uses `oldPrincipal` (the CDP's stored
    ///         principal snapshot), NOT `fullDebt` (current debt with
    ///         accrued fee). See repay() doc + S15-P2-D audit + LESSON #24.
    ///         The user STILL burns `fullDebt` ZUSD — paying the accrued
    ///         fee — but `totalDebt` only ever tracked principal so we
    ///         decrement by `oldPrincipal` to keep the invariant exact.
    function closeCDP() external nonReentrant {
        CDP storage cdp = cdps[msg.sender];
        require(cdp.collateral > 0, "Vault: no CDP");
        _accrueStabilityFee();

        uint256 oldPrincipal = cdp.debt;                       // snapshot BEFORE
        uint256 fullDebt     = _currentDebt(msg.sender);

        if (fullDebt > 0) {
            IZUSD_Mint(zusd).burn(msg.sender, fullDebt);
            require(totalDebt >= oldPrincipal, "Vault: totalDebt invariant broken");
            totalDebt -= oldPrincipal;
        }

        uint256 collateral  = cdp.collateral;
        totalCollateral    -= collateral;
        delete cdps[msg.sender];

        require(IZBX_Transfer(zbx).transfer(msg.sender, collateral), "Vault: transfer failed");
    }

    // ─── Liquidation ──────────────────────────────────────────────────────

    /// @notice Liquidate an undercollateralised CDP.
    ///         Caller repays the debt and receives collateral + 10% bonus.
    ///
    /// @dev    LIQUIDATION RULE (vault v0.2, post-wallet-aware-removal):
    ///         A CDP is liquidatable solely when its current collateral
    ///         ratio is at or below `LIQUIDATION_RATIO` (100% — i.e. the
    ///         collateral USD value has fallen to or below the debt USD
    ///         value). Owner ZUSD wallet balance is NOT considered. See
    ///         in-body NOTE for the rationale.
    ///
    ///         Aligned with Aave / Liquity / MakerDAO. The previous
    ///         "wallet-aware protection" was removed because it allowed
    ///         borrowers to dodge liquidation indefinitely by simply
    ///         holding (not burning) ZUSD, leaving the protocol exposed.
    function liquidate(address cdpOwner) external nonReentrant {
        CDP storage cdp = cdps[cdpOwner];
        require(cdp.collateral > 0, "Vault: no CDP");
        _accrueStabilityFee();

        uint256 currentDebt = _currentDebt(cdpOwner);
        // SOL-01 (MEDIUM): guard against div-by-zero when a CDP had its
        // debt fully repaid via repay() but was never closed (collateral > 0,
        // debt == 0). Without this check `colValueUSD * BPS / currentDebt`
        // would revert, making the vault unable to seize the stranded
        // collateral. Treat a zero-debt CDP as healthy and block liquidation.
        require(currentDebt > 0, "Vault: CDP has no debt");
        uint256 zbxPrice    = _freshPrice(zbx);
        uint256 colValueUSD = cdp.collateral * zbxPrice / 1e18;
        uint256 cr          = colValueUSD * BPS / currentDebt;

        require(cr <= LIQUIDATION_RATIO, "Vault: CDP is healthy");

        // NOTE — a previous "wallet-aware protection" gated liquidation on
        // `ownerZusdBalance < currentDebt`. That was unsound: a borrower
        // could permanently dodge liquidation simply by *holding* (not
        // burning) ZUSD, leaving the protocol carrying the price risk on
        // an undercollateralised position. Aave / Liquity / MakerDAO all
        // gate solely on collateral ratio. Removed in v0.2.

        // Seize: debt value in ZBX + 10% bonus.
        uint256 debtInZbx   = currentDebt * 1e18 / zbxPrice;
        uint256 bonusZbx    = debtInZbx * LIQUIDATION_BONUS / BPS;
        uint256 seizeZbx    = debtInZbx + bonusZbx;

        // Cap at available collateral (in case of bad debt).
        if (seizeZbx > cdp.collateral) {
            seizeZbx = cdp.collateral;
        }

        // Burn ZUSD from liquidator.
        IZUSD_Mint(zusd).burn(msg.sender, currentDebt);

        // Update state — `totalDebt` adjusts by PRINCIPAL snapshot, not
        // current debt (S15-P2-D fix). See repay() doc + LESSON #24.
        uint256 oldPrincipal = cdp.debt;
        require(totalDebt >= oldPrincipal, "Vault: totalDebt invariant broken");
        totalDebt       -= oldPrincipal;
        totalCollateral -= seizeZbx;

        if (seizeZbx == cdp.collateral) {
            delete cdps[cdpOwner];
        } else {
            cdp.collateral  -= seizeZbx;
            cdp.debt         = 0;
            cdp.lastFeeIndex = feeIndex;
        }

        // Transfer seized collateral to liquidator.
        require(IZBX_Transfer(zbx).transfer(msg.sender, seizeZbx), "Vault: transfer failed");

        emit CDPLiquidated(cdpOwner, msg.sender, currentDebt, seizeZbx, bonusZbx);
    }

    // ─── Admin: redemption controls ───────────────────────────────────────

    /// @notice Owner-only emergency stop for redemptions.
    /// @dev    Used in: oracle outage, mass-liquidation event, suspicious
    ///         redemption patterns, or pre-audit cool-down windows.
    // S18: replaced 2 inline `require(msg.sender == owner)` with the
    //      inherited `onlyOwner` modifier from Ownable2Step.
    function setRedemptionPaused(bool paused) external onlyOwner {
        redemptionPaused = paused;
        emit RedemptionPausedToggled(msg.sender, paused);
    }

    /// @notice Owner-only setter for the per-second stability fee rate (in RAY units).
    ///
    /// @dev M53-01 FIX: Previously `stabilityFeePerSec` was a `constant` so no
    ///      setter existed and ZusdPricePeg.adjustPeg() could never change it.
    ///      Changing it from `constant` to a storage variable and adding this
    ///      setter completes the peg-recovery feedback loop.
    ///
    ///      This function MUST be called by ZusdPricePeg (after renouncing owner or
    ///      granting it the owner role), OR directly by the multisig owner.
    ///
    ///      Accrues outstanding fees at the OLD rate before switching, so that no
    ///      CDP gets retroactively penalised (or rewarded) by the rate change.
    ///
    /// @param newRate  Per-second fee in RAY (1e27) units.
    ///                 0 = fee disabled. Default (2% APY) = 627_937_192_491_029_810.
    function setStabilityFeeRate(uint256 newRate) external onlyOwner {
        _accrueStabilityFee();
        uint256 oldRate = stabilityFeePerSec;
        stabilityFeePerSec = newRate;
        emit StabilityFeeRateUpdated(oldRate, newRate);
    }

    /// @notice Owner-only setter for redemption-fee recipient (defaults to owner).
    function setFeeRecipient(address recipient) external onlyOwner {
        feeRecipient = recipient;
        emit FeeRecipientUpdated(recipient);
    }

    // ─── Redemption (S15-P2 — re-enabled, S6-V2 BUG FIXED) ────────────────

    /// @notice Redeem ZUSD for ZBX collateral, drawn from the CDPs supplied
    ///         in `cdpHints`, processed in ascending-CR order WITHIN that
    ///         supplied set.
    ///
    /// @dev    DESIGN — Hint-based, monotonicity-checked redemption.
    ///
    ///         FAIRNESS DISCLOSURE (architect-review M1, S15-P2-A):
    ///         The on-chain monotonicity check enforces ascending CR ONLY
    ///         within the caller-supplied address list. It does NOT prove
    ///         that the supplied list contains the globally-lowest-CR CDPs
    ///         in the protocol — the caller could omit a lower-CR CDP and
    ///         target a higher-CR one instead. The SDK helper supplies the
    ///         canonical lowest-CR ordering off-chain; on-chain enforcement
    ///         of "lowest-CR-first globally" requires a sorted CDP linked
    ///         list (deferred to mainnet ZEP — see ZEP-005 §7).
    ///
    ///         What the on-chain check DOES guarantee:
    ///           - Caller cannot redeem against an out-of-order list
    ///             (ascending CR enforced).
    ///           - Caller cannot drain the vault (per-CDP atomic update).
    ///           - Caller cannot redeem against an unhealthy CDP
    ///             (CR < 100% reverts → must be liquidated instead).
    ///           - Caller cannot create dust CDPs (post-debt cap).
    ///
    ///         What it does NOT guarantee:
    ///           - That the GLOBALLY lowest-CR CDP is hit first. Mitigation
    ///             is economic + off-chain: any low-CR CDP omitted from one
    ///             call remains the most-attractive target for the next
    ///             redeemer (same arbitrage opportunity, lower competition).
    ///
    ///         Per-CDP processing:
    ///           1. Skip if cdp.collateral == 0 (closed).
    ///           2. Skip if currentDebt == 0 (no debt to redeem against).
    ///           3. require(CR >= 100%) — refuse to redeem from unhealthy
    ///              CDPs (those go through `liquidate()` instead).
    ///           4. require(CR >= prevCR) — monotonicity / hint check.
    ///           5. Cap redemption to leave either zero debt (full close)
    ///              or >= MIN_ZUSD_MINT debt (no dust CDPs).
    ///           6. Compute zbxOut = redeemAmount * 1e18 / zbxPrice
    ///              (rounds down → favours protocol).
    ///           7. ATOMICALLY decrement cdp.collateral, cdp.debt,
    ///              totalCollateral, totalDebt. THIS IS THE S6-V2 FIX.
    ///           8. If cdp.debt == 0 after redemption, return leftover
    ///              collateral to the original CDP owner and delete the
    ///              CDP (mirrors closeCDP behaviour).
    ///
    ///         Final settlement:
    ///           - Burn `zusdRedeemed` from caller (must hold the ZUSD).
    ///           - Transfer (zbxOut - 0.5% fee) to caller.
    ///           - Transfer 0.5% fee to feeRecipient (or owner if unset).
    ///
    ///         SAFETY PROPERTIES (proved by tests):
    ///           - Vault solvency invariant:
    ///               sum(cdp.collateral) + leftover_returns == ZBX_in_vault
    ///           - Accounting invariant:
    ///               totalCollateral == sum(cdp.collateral)
    ///               totalDebt       >= sum(cdp.debt)  (>= because of fee accrual lag)
    ///           - Per-CDP CR-monotone: CR after redemption >= CR before
    ///             (proven by algebra when CR >= 100% pre-redemption).
    ///           - Caller cannot drain the vault: each ZBX out is matched
    ///             by exactly that ZBX worth of debt reduction in some CDP.
    ///
    /// @param zusdAmount     Total ZUSD the caller wants to redeem.
    /// @param cdpHints       Off-chain-sorted CDP owners (ascending CR).
    /// @param maxIterations  Max CDPs to traverse (1..MAX_REDEEM_ITER).
    /// @return zusdRedeemed  Actual ZUSD burned (may be < zusdAmount).
    /// @return zbxOut        Net ZBX sent to caller (post-fee).
    function redeem(
        uint256 zusdAmount,
        address[] calldata cdpHints,
        uint256 maxIterations
    ) external nonReentrant returns (uint256 zusdRedeemed, uint256 zbxOut) {
        require(!redemptionPaused,                                "Vault: redemption paused");
        require(zusdAmount >= MIN_REDEEM_AMOUNT,                  "Vault: below min redeem");
        require(cdpHints.length > 0,                              "Vault: empty hints");
        require(maxIterations > 0 && maxIterations <= MAX_REDEEM_ITER, "Vault: bad iter bound");
        require(IZUSD_Mint(zusd).balanceOf(msg.sender) >= zusdAmount,
                                                                  "Vault: insufficient ZUSD");

        _accrueStabilityFee();

        uint256 zbxPrice = _freshPrice(zbx);
        require(zbxPrice > 0, "Vault: oracle price zero");

        uint256 remaining     = zusdAmount;
        uint256 grossZbxOut   = 0;
        uint256 prevCRBps     = 0;          // monotonicity tracker (must non-decrease)
        uint256 iter          = 0;

        for (uint256 i = 0; i < cdpHints.length; i++) {
            if (remaining == 0 || iter >= maxIterations) break;

            address cdpOwner = cdpHints[i];
            CDP storage cdp  = cdps[cdpOwner];

            // Skip empty / closed CDPs (no revert — caller's hint may be stale).
            if (cdp.collateral == 0) continue;

            uint256 oldPrincipal = cdp.debt;                   // snapshot BEFORE (S15-P2-D)
            uint256 cdpDebt      = _currentDebt(cdpOwner);
            if (cdpDebt == 0) continue;

            // Compute current CR; gate on health and monotonicity.
            uint256 colValueUSD = cdp.collateral * zbxPrice / 1e18;
            uint256 crBps       = colValueUSD * BPS / cdpDebt;

            require(crBps >= LIQUIDATION_RATIO,
                    "Vault: unhealthy CDP — liquidate, do not redeem");
            require(crBps >= prevCRBps,
                    "Vault: hints not sorted ascending");
            prevCRBps = crBps;

            // Cap redemption to either (a) full debt, or (b) leave >= MIN_ZUSD_MINT.
            uint256 redeemFromCdp = remaining > cdpDebt ? cdpDebt : remaining;
            uint256 postDebt      = cdpDebt - redeemFromCdp;
            if (postDebt > 0 && postDebt < MIN_ZUSD_MINT) {
                // Avoid dust: either fully redeem this CDP or leave >= MIN.
                if (cdpDebt > MIN_ZUSD_MINT) {
                    redeemFromCdp = cdpDebt - MIN_ZUSD_MINT;
                    postDebt      = MIN_ZUSD_MINT;
                } else {
                    // CDP itself is sub-min; only full redemption is allowed.
                    if (redeemFromCdp != cdpDebt) {
                        // Skip — partial would create dust we can't avoid.
                        continue;
                    }
                }
            }

            // Compute ZBX out (rounds down → favours protocol).
            uint256 zbxFromCdp = redeemFromCdp * 1e18 / zbxPrice;

            // Defensive cap: never take more ZBX than the CDP has. Healthy
            // CDPs (CR >= 100%) mathematically can't trigger this; defensive
            // for oracle precision drift.
            if (zbxFromCdp > cdp.collateral) {
                zbxFromCdp    = cdp.collateral;
                redeemFromCdp = zbxFromCdp * zbxPrice / 1e18;
                if (redeemFromCdp == 0) continue;
                // STRICT — recomputed redeemFromCdp can only DECREASE from
                // its original min(remaining, cdpDebt), so cdpDebt >=
                // redeemFromCdp is guaranteed. Solidity 0.8 will auto-revert
                // if this analysis is ever wrong (LESSON #18).
                postDebt      = cdpDebt - redeemFromCdp;
            }

            // ─── THE S6-V2 FIX: atomic per-CDP record update ──────────────
            //
            // STRICT subtraction (no clamp-to-zero) — Solidity 0.8.24
            // built-in overflow protection will revert on impossible
            // underflow, which is the correct behaviour: clamping would
            // silently mask an invariant violation. See architect-review
            // M2 (S15-P2-A audit block).
            //
            // `totalDebt` adjusted by PRINCIPAL delta (`oldPrincipal -
            // postDebt`), NOT by `redeemFromCdp` (current-debt units).
            // See repay() doc + S15-P2-D audit + LESSON #24.
            cdp.collateral   -= zbxFromCdp;
            cdp.debt          = postDebt;                       // newPrincipal
            cdp.lastFeeIndex  = feeIndex;

            totalCollateral  -= zbxFromCdp;
            if (postDebt >= oldPrincipal) {
                totalDebt += (postDebt - oldPrincipal);
            } else {
                uint256 principalReduction = oldPrincipal - postDebt;
                require(totalDebt >= principalReduction,
                        "Vault: totalDebt invariant broken");
                totalDebt -= principalReduction;
            }

            remaining        -= redeemFromCdp;
            grossZbxOut      += zbxFromCdp;

            emit RedeemedFromCDP(cdpOwner, redeemFromCdp, zbxFromCdp);

            // Full-close path: return leftover collateral to original owner.
            // STRICT subtraction (no clamp) — see M2 note above.
            if (postDebt == 0) {
                uint256 leftover = cdp.collateral;
                if (leftover > 0) {
                    cdp.collateral  = 0;
                    require(totalCollateral >= leftover,
                            "Vault: totalCollateral invariant broken");
                    totalCollateral -= leftover;
                    require(IZBX_Transfer(zbx).transfer(cdpOwner, leftover),
                            "Vault: leftover transfer failed");
                }
                delete cdps[cdpOwner];
                emit CDPClosedByRedemption(cdpOwner, leftover);
            }

            iter++;
        }

        require(grossZbxOut > 0, "Vault: nothing redeemed (check hints)");

        zusdRedeemed = zusdAmount - remaining;

        // Burn the redeemer's ZUSD.
        IZUSD_Mint(zusd).burn(msg.sender, zusdRedeemed);

        // Apply 0.5% fee on gross ZBX out.
        uint256 fee     = grossZbxOut * REDEMPTION_FEE_BPS / BPS;
        zbxOut          = grossZbxOut - fee;

        // Lifetime analytics.
        totalRedeemed         += zusdRedeemed;
        totalRedemptionFees   += fee;

        require(IZBX_Transfer(zbx).transfer(msg.sender, zbxOut),
                "Vault: redeemer transfer failed");

        if (fee > 0) {
            address feeAddr = feeRecipient == address(0) ? owner : feeRecipient;
            require(IZBX_Transfer(zbx).transfer(feeAddr, fee),
                    "Vault: fee transfer failed");
        }

        emit Redeemed(msg.sender, zusdRedeemed, zbxOut);
    }

    // ─── Views ────────────────────────────────────────────────────────────

    /// @notice Get CDP info including real-time collateral ratio.
    function getCDP(address user) external view returns (
        uint256 collateral, uint256 debt, uint256 collateralRatioBps, bool liquidatable
    ) {
        CDP storage cdp = cdps[user];
        collateral = cdp.collateral;
        debt       = cdp.debt; // approximate (no live fee accrual in view)

        if (debt == 0 || collateral == 0) {
            collateralRatioBps = type(uint256).max;
            liquidatable       = false;
        } else {
            uint256 zbxPrice    = _freshPrice(zbx);
            uint256 colValueUSD = collateral * zbxPrice / 1e18;
            collateralRatioBps  = colValueUSD * BPS / debt;
            liquidatable        = collateralRatioBps < LIQUIDATION_RATIO;
        }
    }

    /// @notice Max ZUSD mintable for a given ZBX collateral amount.
    function maxMintable(uint256 zbxAmount) external view returns (uint256) {
        uint256 zbxPrice    = _freshPrice(zbx);
        uint256 colValueUSD = zbxAmount * zbxPrice / 1e18;
        return colValueUSD * BPS / MIN_COLLATERAL_RATIO;
    }

    // ─── Internal ─────────────────────────────────────────────────────────

    /// @dev Update global stability fee accumulator.
    function _accrueStabilityFee() internal {
        uint256 elapsed = block.timestamp - lastFeeUpdate;
        if (elapsed == 0) return;
        // feeIndex grows by STABILITY_FEE_PER_SEC each second (compounded).
        feeIndex      = feeIndex + (feeIndex * stabilityFeePerSec * elapsed / RAY);
        lastFeeUpdate = block.timestamp;
    }

    /// @dev Calculate current debt of a CDP (with accrued stability fee).
    function _currentDebt(address user) internal view returns (uint256) {
        CDP storage cdp = cdps[user];
        if (cdp.debt == 0) return 0;
        return cdp.debt * feeIndex / cdp.lastFeeIndex;
    }
}