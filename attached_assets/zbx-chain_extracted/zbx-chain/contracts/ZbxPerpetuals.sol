// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import "./libraries/SafeERC20.sol";
import "./libraries/ReentrancyGuard.sol";

/// @title  ZbxPerpetuals v5 — Multi-Market, 200× Leverage, Liq-Price Trigger
/// @author Zebvix Technologies Pvt Ltd
///
/// @notice Full-featured perpetual DEX supporting UNLIMITED trading pairs.
///         Each market (coin) has its own oracle, leverage cap, open interest
///         accounting, and 8-hour funding rate.  All v4 features retained:
///
///         200× MAX LEVERAGE
///           Global cap raised to 200× (was 20×).
///           Per-market maxLeverage set by owner — can be up to 200×.
///           Higher leverage = tighter liquidation price distance from entry.
///
///         LIQUIDATION PRICE
///           liquidationPrice(positionId) — returns the exact oracle price at
///           which an isolated position becomes liquidatable (10% maint margin).
///           liquidate(positionId) — keeper calls to execute liquidation once
///           mark price crosses the liq price.  Earns 1% bounty.
///           crossLiquidationThreshold(trader) — cross account liq threshold.
///
///         MULTI-MARKET
///           Owner adds any ERC-20-priced coin via addMarket().
///           Each market identified by a uint256 marketId (0-indexed).
///           Markets can be paused (setMarketActive) without affecting others.
///           Per-market: oracle, symbol, maxLeverage, totalLongOI, totalShortOI,
///                       cumulativeFunding, lastFundingUpdate.
///
///         MARGIN MODES
///           Isolated — per-position collateral (liquidation scope: one position).
///           Cross    — shared account balance across all markets for one trader.
///                      Cross equity = balance + unrealised PnL across ALL markets.
///
///         RISK PARAMETERS (per market, with global defaults)
///           10% maintenance margin (MAINTENANCE_MARGIN_BPS = 1000)
///           Per-market maxLeverage (up to global MAX_LEVERAGE = 20)
///
///         ORDERS
///           Stop Loss / Take Profit (set at open or anytime)
///           Trailing Stop Loss (keeper-ratcheted, per position)
///           Keeper trigger (0.05% bounty)
///
///         OTHER
///           8-hour funding per market
///           addCollateral (isolated)
///           partialClose (both modes)
///           healthBps view (isolated)
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:module     DeFi / Perpetuals v4 (ZEP-034 rev4)

interface IOracle {
    function latestAnswer() external view returns (int256);
    function decimals()     external view returns (uint8);
    /// @dev Chainlink-compatible — used for staleness check (SEC-2026-05-09).
    function latestRoundData() external view returns (
        uint80 roundId, int256 answer, uint256 startedAt, uint256 updatedAt, uint80 answeredInRound
    );
}

contract ZbxPerpetuals is ReentrancyGuard {
    using SafeERC20 for IERC20Minimal;

    // ─── Errors ───────────────────────────────────────────────────────────

    error ZeroAmount();
    error LeverageTooHigh();
    error MarketNotFound();
    error MarketInactive();
    error PositionNotFound();
    error NotPositionOwner();
    error NotLiquidatable();
    error AlreadyLiquidated();
    error SLNotTriggered();
    error TPNotTriggered();
    error NeitherTriggered();
    error InvalidSLPrice();
    error InvalidTPPrice();
    error InvalidBps();
    error TrailNotFavourable();
    error InsufficientCrossMargin();
    error CrossWithdrawTooLarge();
    error NotIsolatedPosition();
    error NotOwner();
    error InvalidOracle();
    error StaleOracle();

    // ─── Events ───────────────────────────────────────────────────────────

    event MarketAdded(uint256 indexed marketId, string symbol, address oracle, uint256 maxLeverage);
    event MarketUpdated(uint256 indexed marketId, address oracle, bool active, uint256 maxLeverage);
    event PositionOpened(
        uint256 indexed positionId,
        uint256 indexed marketId,
        address indexed trader,
        bool    isLong,
        bool    isCross,
        uint256 size,
        uint256 leverage,
        uint256 entryPrice
    );
    event PositionClosed(
        uint256 indexed positionId,
        uint256 indexed marketId,
        address indexed trader,
        int256  netPnl,
        uint256 exitPrice,
        string  reason
    );
    event PositionLiquidated(uint256 indexed positionId, address indexed liquidator, uint256 exitPrice);
    event CrossLiquidated(address indexed trader, address indexed liquidator, uint256 positionCount);
    event CrossDeposit(address indexed trader, uint256 amount);
    event CrossWithdraw(address indexed trader, uint256 amount);
    event StopLossSet(uint256 indexed positionId, uint256 price);
    event TakeProfitSet(uint256 indexed positionId, uint256 price);
    event StopLossTriggered(uint256 indexed positionId, address indexed keeper, uint256 price);
    event TakeProfitTriggered(uint256 indexed positionId, address indexed keeper, uint256 price);
    event TrailingStopSet(uint256 indexed positionId, uint256 trailBps, uint256 initialSL);
    event TrailingStopUpdated(uint256 indexed positionId, uint256 newSL, uint256 markPrice);
    event CollateralAdded(uint256 indexed positionId, uint256 amount);
    event PartialClose(uint256 indexed positionId, uint256 closedBps, int256 pnl, uint256 exitPrice);
    event FundingRateUpdated(uint256 indexed marketId, int256 rate, uint256 nextFundingAt);
    event FeeCollected(uint256 amount);

    // ─── Constants ────────────────────────────────────────────────────────

    uint256 public constant MAX_LEVERAGE           = 200;   // 200× global cap
    uint256 public constant MAINTENANCE_MARGIN_BPS = 1000;  // 10%
    uint256 public constant PROTOCOL_FEE_BPS       = 10;    // 0.10%
    uint256 public constant KEEPER_BOUNTY_BPS      = 5;     // 0.05%
    uint256 public constant LIQUIDATION_BOUNTY_BPS = 100;   // 1.00%
    uint256 public constant FUNDING_INTERVAL       = 8 hours;
    uint256 public constant FUNDING_RATE_SCALE     = 1e10;
    uint256 public constant MAX_TRAIL_BPS          = 5000;  // 50%
    uint256 public constant MAX_ORACLE_DELAY       = 1 hours; // SEC-2026-05-09 staleness

    // ─── Types ────────────────────────────────────────────────────────────

    struct Market {
        string  symbol;          // e.g. "BTC", "ETH", "ZBX"
        address oracle;          // Chainlink-compatible price feed
        bool    active;
        uint256 maxLeverage;     // per-market cap (≤ MAX_LEVERAGE)
        // Per-market OI
        uint256 totalLongOI;
        uint256 totalShortOI;
        // Per-market funding
        int256  cumulativeFunding;
        uint256 lastFundingUpdate;
    }

    struct Position {
        address trader;
        uint256 marketId;
        bool    isLong;
        bool    isCross;
        uint256 collateral;      // isolated: margin deposited. cross: 0
        uint256 size;            // notional
        uint256 entryPrice;
        int256  fundingEntryRate;

        uint256 stopLoss;
        uint256 takeProfit;
        uint256 trailBps;
        uint256 trailPeak;

        bool    closed;
        // SEC-2026-05-09 — per-position IM share for cross accounts so that
        // _executeClose / partialClose release the right amount instead of
        // assuming size/MAX_LEVERAGE (which only matched 200x positions).
        uint256 initialMargin;
    }

    struct CrossAccount {
        uint256   balance;
        uint256   initialMargin;
        uint256[] posIds;
    }

    // ─── State ────────────────────────────────────────────────────────────

    address public owner;
    address public collateralToken;
    address public treasury;

    // Markets
    mapping(uint256 => Market) public markets;
    uint256 public marketCount;

    // Positions
    mapping(uint256 => Position) public positions;
    uint256 public nextPositionId;

    // Cross accounts
    mapping(address => CrossAccount) internal _cross;

    uint256 public protocolFeeBalance;

    // ─── Modifier ─────────────────────────────────────────────────────────

    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address collateralToken_, address treasury_) {
        require(collateralToken_ != address(0) && treasury_ != address(0), "Perps: zero address");
        owner           = msg.sender;
        collateralToken = collateralToken_;
        treasury        = treasury_;
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  MARKET MANAGEMENT (owner)
    // ═══════════════════════════════════════════════════════════════════════

    /// @notice Add a new trading pair.
    /// @param  oracle_      Chainlink-compatible price feed for this coin.
    /// @param  symbol_      Human-readable ticker (e.g. "BTC", "ZBX").
    /// @param  maxLeverage_ Max leverage allowed on this market (1–20).
    /// @return marketId     ID to use in openPosition().
    function addMarket(
        address oracle_,
        string  calldata symbol_,
        uint256 maxLeverage_
    ) external onlyOwner returns (uint256 marketId) {
        if (oracle_ == address(0))                          revert InvalidOracle();
        if (maxLeverage_ == 0 || maxLeverage_ > MAX_LEVERAGE) revert LeverageTooHigh();

        marketId = marketCount++;
        markets[marketId] = Market({
            symbol:            symbol_,
            oracle:            oracle_,
            active:            true,
            maxLeverage:       maxLeverage_,
            totalLongOI:       0,
            totalShortOI:      0,
            cumulativeFunding: 0,
            lastFundingUpdate: block.timestamp
        });

        emit MarketAdded(marketId, symbol_, oracle_, maxLeverage_);
    }

    /// @notice Update market oracle / active flag / leverage cap.
    function updateMarket(
        uint256 marketId,
        address oracle_,
        bool    active_,
        uint256 maxLeverage_
    ) external onlyOwner {
        if (marketId >= marketCount) revert MarketNotFound();
        if (oracle_ == address(0))   revert InvalidOracle();
        if (maxLeverage_ == 0 || maxLeverage_ > MAX_LEVERAGE) revert LeverageTooHigh();
        Market storage m = markets[marketId];
        m.oracle      = oracle_;
        m.active      = active_;
        m.maxLeverage = maxLeverage_;
        emit MarketUpdated(marketId, oracle_, active_, maxLeverage_);
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  CROSS MARGIN — Deposit / Withdraw
    // ═══════════════════════════════════════════════════════════════════════

    function depositCross(uint256 amount) external nonReentrant {
        if (amount == 0) revert ZeroAmount();
        // SEC-2026-05-09 — SafeERC20 (USDT-compat) + nonReentrant.
        IERC20Minimal(collateralToken).safeTransferFrom(msg.sender, address(this), amount);
        _cross[msg.sender].balance += amount;
        emit CrossDeposit(msg.sender, amount);
    }

    function withdrawCross(uint256 amount) external nonReentrant {
        if (amount == 0) revert ZeroAmount();
        uint256 free = _freeCrossMargin(msg.sender);
        if (amount > free) revert CrossWithdrawTooLarge();
        _cross[msg.sender].balance -= amount;
        IERC20Minimal(collateralToken).safeTransfer(msg.sender, amount); // SEC-2026-05-09
        emit CrossWithdraw(msg.sender, amount);
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  OPEN POSITION
    // ═══════════════════════════════════════════════════════════════════════

    /// @notice Open a position on any listed market.
    /// @param  marketId   Which coin to trade (from addMarket).
    /// @param  isLong     true = long.
    /// @param  collateral Margin amount (isolated) or initial margin (cross from balance).
    /// @param  leverage   Multiplier (1 – market.maxLeverage).
    /// @param  isCross    true = use cross account.
    /// @param  slPrice    Stop-loss price (0 = none).
    /// @param  tpPrice    Take-profit price (0 = none).
    function openPosition(
        uint256 marketId,
        bool    isLong,
        uint256 collateral,
        uint256 leverage,
        bool    isCross,
        uint256 slPrice,
        uint256 tpPrice
    ) external nonReentrant returns (uint256 positionId) {
        if (marketId >= marketCount) revert MarketNotFound();
        Market storage m = markets[marketId];
        if (!m.active) revert MarketInactive();
        if (collateral == 0) revert ZeroAmount();
        if (leverage == 0 || leverage > m.maxLeverage) revert LeverageTooHigh();

        _updateFunding(marketId);
        uint256 price = _marketPrice(m);

        if (slPrice != 0) _validateSL(isLong, price, slPrice);
        if (tpPrice != 0) _validateTP(isLong, price, tpPrice);

        uint256 fee    = (collateral * PROTOCOL_FEE_BPS) / 10_000;
        uint256 colNet = collateral - fee;
        uint256 size   = colNet * leverage;

        if (isCross) {
            CrossAccount storage ca = _cross[msg.sender];
            // SEC-2026-05-09 — pre-trade balance must cover BOTH the new
            // collateral lockup AND the post-trade maintenance margin (own
            // existing positions PLUS this new one). Previously the new
            // position's maint margin was excluded → an account could open
            // a position that was instantly liquidatable.
            uint256 newMaint = (size * MAINTENANCE_MARGIN_BPS) / 10_000;
            uint256 needed   = collateral + _maintMarginForCross(msg.sender) + newMaint;
            if (ca.balance < needed) revert InsufficientCrossMargin();
            ca.balance       -= fee;
            ca.initialMargin += colNet;
            protocolFeeBalance += fee;
        } else {
            // SEC-2026-05-09 — SafeERC20 (USDT-compat).
            IERC20Minimal(collateralToken).safeTransferFrom(msg.sender, address(this), collateral);
            protocolFeeBalance += fee;
        }

        positionId = ++nextPositionId;
        positions[positionId] = Position({
            trader:           msg.sender,
            marketId:         marketId,
            isLong:           isLong,
            isCross:          isCross,
            collateral:       isCross ? 0 : colNet,
            size:             size,
            entryPrice:       price,
            fundingEntryRate: m.cumulativeFunding,
            stopLoss:         slPrice,
            takeProfit:       tpPrice,
            trailBps:         0,
            trailPeak:        price,
            closed:           false,
            initialMargin:    colNet  // SEC-2026-05-09 — per-position IM share
        });

        if (isCross) _cross[msg.sender].posIds.push(positionId);

        if (isLong) m.totalLongOI  += size;
        else        m.totalShortOI += size;

        if (slPrice != 0) emit StopLossSet(positionId, slPrice);
        if (tpPrice != 0) emit TakeProfitSet(positionId, tpPrice);
        emit PositionOpened(positionId, marketId, msg.sender, isLong, isCross, size, leverage, price);
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  CLOSE
    // ═══════════════════════════════════════════════════════════════════════

    function closePosition(uint256 positionId) external nonReentrant {
        Position storage p = positions[positionId];
        if (p.trader == address(0)) revert PositionNotFound();
        if (p.trader != msg.sender) revert NotPositionOwner();
        if (p.closed)               revert AlreadyLiquidated();
        _updateFunding(p.marketId);
        _executeClose(positionId, p, _marketPrice(markets[p.marketId]), "manual");
    }

    function partialClose(uint256 positionId, uint256 closeBps) external nonReentrant {
        if (closeBps == 0 || closeBps > 10_000) revert InvalidBps();
        Position storage p = positions[positionId];
        if (p.trader == address(0)) revert PositionNotFound();
        if (p.trader != msg.sender) revert NotPositionOwner();
        if (p.closed)               revert AlreadyLiquidated();
        if (closeBps == 10_000) {
            _updateFunding(p.marketId);
            _executeClose(positionId, p, _marketPrice(markets[p.marketId]), "manual");
            return;
        }

        _updateFunding(p.marketId);
        Market storage m  = markets[p.marketId];
        uint256 exitPrice = _marketPrice(m);
        uint256 closeSize = (p.size      * closeBps) / 10_000;
        uint256 closeCol  = p.isCross ? 0 : (p.collateral * closeBps) / 10_000;

        int256 pnl    = _pnlFor(p.isLong, p.entryPrice, exitPrice, closeSize);
        int256 fund   = _fundingForSize(m, p, closeSize);
        int256 netPnl = pnl - fund;

        p.size -= closeSize;
        if (!p.isCross) p.collateral -= closeCol;
        if (p.isLong) m.totalLongOI  -= closeSize;
        else          m.totalShortOI -= closeSize;

        if (p.isCross) {
            _settleCrossPnl(msg.sender, netPnl);
            // SEC-2026-05-09 — release IM proportional to THIS position's
            // recorded share, not the global `initialMargin` total. Was
            // releasing wrong amount when account had multiple positions.
            uint256 imRel = (p.initialMargin * closeBps) / 10_000;
            CrossAccount storage caP = _cross[msg.sender];
            if (imRel <= caP.initialMargin) caP.initialMargin -= imRel;
            else caP.initialMargin = 0;
            p.initialMargin -= imRel;
        } else {
            uint256 payout;
            if (netPnl >= 0) payout = closeCol + uint256(netPnl);
            else {
                uint256 loss = uint256(-netPnl);
                payout = loss >= closeCol ? 0 : closeCol - loss;
            }
            uint256 fee = (payout * PROTOCOL_FEE_BPS) / 10_000;
            protocolFeeBalance += fee;
            if (payout > fee) IERC20Minimal(collateralToken).safeTransfer(msg.sender, payout - fee); // SEC-2026-05-09
        }
        emit PartialClose(positionId, closeBps, netPnl, exitPrice);
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  SL / TP — Set
    // ═══════════════════════════════════════════════════════════════════════

    function setStopLoss(uint256 positionId, uint256 slPrice) external {
        Position storage p = positions[positionId];
        if (p.trader != msg.sender) revert NotPositionOwner();
        if (p.closed)               revert AlreadyLiquidated();
        if (slPrice != 0) _validateSL(p.isLong, _marketPrice(markets[p.marketId]), slPrice);
        p.stopLoss = slPrice;
        emit StopLossSet(positionId, slPrice);
    }

    function setTakeProfit(uint256 positionId, uint256 tpPrice) external {
        Position storage p = positions[positionId];
        if (p.trader != msg.sender) revert NotPositionOwner();
        if (p.closed)               revert AlreadyLiquidated();
        if (tpPrice != 0) _validateTP(p.isLong, _marketPrice(markets[p.marketId]), tpPrice);
        p.takeProfit = tpPrice;
        emit TakeProfitSet(positionId, tpPrice);
    }

    function setTrailingStop(uint256 positionId, uint256 trailBps) external {
        if (trailBps == 0 || trailBps > MAX_TRAIL_BPS) revert InvalidBps();
        Position storage p = positions[positionId];
        if (p.trader != msg.sender) revert NotPositionOwner();
        if (p.closed)               revert AlreadyLiquidated();
        uint256 mark = _marketPrice(markets[p.marketId]);
        p.trailBps   = trailBps;
        p.trailPeak  = mark;
        uint256 initSL = p.isLong
            ? (mark * (10_000 - trailBps)) / 10_000
            : (mark * (10_000 + trailBps)) / 10_000;
        p.stopLoss = initSL;
        emit TrailingStopSet(positionId, trailBps, initSL);
        emit StopLossSet(positionId, initSL);
    }

    function updateTrailingStop(uint256 positionId) external {
        Position storage p = positions[positionId];
        if (p.trader == address(0)) revert PositionNotFound();
        if (p.closed || p.trailBps == 0) revert InvalidBps();
        uint256 mark = _marketPrice(markets[p.marketId]);
        bool improved;
        if (p.isLong && mark > p.trailPeak) {
            p.trailPeak = mark;
            p.stopLoss  = (mark * (10_000 - p.trailBps)) / 10_000;
            improved    = true;
        } else if (!p.isLong && mark < p.trailPeak) {
            p.trailPeak = mark;
            p.stopLoss  = (mark * (10_000 + p.trailBps)) / 10_000;
            improved    = true;
        }
        if (!improved) revert TrailNotFavourable();
        emit TrailingStopUpdated(positionId, p.stopLoss, mark);
        emit StopLossSet(positionId, p.stopLoss);
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  TRIGGER (keeper)
    // ═══════════════════════════════════════════════════════════════════════

    function triggerOrder(uint256 positionId) external nonReentrant {
        Position storage p = positions[positionId];
        if (p.trader == address(0)) revert PositionNotFound();
        if (p.closed)               revert AlreadyLiquidated();
        _updateFunding(p.marketId);
        uint256 mark = _marketPrice(markets[p.marketId]);
        bool slHit = _slHit(p, mark);
        bool tpHit = _tpHit(p, mark);
        if (!slHit && !tpHit) revert NeitherTriggered();
        _payKeeperBounty(positionId, p);
        if (slHit) emit StopLossTriggered(positionId, msg.sender, mark);
        else        emit TakeProfitTriggered(positionId, msg.sender, mark);
        _executeClose(positionId, p, mark, slHit ? "stop_loss" : "take_profit");
    }

    function triggerStopLoss(uint256 positionId) external nonReentrant {
        Position storage p = positions[positionId];
        if (p.trader == address(0)) revert PositionNotFound();
        if (p.closed)               revert AlreadyLiquidated();
        _updateFunding(p.marketId);
        uint256 mark = _marketPrice(markets[p.marketId]);
        if (!_slHit(p, mark)) revert SLNotTriggered();
        _payKeeperBounty(positionId, p);
        emit StopLossTriggered(positionId, msg.sender, mark);
        _executeClose(positionId, p, mark, "stop_loss");
    }

    function triggerTakeProfit(uint256 positionId) external nonReentrant {
        Position storage p = positions[positionId];
        if (p.trader == address(0)) revert PositionNotFound();
        if (p.closed)               revert AlreadyLiquidated();
        _updateFunding(p.marketId);
        uint256 mark = _marketPrice(markets[p.marketId]);
        if (!_tpHit(p, mark)) revert TPNotTriggered();
        _payKeeperBounty(positionId, p);
        emit TakeProfitTriggered(positionId, msg.sender, mark);
        _executeClose(positionId, p, mark, "take_profit");
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  ADD COLLATERAL (Isolated only)
    // ═══════════════════════════════════════════════════════════════════════

    function addCollateral(uint256 positionId, uint256 amount) external nonReentrant {
        if (amount == 0) revert ZeroAmount();
        Position storage p = positions[positionId];
        if (p.trader == address(0)) revert PositionNotFound();
        if (p.closed)               revert AlreadyLiquidated();
        if (p.isCross)              revert NotIsolatedPosition();
        // SEC-2026-05-09 — SafeERC20.
        IERC20Minimal(collateralToken).safeTransferFrom(msg.sender, address(this), amount);
        p.collateral += amount;
        emit CollateralAdded(positionId, amount);
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  LIQUIDATION
    // ═══════════════════════════════════════════════════════════════════════

    function liquidate(uint256 positionId) external nonReentrant {
        Position storage p = positions[positionId];
        if (p.trader == address(0)) revert PositionNotFound();
        if (p.closed)               revert AlreadyLiquidated();
        if (p.isCross)              revert NotIsolatedPosition();
        _updateFunding(p.marketId);
        Market storage m  = markets[p.marketId];
        uint256 mark      = _marketPrice(m);
        if (!_isolatedLiquidatable(m, p, mark)) revert NotLiquidatable();

        if (p.isLong) m.totalLongOI  -= p.size;
        else          m.totalShortOI -= p.size;
        p.closed = true;

        uint256 bounty = (p.collateral * LIQUIDATION_BOUNTY_BPS) / 10_000;
        protocolFeeBalance += p.collateral > bounty ? p.collateral - bounty : 0;
        if (bounty > 0) IERC20Minimal(collateralToken).safeTransfer(msg.sender, bounty); // SEC-2026-05-09 (CEI + SafeERC20)
        emit PositionLiquidated(positionId, msg.sender, mark);
    }

    function liquidateCross(address trader) external nonReentrant {
        if (!_crossLiquidatable(trader)) revert NotLiquidatable();

        CrossAccount storage ca = _cross[trader];
        uint256[] memory posIds = ca.posIds;
        uint256 count;

        for (uint256 i; i < posIds.length; i++) {
            uint256 pid = posIds[i];
            Position storage p = positions[pid];
            if (p.closed) continue;
            _updateFunding(p.marketId);
            Market storage m = markets[p.marketId];
            if (p.isLong) m.totalLongOI  -= p.size;
            else          m.totalShortOI -= p.size;
            p.closed = true;
            count++;
            emit PositionLiquidated(pid, msg.sender, _marketPrice(m));
        }

        uint256 bounty = (ca.balance * LIQUIDATION_BOUNTY_BPS) / 10_000;
        // SEC-2026-05-09 — CEI: zero out state before transfer.
        if (bounty > 0 && bounty <= ca.balance) {
            ca.balance -= bounty;
        } else {
            bounty = 0;
        }
        protocolFeeBalance += ca.balance;
        ca.balance       = 0;
        ca.initialMargin = 0;
        delete ca.posIds;
        if (bounty > 0) IERC20Minimal(collateralToken).safeTransfer(msg.sender, bounty);

        emit CrossLiquidated(trader, msg.sender, count);
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  FUNDING (8-hour, per market)
    // ═══════════════════════════════════════════════════════════════════════

    function updateFunding(uint256 marketId) external {
        if (marketId >= marketCount) revert MarketNotFound();
        _updateFunding(marketId);
    }

    function _updateFunding(uint256 marketId) internal {
        Market storage m = markets[marketId];
        uint256 elapsed  = block.timestamp - m.lastFundingUpdate;
        if (elapsed < FUNDING_INTERVAL) return;
        uint256 intervals = elapsed / FUNDING_INTERVAL;
        m.lastFundingUpdate += intervals * FUNDING_INTERVAL;

        uint256 totalOI = m.totalLongOI + m.totalShortOI;
        int256 rate;
        if (totalOI > 0) {
            int256 lb = int256((m.totalLongOI  * 10_000) / totalOI);
            int256 sb = int256((m.totalShortOI * 10_000) / totalOI);
            rate = ((lb - sb) * int256(FUNDING_RATE_SCALE)) / 1_000_000;
        }
        m.cumulativeFunding += rate * int256(intervals);
        emit FundingRateUpdated(marketId, rate, m.lastFundingUpdate + FUNDING_INTERVAL);
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  ADMIN
    // ═══════════════════════════════════════════════════════════════════════

    function withdrawFees() external onlyOwner {
        uint256 amount = protocolFeeBalance;
        protocolFeeBalance = 0;
        IERC20Minimal(collateralToken).safeTransfer(treasury, amount); // SEC-2026-05-09
        emit FeeCollected(amount);
    }

    function transferOwnership(address newOwner) external onlyOwner {
        require(newOwner != address(0));
        owner = newOwner;
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  VIEW — Market
    // ═══════════════════════════════════════════════════════════════════════

    function getMarket(uint256 marketId) external view returns (
        string memory symbol,
        address oracle,
        bool    active,
        uint256 maxLeverage,
        uint256 totalLongOI,
        uint256 totalShortOI,
        int256  cumulativeFunding,
        uint256 nextFundingIn
    ) {
        if (marketId >= marketCount) revert MarketNotFound();
        Market storage m = markets[marketId];
        uint256 next = m.lastFundingUpdate + FUNDING_INTERVAL;
        return (
            m.symbol,
            m.oracle,
            m.active,
            m.maxLeverage,
            m.totalLongOI,
            m.totalShortOI,
            m.cumulativeFunding,
            block.timestamp >= next ? 0 : next - block.timestamp
        );
    }

    function markPrice(uint256 marketId) external view returns (uint256) {
        if (marketId >= marketCount) revert MarketNotFound();
        return _marketPrice(markets[marketId]);
    }

    function currentFundingRate(uint256 marketId) external view returns (int256) {
        if (marketId >= marketCount) revert MarketNotFound();
        Market storage m = markets[marketId];
        uint256 totalOI  = m.totalLongOI + m.totalShortOI;
        if (totalOI == 0) return 0;
        int256 lb = int256((m.totalLongOI  * 10_000) / totalOI);
        int256 sb = int256((m.totalShortOI * 10_000) / totalOI);
        return ((lb - sb) * int256(FUNDING_RATE_SCALE)) / 1_000_000;
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  VIEW — Position / Isolated
    // ═══════════════════════════════════════════════════════════════════════

    function unrealisedPnl(uint256 positionId) external view returns (int256) {
        Position storage p = positions[positionId];
        return _pnlFor(p.isLong, p.entryPrice, _marketPrice(markets[p.marketId]), p.size);
    }

    function healthBps(uint256 positionId) external view returns (uint256) {
        Position storage p = positions[positionId];
        if (p.closed || p.isCross || p.trader == address(0)) return 0;
        Market storage m = markets[p.marketId];
        uint256 mark  = _marketPrice(m);
        int256 pnl    = _pnlFor(p.isLong, p.entryPrice, mark, p.size);
        int256 fund   = _fundingForSize(m, p, p.size);
        int256 eq     = int256(p.collateral) + pnl - fund;
        if (eq <= 0) return 0;
        uint256 maint = (p.size * MAINTENANCE_MARGIN_BPS) / 10_000;
        if (uint256(eq) <= maint) return 0;
        uint256 h = (uint256(eq) * 10_000) / (p.collateral == 0 ? 1 : p.collateral);
        return h > 10_000 ? 10_000 : h;
    }

    function isLiquidatable(uint256 positionId) external view returns (bool) {
        Position storage p = positions[positionId];
        if (p.closed || p.isCross || p.trader == address(0)) return false;
        return _isolatedLiquidatable(markets[p.marketId], p, _marketPrice(markets[p.marketId]));
    }

    function isSLTriggered(uint256 positionId) external view returns (bool) {
        Position storage p = positions[positionId];
        return !p.closed && _slHit(p, _marketPrice(markets[p.marketId]));
    }

    function isTPTriggered(uint256 positionId) external view returns (bool) {
        Position storage p = positions[positionId];
        return !p.closed && _tpHit(p, _marketPrice(markets[p.marketId]));
    }

    /// @notice Exact oracle price at which this isolated position gets liquidated.
    ///
    ///         Formula (derived from equity = maintenance margin):
    ///
    ///           LONG:  liqPrice = entry + entry × (MM − col + funding) / size
    ///           SHORT: liqPrice = entry − entry × (MM − col + funding) / size
    ///
    ///         where MM = size × MAINTENANCE_MARGIN_BPS / 10000.
    ///
    ///         If funding is negative (shorts paying longs) the liq price moves
    ///         further away from entry — the position is safer.
    ///
    ///         Returns 0 if position is cross (no single liq price) or closed.
    function liquidationPrice(uint256 positionId) external view returns (uint256) {
        Position storage p = positions[positionId];
        if (p.closed || p.isCross || p.trader == address(0)) return 0;

        Market storage m  = markets[p.marketId];
        int256 funding    = _fundingForSize(m, p, p.size);
        int256 maintMargin = int256((p.size * MAINTENANCE_MARGIN_BPS) / 10_000);

        // numerator = MM - collateral + funding (signed)
        int256 numerator = maintMargin - int256(p.collateral) + funding;

        // delta = entry * numerator / size
        int256 entryI = int256(p.entryPrice);
        int256 sizeI  = int256(p.size);
        int256 delta  = (entryI * numerator) / sizeI;

        int256 liqPriceI;
        if (p.isLong) {
            liqPriceI = entryI + delta;   // LONG: liq below entry
        } else {
            liqPriceI = entryI - delta;   // SHORT: liq above entry
        }

        return liqPriceI > 0 ? uint256(liqPriceI) : 0;
    }

    /// @notice For cross accounts: the equity level (in collateral units) below
    ///         which the cross account becomes liquidatable.
    ///         = sum(size_i × 10%) across all open cross positions.
    ///         If crossEquity(trader) < crossLiquidationThreshold(trader) → liquidatable.
    function crossLiquidationThreshold(address trader) external view returns (uint256) {
        return _maintMarginForCross(trader);
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  VIEW — Cross
    // ═══════════════════════════════════════════════════════════════════════

    function crossBalance(address trader)      external view returns (uint256) { return _cross[trader].balance; }
    function crossPositionIds(address trader)  external view returns (uint256[] memory) { return _cross[trader].posIds; }
    function isCrossLiquidatable(address trader) external view returns (bool) { return _crossLiquidatable(trader); }

    function crossEquity(address trader) external view returns (int256) {
        return _crossEquity(trader);
    }

    function crossMaintMargin(address trader) external view returns (uint256) {
        return _maintMarginForCross(trader);
    }

    function freeCrossMargin(address trader) external view returns (uint256) {
        return _freeCrossMargin(trader);
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  INTERNAL
    // ═══════════════════════════════════════════════════════════════════════

    function _executeClose(
        uint256 positionId,
        Position storage p,
        uint256 exitPrice,
        string memory reason
    ) private {
        Market storage m = markets[p.marketId];
        int256 pnl    = _pnlFor(p.isLong, p.entryPrice, exitPrice, p.size);
        int256 fund   = _fundingForSize(m, p, p.size);
        int256 netPnl = pnl - fund;

        if (p.isLong) m.totalLongOI  -= p.size;
        else          m.totalShortOI -= p.size;
        p.closed = true;

        if (p.isCross) {
            _settleCrossPnl(p.trader, netPnl);
            // SEC-2026-05-09 — release the actually-locked IM share, not
            // size/MAX_LEVERAGE (which under-released for any position
            // opened below the global 200× cap).
            uint256 imRel = p.initialMargin;
            CrossAccount storage ca = _cross[p.trader];
            if (imRel <= ca.initialMargin) ca.initialMargin -= imRel;
            else ca.initialMargin = 0;
            p.initialMargin = 0;
            _removeCrossPosId(p.trader, positionId);
        } else {
            uint256 payout;
            if (netPnl >= 0) payout = p.collateral + uint256(netPnl);
            else {
                uint256 loss = uint256(-netPnl);
                payout = loss >= p.collateral ? 0 : p.collateral - loss;
            }
            uint256 fee = (payout * PROTOCOL_FEE_BPS) / 10_000;
            protocolFeeBalance += fee;
            if (payout > fee) IERC20Minimal(collateralToken).safeTransfer(p.trader, payout - fee); // SEC-2026-05-09
        }
        emit PositionClosed(positionId, p.marketId, p.trader, netPnl, exitPrice, reason);
    }

    function _settleCrossPnl(address trader, int256 netPnl) private {
        CrossAccount storage ca = _cross[trader];
        if (netPnl >= 0) {
            ca.balance += uint256(netPnl);
            uint256 fee = (uint256(netPnl) * PROTOCOL_FEE_BPS) / 10_000;
            if (fee <= ca.balance) { ca.balance -= fee; protocolFeeBalance += fee; }
        } else {
            uint256 loss = uint256(-netPnl);
            ca.balance = ca.balance > loss ? ca.balance - loss : 0;
        }
    }

    function _removeCrossPosId(address trader, uint256 posId) private {
        uint256[] storage arr = _cross[trader].posIds;
        for (uint256 i; i < arr.length; i++) {
            if (arr[i] == posId) { arr[i] = arr[arr.length - 1]; arr.pop(); break; }
        }
    }

    function _payKeeperBounty(uint256 /*posId*/, Position storage p) private {
        // SEC-2026-05-09 — CEI ordering + SafeERC20.
        if (p.isCross) {
            CrossAccount storage ca = _cross[p.trader];
            uint256 b = (ca.balance * KEEPER_BOUNTY_BPS) / 10_000;
            if (b > 0 && b <= ca.balance) {
                ca.balance -= b;
                IERC20Minimal(collateralToken).safeTransfer(msg.sender, b);
            }
        } else {
            uint256 b = (p.collateral * KEEPER_BOUNTY_BPS) / 10_000;
            if (b > 0 && b <= p.collateral) {
                p.collateral -= b;
                IERC20Minimal(collateralToken).safeTransfer(msg.sender, b);
            }
        }
    }

    function _crossEquity(address trader) private view returns (int256) {
        int256 eq = int256(_cross[trader].balance);
        uint256[] storage posIds = _cross[trader].posIds;
        for (uint256 i; i < posIds.length; i++) {
            Position storage p = positions[posIds[i]];
            if (p.closed) continue;
            Market storage m = markets[p.marketId];
            eq += _pnlFor(p.isLong, p.entryPrice, _marketPrice(m), p.size);
            eq -= _fundingForSize(m, p, p.size);
        }
        return eq;
    }

    function _maintMarginForCross(address trader) private view returns (uint256) {
        uint256 maint;
        uint256[] storage posIds = _cross[trader].posIds;
        for (uint256 i; i < posIds.length; i++) {
            Position storage p = positions[posIds[i]];
            if (!p.closed) maint += (p.size * MAINTENANCE_MARGIN_BPS) / 10_000;
        }
        return maint;
    }

    function _freeCrossMargin(address trader) private view returns (uint256) {
        int256 eq     = _crossEquity(trader);
        uint256 maint = _maintMarginForCross(trader);
        if (eq <= 0) return 0;
        uint256 ueq   = uint256(eq);
        return ueq > maint ? ueq - maint : 0;
    }

    function _crossLiquidatable(address trader) private view returns (bool) {
        if (_cross[trader].posIds.length == 0) return false;
        int256 eq     = _crossEquity(trader);
        uint256 maint = _maintMarginForCross(trader);
        return eq <= 0 || uint256(eq) < maint;
    }

    function _isolatedLiquidatable(Market storage m, Position storage p, uint256 mark)
        private view returns (bool)
    {
        int256 pnl  = _pnlFor(p.isLong, p.entryPrice, mark, p.size);
        int256 fund = _fundingForSize(m, p, p.size);
        int256 eq   = int256(p.collateral) + pnl - fund;
        if (eq <= 0) return true;
        return uint256(eq) < (p.size * MAINTENANCE_MARGIN_BPS) / 10_000;
    }

    function _marketPrice(Market storage m) private view returns (uint256) {
        // SEC-2026-05-09 — oracle staleness check via Chainlink-style
        // latestRoundData(); reverts if updatedAt is older than MAX_ORACLE_DELAY
        // or if the round id never advanced (`answeredInRound < roundId`).
        (uint80 roundId, int256 p, , uint256 updatedAt, uint80 answeredInRound)
            = IOracle(m.oracle).latestRoundData();
        if (p <= 0) revert InvalidOracle();
        if (updatedAt == 0 || answeredInRound < roundId) revert StaleOracle();
        if (block.timestamp - updatedAt > MAX_ORACLE_DELAY) revert StaleOracle();
        uint8 dec = IOracle(m.oracle).decimals();
        return dec < 18 ? uint256(p) * (10 ** (18 - dec)) : uint256(p);
    }

    function _pnlFor(bool isLong, uint256 entry, uint256 exit, uint256 size)
        internal pure returns (int256)
    {
        int256 delta = int256(exit) - int256(entry);
        if (!isLong) delta = -delta;
        return (delta * int256(size)) / int256(entry);
    }

    function _fundingForSize(Market storage m, Position storage p, uint256 size)
        internal view returns (int256)
    {
        int256 delta = m.cumulativeFunding - p.fundingEntryRate;
        if (!p.isLong) delta = -delta;
        return (delta * int256(size)) / int256(FUNDING_RATE_SCALE * 10_000);
    }

    function _slHit(Position storage p, uint256 mark) private view returns (bool) {
        if (p.stopLoss == 0) return false;
        return p.isLong ? mark <= p.stopLoss : mark >= p.stopLoss;
    }

    function _tpHit(Position storage p, uint256 mark) private view returns (bool) {
        if (p.takeProfit == 0) return false;
        return p.isLong ? mark >= p.takeProfit : mark <= p.takeProfit;
    }

    function _validateSL(bool isLong, uint256 mark, uint256 sl) internal pure {
        if (isLong  && sl >= mark) revert InvalidSLPrice();
        if (!isLong && sl <= mark) revert InvalidSLPrice();
    }

    function _validateTP(bool isLong, uint256 mark, uint256 tp) internal pure {
        if (isLong  && tp <= mark) revert InvalidTPPrice();
        if (!isLong && tp >= mark) revert InvalidTPPrice();
    }
}
