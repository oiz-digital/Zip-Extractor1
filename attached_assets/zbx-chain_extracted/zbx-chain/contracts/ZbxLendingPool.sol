// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title ZbxLendingPool — Aave-style DeFi lending protocol on Zebvix Chain.
/// @notice Allows users to:
///           - Supply assets (earn interest)
///           - Borrow against collateral (overcollateralised)
///           - Receive flash loans (borrowed + repaid in same tx)
///           - Earn ZBX reward emissions on top of interest
///
/// @dev   Interest rates use a kinked rate model:
///           - Below optimal utilisation: base rate + (util/optimal) * slope1
///           - Above optimal utilisation: base + slope1 + (excess/max_excess) * slope2
///
/// @dev   STORAGE SEMANTICS (Aave v2 style):
///         - `userCollateral`, `userDebt`, `reserve.totalSupplied`,
///           `reserve.totalBorrowed` store *scaled* values: real / index.
///         - Real balance at any time = scaled * index / RAY.
///         - This allows per-user interest accrual with O(1) writes per
///           reserve via index updates only.
///
/// @custom:zbx-chain  Chain ID 8989

import "./libraries/Governable.sol";
import "./libraries/ReentrancyGuard.sol";

interface IZRC20Transfer {
    function transfer(address to, uint256 amount) external returns (bool);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function balanceOf(address account) external view returns (uint256);
    function decimals() external view returns (uint8);
}

interface IZbxOracle {
    function getPrice(address asset) external view returns (uint256);
}

contract ZbxLendingPool is Governable, ReentrancyGuard {

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

    // ─── Data structures ─────────────────────────────────────────────────

    struct ReserveData {
        address asset;
        address zToken;             // interest-bearing token for suppliers
        address debtToken;          // debt token for borrowers
        uint128 totalSupplied;      // SCALED supply
        uint128 totalBorrowed;      // SCALED debt
        uint128 liquidityRate;      // current supply APR (ray = 1e27)
        uint128 borrowRate;         // current borrow APR (ray = 1e27)
        uint128 liquidityIndex;     // cumulative supply interest index
        uint128 borrowIndex;        // cumulative borrow interest index
        uint40  lastUpdateTimestamp;
        uint16  ltv;                // loan-to-value ratio (bps, e.g. 8000 = 80%)
        uint16  liquidationThreshold; // (bps, e.g. 8250 = 82.5%)
        uint16  liquidationBonus;     // (bps, e.g. 10500 = 105%)
        uint16  reserveFactor;        // share of borrow interest kept by protocol (bps)
        uint8   decimals;
        bool    active;
        bool    borrowEnabled;
        bool    flashLoanEnabled;
    }

    struct InterestRateConfig {
        uint128 baseBorrowRate;     // ray, e.g. 0%
        uint128 slope1;             // ray, rate at optimal util
        uint128 slope2;             // ray, additional rate above optimal
        uint16  optimalUtilization; // bps, e.g. 8000 = 80%
    }

    struct UserAccountData {
        uint256 totalCollateral;    // in USD (oracle decimals)
        uint256 totalDebt;          // in USD
        uint256 availableToBorrow;  // in USD
        uint256 currentLtv;
        uint256 healthFactor;       // 1e18 = healthy boundary, <1e18 = liquidatable
    }

    // ─── State ────────────────────────────────────────────────────────────

    mapping(address => ReserveData)                 public reserves;
    mapping(address => InterestRateConfig)          public rateConfig;
    mapping(address => mapping(address => uint256)) public userCollateral; // scaled
    mapping(address => mapping(address => uint256)) public userDebt;       // scaled
    address[]                                       public reserveList;

    /// @notice Credit delegation: `borrowAllowance[delegator][delegatee][asset]`
    ///         is the max wei `delegatee` may borrow on behalf of `delegator`.
    mapping(address => mapping(address => mapping(address => uint256))) public borrowAllowance;

    address public oracle;
    // `owner`, `pendingOwner`, `governor` and the 2-step ownership transfer
    // come from the Governable base contract. High-risk admin functions
    // (init/config/flag changes) are gated by `onlyAdmin`, which routes to
    // the timelock once `setGovernor(...)` is called.

    uint256 public constant RAY              = 1e27;
    uint256 public constant SECONDS_PER_YEAR = 365 days;
    uint256 public constant HEALTH_THRESHOLD = 1e18;
    /// @notice Aave-style close factor — a single liquidation may repay at
    ///         most 50% of the user's debt for the chosen asset.
    uint256 public constant CLOSE_FACTOR_BPS = 5000;
    /// @notice Default protocol cut from borrow interest if a reserve is
    ///         configured with reserveFactor == 0 (10% by default).
    uint16  public constant DEFAULT_RESERVE_FACTOR = 1000;

    /// @dev S25-Y2: inline `_entry` guard migrated to shared
    ///      `ReentrancyGuard` library (see contracts/libraries/ReentrancyGuard.sol).
    ///      Same `_NOT_ENTERED=1` / `_ENTERED=2` semantics; `nonReentrant`
    ///      modifier now inherited so audit surface is one file instead of seven.

    // ─── Events ───────────────────────────────────────────────────────────

    event Supply(address indexed asset, address indexed user, uint256 amount);
    event Withdraw(address indexed asset, address indexed user, uint256 amount);
    event Borrow(address indexed asset, address indexed user, uint256 amount);
    event Repay(address indexed asset, address indexed user, uint256 amount);
    event Liquidate(
        address indexed collateral, address indexed debt, address indexed user,
        uint256 debtToCover, uint256 liquidatedAmount
    );
    event FlashLoan(address indexed receiver, address indexed asset, uint256 amount, uint256 fee);
    event ReserveDataUpdated(
        address indexed asset, uint256 liquidityRate, uint256 borrowRate,
        uint256 liquidityIndex, uint256 borrowIndex
    );
    event BorrowAllowanceSet(
        address indexed delegator, address indexed delegatee,
        address indexed asset, uint256 amount
    );

    constructor(address oracle_) Governable(msg.sender) {
        require(oracle_ != address(0), "Pool: zero oracle");
        oracle = oracle_;
    }

    // ─── Supply ───────────────────────────────────────────────────────────

    function supply(address asset, uint256 amount, address onBehalfOf) external nonReentrant {
        require(amount > 0, "Pool: zero amount");
        require(onBehalfOf != address(0), "Pool: zero recipient");
        ReserveData storage reserve = _getActiveReserve(asset);
        _updateState(reserve);

        // Pull funds first (CEI-safe: state mutation after).
        IZRC20Transfer(asset).transferFrom(msg.sender, address(this), amount);

        uint256 scaled = (amount * RAY) / reserve.liquidityIndex;
        require(scaled > 0, "Pool: scaled supply rounds to zero");

        reserve.totalSupplied              += uint128(scaled);
        userCollateral[onBehalfOf][asset]  += scaled;

        _updateInterestRates(reserve);
        emit Supply(asset, onBehalfOf, amount);
    }

    function withdraw(address asset, uint256 amount, address to) external nonReentrant returns (uint256) {
        require(to != address(0), "Pool: zero recipient");
        ReserveData storage reserve = _getActiveReserve(asset);
        _updateState(reserve);

        uint256 scaledBal  = userCollateral[msg.sender][asset];
        uint256 realBal    = (scaledBal * reserve.liquidityIndex) / RAY;
        uint256 withdrawAmount = amount == type(uint256).max ? realBal : amount;
        require(withdrawAmount <= realBal, "Pool: insufficient supplied balance");
        require(withdrawAmount > 0, "Pool: zero withdraw");

        // Round scaled deduction up so users cannot withdraw a wei more than backed.
        uint256 scaledWithdraw = (withdrawAmount * RAY + reserve.liquidityIndex - 1) / reserve.liquidityIndex;
        if (scaledWithdraw > scaledBal) scaledWithdraw = scaledBal;

        userCollateral[msg.sender][asset]  = scaledBal - scaledWithdraw;
        reserve.totalSupplied             -= uint128(scaledWithdraw);

        // Health check after deducting collateral.
        require(_healthFactor(msg.sender) >= HEALTH_THRESHOLD, "Pool: health factor too low");

        IZRC20Transfer(asset).transfer(to, withdrawAmount);
        _updateInterestRates(reserve);

        emit Withdraw(asset, msg.sender, withdrawAmount);
        return withdrawAmount;
    }

    // ─── Borrow ───────────────────────────────────────────────────────────

    function borrow(address asset, uint256 amount, address onBehalfOf) external nonReentrant {
        require(amount > 0, "Pool: zero amount");
        require(onBehalfOf != address(0), "Pool: zero borrower");
        ReserveData storage reserve = _getActiveReserve(asset);
        require(reserve.borrowEnabled, "Pool: borrowing not enabled for this asset");

        if (msg.sender != onBehalfOf) {
            uint256 allowed = borrowAllowance[onBehalfOf][msg.sender][asset];
            require(allowed >= amount, "Pool: borrow allowance insufficient");
            unchecked {
                borrowAllowance[onBehalfOf][msg.sender][asset] = allowed - amount;
            }
        }

        _updateState(reserve);

        uint256 scaled = (amount * RAY + reserve.borrowIndex - 1) / reserve.borrowIndex;
        require(scaled > 0, "Pool: scaled borrow rounds to zero");

        userDebt[onBehalfOf][asset]  += scaled;
        reserve.totalBorrowed        += uint128(scaled);

        require(_healthFactor(onBehalfOf) >= HEALTH_THRESHOLD, "Pool: collateral insufficient");

        IZRC20Transfer(asset).transfer(msg.sender, amount);
        _updateInterestRates(reserve);

        emit Borrow(asset, onBehalfOf, amount);
    }

    function approveDelegation(address delegatee, address asset, uint256 amount) external {
        require(delegatee != address(0), "Pool: zero delegatee");
        borrowAllowance[msg.sender][delegatee][asset] = amount;
        emit BorrowAllowanceSet(msg.sender, delegatee, asset, amount);
    }

    function repay(address asset, uint256 amount, address onBehalfOf) external nonReentrant returns (uint256) {
        ReserveData storage reserve = _getActiveReserve(asset);
        _updateState(reserve);

        uint256 scaledDebt = userDebt[onBehalfOf][asset];
        uint256 realDebt   = (scaledDebt * reserve.borrowIndex) / RAY;
        uint256 repayAmount = amount == type(uint256).max ? realDebt : amount;
        require(repayAmount > 0, "Pool: zero repay");
        require(repayAmount <= realDebt, "Pool: repay exceeds debt");

        // Round scaled deduction down so debt is never under-cleared.
        uint256 scaledRepay = (repayAmount * RAY) / reserve.borrowIndex;
        if (scaledRepay > scaledDebt) scaledRepay = scaledDebt;

        IZRC20Transfer(asset).transferFrom(msg.sender, address(this), repayAmount);

        userDebt[onBehalfOf][asset] = scaledDebt - scaledRepay;
        reserve.totalBorrowed       -= uint128(scaledRepay);

        _updateInterestRates(reserve);
        emit Repay(asset, onBehalfOf, repayAmount);
        return repayAmount;
    }

    // ─── Liquidation ──────────────────────────────────────────────────────

    function liquidate(
        address collateralAsset,
        address debtAsset,
        address user,
        uint256 debtToCover
    ) external nonReentrant {
        require(_healthFactor(user) < HEALTH_THRESHOLD, "Pool: position healthy");
        require(user != msg.sender, "Pool: self-liquidate");

        ReserveData storage debtReserve       = _getActiveReserve(debtAsset);
        ReserveData storage collateralReserve = _getActiveReserve(collateralAsset);
        _updateState(debtReserve);
        _updateState(collateralReserve);

        // Aave close factor: at most 50% of outstanding debt may be repaid.
        uint256 scaledDebt   = userDebt[user][debtAsset];
        uint256 outstanding  = (scaledDebt * debtReserve.borrowIndex) / RAY;
        uint256 maxClose     = (outstanding * CLOSE_FACTOR_BPS) / 10000;
        if (debtToCover > maxClose) debtToCover = maxClose;
        require(debtToCover > 0, "Pool: nothing to liquidate");

        uint256 debtPrice       = IZbxOracle(oracle).getPrice(debtAsset);
        uint256 collateralPrice = IZbxOracle(oracle).getPrice(collateralAsset);
        require(debtPrice > 0 && collateralPrice > 0, "Pool: bad oracle price");

        // collateralAmount (real) = debtToCover * (debtPrice/collateralPrice) * bonus / 10000
        // Both prices use same scale → ratio cancels.
        uint256 collateralAmount = (debtToCover * debtPrice * collateralReserve.liquidationBonus)
            / (collateralPrice * 10000);
        require(collateralAmount > 0, "Pool: zero collateral seize");

        uint256 scaledColl    = userCollateral[user][collateralAsset];
        uint256 collRealBal   = (scaledColl * collateralReserve.liquidityIndex) / RAY;
        require(collateralAmount <= collRealBal, "Pool: insufficient collateral");

        // CEI: state-update before external transfers.
        uint256 scaledRepay   = (debtToCover * RAY) / debtReserve.borrowIndex;
        if (scaledRepay > scaledDebt) scaledRepay = scaledDebt;
        uint256 scaledSeize   = (collateralAmount * RAY + collateralReserve.liquidityIndex - 1)
                                  / collateralReserve.liquidityIndex;
        if (scaledSeize > scaledColl) scaledSeize = scaledColl;

        userDebt[user][debtAsset]              = scaledDebt - scaledRepay;
        userCollateral[user][collateralAsset]  = scaledColl - scaledSeize;
        debtReserve.totalBorrowed             -= uint128(scaledRepay);
        collateralReserve.totalSupplied       -= uint128(scaledSeize);

        IZRC20Transfer(debtAsset).transferFrom(msg.sender, address(this), debtToCover);
        IZRC20Transfer(collateralAsset).transfer(msg.sender, collateralAmount);

        _updateInterestRates(debtReserve);
        _updateInterestRates(collateralReserve);
        emit Liquidate(collateralAsset, debtAsset, user, debtToCover, collateralAmount);
    }

    // ─── Flash Loans ──────────────────────────────────────────────────────

    function flashLoan(
        address receiver,
        address asset,
        uint256 amount,
        bytes calldata params
    ) external nonReentrant {
        ReserveData storage reserve = _getActiveReserve(asset);
        require(reserve.flashLoanEnabled, "Pool: flash loans not enabled");
        require(amount > 0, "Pool: zero amount");
        require(receiver != address(0) && receiver != address(this), "Pool: bad receiver");

        uint256 fee = (amount * 9) / 10_000;
        // SEC-2026-05-09 Pass-19 (Tier-2 #4): close the dust-fee
        // bypass — `(amount * 9) / 10_000` rounds to ZERO for any
        // amount < 1112 wei, allowing free flash-loans of dust
        // amounts. Reject explicitly so MIN_FLASH_AMOUNT is enforced
        // by arithmetic rather than economics.
        require(fee > 0, "Pool: flash amount too small (zero fee)");

        uint256 balanceBefore = IZRC20Transfer(asset).balanceOf(address(this));
        require(balanceBefore >= amount, "Pool: insufficient liquidity");

        IZRC20Transfer(asset).transfer(receiver, amount);

        (bool ok, ) = receiver.call(
            abi.encodeWithSignature(
                "executeOperation(address,uint256,uint256,address,bytes)",
                asset, amount, fee, msg.sender, params
            )
        );
        require(ok, "Pool: flash loan callback failed");

        uint256 balanceAfter = IZRC20Transfer(asset).balanceOf(address(this));
        require(balanceAfter >= balanceBefore + fee, "Pool: flash loan not repaid in full");

        // Credit fee to suppliers by inflating liquidityIndex proportionally.
        // Δindex / index ≈ fee / poolReal  → newIndex = index * (poolReal + fee) / poolReal
        _updateState(reserve);
        uint256 supplied = (uint256(reserve.totalSupplied) * reserve.liquidityIndex) / RAY;
        if (supplied > 0) {
            reserve.liquidityIndex = uint128(
                (uint256(reserve.liquidityIndex) * (supplied + fee)) / supplied
            );
        }

        _updateInterestRates(reserve);
        emit FlashLoan(receiver, asset, amount, fee);
    }

    // ─── Views ────────────────────────────────────────────────────────────

    function getUserAccountData(address user)
        external view returns (UserAccountData memory data)
    {
        uint256 weightedThreshold;
        uint256 weightedLtv;
        for (uint256 i; i < reserveList.length; ++i) {
            address asset = reserveList[i];
            ReserveData storage r = reserves[asset];
            uint256 price = IZbxOracle(oracle).getPrice(asset);
            uint256 collReal = (userCollateral[user][asset] * r.liquidityIndex) / RAY;
            uint256 debtReal = (userDebt[user][asset]       * r.borrowIndex)    / RAY;
            uint256 collUsd  = (collReal * price) / 1e18;
            uint256 debtUsd  = (debtReal * price) / 1e18;
            data.totalCollateral += collUsd;
            data.totalDebt       += debtUsd;
            weightedThreshold    += collUsd * r.liquidationThreshold;
            weightedLtv          += collUsd * r.ltv;
        }
        if (data.totalCollateral > 0) {
            data.currentLtv      = weightedLtv / data.totalCollateral;
        }
        if (data.totalDebt == 0) {
            data.healthFactor = type(uint256).max;
        } else if (data.totalCollateral == 0) {
            // DEFI-01 fix: no collateral backing any debt → health = 0 (fully insolvent).
            // Pre-fix: `weightedThreshold / data.totalCollateral` would divide-by-zero and
            // revert, making getUserAccountData unusable for such positions.
            data.healthFactor = 0;
        } else {
            uint256 thr = weightedThreshold / data.totalCollateral; // bps
            data.healthFactor = (data.totalCollateral * thr * 1e18) / (10000 * data.totalDebt);
        }
        if (data.totalCollateral > 0 && data.totalDebt < (data.totalCollateral * weightedLtv) / (10000 * data.totalCollateral)) {
            data.availableToBorrow =
                (data.totalCollateral * weightedLtv) / (10000 * data.totalCollateral) - data.totalDebt;
        }
    }

    /// @notice Number of asset reserves currently registered with the pool.
    /// @dev    Added in S17-T03 so the ZbxTvlOracle can iterate `reserveList`
    ///         without relying on factory-style probing. Forward-compatible:
    ///         older off-chain consumers can ignore this getter.
    function reservesCount() external view returns (uint256) {
        return reserveList.length;
    }

    /// @notice Single-call accessor returning the entire `ReserveData` struct.
    /// @dev    Added in S17-T03. The auto-getter on the public `reserves`
    ///         mapping already returns every field, but as 18 separate return
    ///         values which is cumbersome for external callers. This wrapper
    ///         keeps the call ergonomics close to Aave's `getReserveData`.
    function getReserveData(address asset) external view returns (ReserveData memory) {
        return reserves[asset];
    }

    /// @notice Real (interest-accrued) supply balance for a user.
    function balanceOfSupplied(address user, address asset) external view returns (uint256) {
        return (userCollateral[user][asset] * reserves[asset].liquidityIndex) / RAY;
    }

    /// @notice Real (interest-accrued) debt balance for a user.
    function balanceOfDebt(address user, address asset) external view returns (uint256) {
        return (userDebt[user][asset] * reserves[asset].borrowIndex) / RAY;
    }

    // ─── Internal ─────────────────────────────────────────────────────────

    /// SEC-2026-05-09 Pass-19 (Tier-2 #3): defensive oracle freshness
    /// at every health-factor evaluation. Same pattern as
    /// `OracleFreshness.assertFresh` in ZusdVault — inlined here to
    /// avoid a cross-contract import for a 4-line check. Stale oracle
    /// data on the lending side enables the borrow-then-stale-price
    /// attack: borrow at fresh-high, wait for price drop, oracle
    /// stays stale at old high, liquidation never fires while
    /// borrower's debt is structurally insolvent.
    uint256 internal constant POOL_MAX_STALENESS = 3600;

    /// @dev Per-asset weighted health factor across all reserves.
    function _healthFactor(address user) internal view returns (uint256) {
        uint256 weightedColl;
        uint256 totalDebtUsd;
        for (uint256 i; i < reserveList.length; ++i) {
            address asset = reserveList[i];
            ReserveData storage r = reserves[asset];
            uint256 price = IZbxOracle(oracle).getPrice(asset);
            require(price > 0, "Pool: missing oracle price");
            // Pass-19: best-effort freshness probe via Chainlink-style
            // latestRoundData. Wrapped in try/catch so minimal oracles
            // (legacy interface) still work in dev/test.
            (bool ok, bytes memory ret) = oracle.staticcall(
                abi.encodeWithSignature("latestRoundData(address)", asset)
            );
            if (ok && ret.length >= 5 * 32) {
                (, , , uint256 updatedAt, ) = abi.decode(
                    ret, (uint80, int256, uint256, uint256, uint80)
                );
                require(updatedAt > 0, "Pool: oracle has no price");
                require(
                    block.timestamp - updatedAt <= POOL_MAX_STALENESS,
                    "Pool: oracle stale"
                );
            }
            uint256 collReal = (userCollateral[user][asset] * r.liquidityIndex) / RAY;
            uint256 debtReal = (userDebt[user][asset]       * r.borrowIndex)    / RAY;
            uint256 collUsd  = (collReal * price) / 1e18;
            uint256 debtUsd  = (debtReal * price) / 1e18;
            // Per-asset liquidationThreshold weighting.
            weightedColl += (collUsd * r.liquidationThreshold) / 10000;
            totalDebtUsd += debtUsd;
        }
        if (totalDebtUsd == 0) return type(uint256).max;
        return (weightedColl * 1e18) / totalDebtUsd;
    }

    function _getActiveReserve(address asset) internal view returns (ReserveData storage r) {
        r = reserves[asset];
        require(r.active, "Pool: reserve not active");
    }

    /// @dev Accrue interest by advancing liquidityIndex and borrowIndex.
    ///      Borrow side uses linear approximation (interest-on-interest is
    ///      small for short Δt — Aave uses the same approximation per-block).
    /// SEC-2026-05-09 Pass-19 (Tier-2 #2): cap interest accrual at
    /// 100% APR. Pre-fix, a misconfigured `setRateConfig` could push
    /// `borrowRate` above 100% APR (the kinked model permits up to
    /// `slope1 + slope2` ≈ 79% by default but governance can set
    /// arbitrary slopes), and a long `elapsed` would compound that
    /// catastrophically. We enforce a hard `MAX_RATE_PER_SEC` ceiling
    /// = `RAY / SECONDS_PER_YEAR` (= 1.0 ray per year per ray of
    /// principal, the linear 100% APR rate) BEFORE multiplying by
    /// elapsed. Defense-in-depth: also cap `elapsed` at 30 days so
    /// a multi-year-stale reserve catches up across multiple calls
    /// instead of compounding 4× in a single call. Both caps are
    /// no-ops in normal operation.
    uint256 internal constant MAX_INTEREST_ELAPSED = 30 days;
    /// 100% APR ceiling in ray-per-second.
    uint256 internal constant MAX_RATE_PER_SEC = RAY / SECONDS_PER_YEAR;

    function _updateState(ReserveData storage reserve) internal {
        uint256 elapsed = block.timestamp - reserve.lastUpdateTimestamp;
        if (elapsed == 0) return;
        if (elapsed > MAX_INTEREST_ELAPSED) elapsed = MAX_INTEREST_ELAPSED;

        uint256 borrowIdx = reserve.borrowIndex;
        uint256 liqIdx    = reserve.liquidityIndex;

        if (reserve.totalBorrowed > 0) {
            uint256 borrowRatePerSec = uint256(reserve.borrowRate) / SECONDS_PER_YEAR;
            // Pass-19 Tier-2 #2: enforce 100% APR ceiling on borrow rate.
            if (borrowRatePerSec > MAX_RATE_PER_SEC) borrowRatePerSec = MAX_RATE_PER_SEC;
            uint256 borrowAccr       = borrowRatePerSec * elapsed; // ray
            borrowIdx = borrowIdx + (borrowIdx * borrowAccr) / RAY;

            uint256 liqRatePerSec    = uint256(reserve.liquidityRate) / SECONDS_PER_YEAR;
            // Pass-19 Tier-2 #2: same ceiling on supply side.
            if (liqRatePerSec > MAX_RATE_PER_SEC) liqRatePerSec = MAX_RATE_PER_SEC;
            uint256 liqAccr          = liqRatePerSec * elapsed;
            liqIdx = liqIdx + (liqIdx * liqAccr) / RAY;
        }

        reserve.borrowIndex          = uint128(borrowIdx);
        reserve.liquidityIndex       = uint128(liqIdx);
        // SEC-2026-05-09 Pass-19 (Tier-2 #2 follow-up): advance by the
        // CLAMPED `elapsed` (not block.timestamp) so a >30-day-stale
        // reserve catches up across multiple calls instead of silently
        // forfeiting the interest beyond the 30-day window. The next
        // touch will accrue the next chunk; eventually the timestamp
        // converges to block.timestamp.
        reserve.lastUpdateTimestamp  = uint40(reserve.lastUpdateTimestamp + elapsed);

        emit ReserveDataUpdated(
            reserve.asset, reserve.liquidityRate, reserve.borrowRate, liqIdx, borrowIdx
        );
    }

    /// @dev Recompute borrow/liquidity rates from the kinked rate model.
    function _updateInterestRates(ReserveData storage reserve) internal {
        InterestRateConfig memory cfg = rateConfig[reserve.asset];
        // Default config if none set: 0% base, 4% slope1, 75% slope2, 80% optimal.
        if (cfg.optimalUtilization == 0) {
            cfg = InterestRateConfig({
                baseBorrowRate: 0,
                slope1: uint128(4 * RAY / 100),
                slope2: uint128(75 * RAY / 100),
                optimalUtilization: 8000
            });
        }

        uint256 supReal = (uint256(reserve.totalSupplied) * reserve.liquidityIndex) / RAY;
        uint256 borReal = (uint256(reserve.totalBorrowed) * reserve.borrowIndex)    / RAY;
        uint256 utilBps = supReal == 0 ? 0 : (borReal * 10000) / supReal;
        if (utilBps > 10000) utilBps = 10000;

        uint256 borrowRate;
        if (utilBps <= cfg.optimalUtilization) {
            borrowRate = cfg.baseBorrowRate
                + (uint256(cfg.slope1) * utilBps) / cfg.optimalUtilization;
        } else {
            uint256 excess = utilBps - cfg.optimalUtilization;
            uint256 maxExcess = 10000 - cfg.optimalUtilization;
            borrowRate = cfg.baseBorrowRate + cfg.slope1
                + (uint256(cfg.slope2) * excess) / maxExcess;
        }

        // supplyRate = borrowRate * util * (1 - reserveFactor)
        uint16 rf = reserve.reserveFactor == 0 ? DEFAULT_RESERVE_FACTOR : reserve.reserveFactor;
        uint256 liquidityRate = (borrowRate * utilBps * (10000 - rf)) / (10000 * 10000);

        reserve.borrowRate    = uint128(borrowRate);
        reserve.liquidityRate = uint128(liquidityRate);
    }

    // ─── Admin ────────────────────────────────────────────────────────────

    function addReserve(
        address asset,
        uint16  ltv,
        uint16  liqThreshold,
        uint16  liqBonus,
        uint16  reserveFactor,
        uint8   decimals
    ) external onlyAdmin {
        require(asset != address(0), "Pool: zero asset");
        require(reserves[asset].asset == address(0), "Pool: reserve exists");
        require(ltv <= 9000, "Pool: ltv too high");                 // ≤ 90%
        require(liqThreshold >= ltv && liqThreshold <= 9500, "Pool: bad threshold");
        require(liqBonus >= 10000 && liqBonus <= 11500, "Pool: bad bonus"); // 0–15%
        require(reserveFactor <= 5000, "Pool: rf too high");        // ≤ 50%

        reserves[asset] = ReserveData({
            asset: asset, zToken: address(0), debtToken: address(0),
            totalSupplied: 0, totalBorrowed: 0,
            liquidityRate: 0, borrowRate: 0,
            liquidityIndex: uint128(RAY), borrowIndex: uint128(RAY),
            lastUpdateTimestamp: uint40(block.timestamp),
            ltv: ltv, liquidationThreshold: liqThreshold,
            liquidationBonus: liqBonus, reserveFactor: reserveFactor,
            decimals: decimals,
            active: true, borrowEnabled: true, flashLoanEnabled: true
        });
        reserveList.push(asset);
    }

    function setRateConfig(
        address asset, uint128 baseRate, uint128 slope1, uint128 slope2, uint16 optimalUtil
    ) external onlyAdmin {
        require(reserves[asset].asset != address(0), "Pool: unknown asset");
        require(optimalUtil > 0 && optimalUtil <= 9500, "Pool: bad optimal");
        rateConfig[asset] = InterestRateConfig({
            baseBorrowRate: baseRate, slope1: slope1, slope2: slope2,
            optimalUtilization: optimalUtil
        });
    }

    function setReserveFlags(
        address asset, bool active_, bool borrowEnabled_, bool flashEnabled_
    ) external onlyAdmin {
        ReserveData storage r = reserves[asset];
        require(r.asset != address(0), "Pool: unknown asset");
        r.active           = active_;
        r.borrowEnabled    = borrowEnabled_;
        r.flashLoanEnabled = flashEnabled_;
    }

    // ─── EIP-165 supportsInterface (S21) ───────────────────────────────────
    //
    // Conservative claim: EIP-165 itself only.
    //
    // Why NOT also claim IZbxLending:
    //   The current ZbxLendingPool ABI uses Aave-style names (`supply`,
    //   `getReserveData`, `balanceOfSupplied`, `balanceOfDebt`) that DO
    //   NOT match the IZbxLending interface (which expects `deposit`,
    //   `getReserve`, `getUserBalance`, `getBorrowBalance`,
    //   `getUtilization`). Claiming `type(IZbxLending).interfaceId` here
    //   would be a FALSE EIP-165 advertisement.
    //
    // Tracked as `S21-FOLLOWUP-LENDING-INTERFACE-RECONCILIATION`:
    //   either rename the Aave-style functions to IZbxLending names (with
    //   one release of deprecated aliases for ABI-stability), OR rewrite
    //   IZbxLending to match the Aave-style ABI. Pick before the IZbxLending
    //   interfaceId is added to this claim.
    function supportsInterface(bytes4 interfaceId) external pure returns (bool) {
        return interfaceId == 0x01ffc9a7;   // EIP-165 itself
    }

    // NOTE: Single-step `transferOwnership(newOwner)` removed. The 2-step
    // `transferOwnership(newOwner)` + `acceptOwnership()` flow inherited
    // from Governable replaces it — and prevents accidental transfer to a
    // wrong / un-controlled address (a classic single-step ownership bug).
}
