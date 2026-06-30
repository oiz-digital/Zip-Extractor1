// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import "./libraries/SafeERC20.sol";
import "./libraries/ReentrancyGuard.sol";

/// @title  ZbxDatedFutures — Fixed-expiry futures with oracle cash settlement
/// @author Zebvix Technologies Pvt Ltd
///
/// @notice Dated futures (as opposed to perpetuals) have a fixed expiry date.
///         At expiry, all open positions are cash-settled using the oracle's
///         settlement price — no physical delivery.
///
///         Differences from ZbxPerpetuals:
///           - Fixed expiry timestamp (no indefinite holding)
///           - No funding rate — position cost is baked into basis
///           - Oracle freezes settlement price at/after expiry
///           - Supports multiple concurrent markets (ZBX-JUN26, ZBX-DEC26…)
///
///         Architecture:
///           - Admin creates markets (assetId, collateralToken, expiry)
///           - Traders open/close positions before expiry
///           - At expiry: oracle sets settlement price
///           - After settlement: any address calls settle(positionId) to pay out
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:module     Trading / Dated Futures (ZEP-043)

interface ISettlementOracle {
    /// @notice Returns the settlement price for an asset at or after its expiry.
    ///         Returns 0 if not yet settled.
    function getSettlementPrice(bytes32 marketId) external view returns (uint256);
    function latestAnswer() external view returns (int256);
    function decimals() external view returns (uint8);
    /// @dev SEC-2026-05-09 — Chainlink-style staleness check.
    function latestRoundData() external view returns (
        uint80 roundId, int256 answer, uint256 startedAt, uint256 updatedAt, uint80 answeredInRound
    );
}

contract ZbxDatedFutures is ReentrancyGuard {
    using SafeERC20 for IERC20Minimal;

    // ─── Errors ───────────────────────────────────────────────────────────

    error MarketNotFound();
    error MarketExpired();
    error MarketNotExpired();
    error MarketNotSettled();
    error MarketAlreadySettled();
    error PositionNotFound();
    error NotPositionOwner();
    error PositionAlreadySettled();
    error LeverageTooHigh();
    error InsufficientMargin();
    error NotLiquidatable();
    error AlreadyLiquidated();
    error NotAdmin();
    error ZeroAmount();
    error StaleOracle();

    // ─── Events ───────────────────────────────────────────────────────────

    event MarketCreated(
        bytes32 indexed marketId,
        string  name,
        address oracle,
        address collateralToken,
        uint256 expiry,
        uint256 maxLeverage
    );
    event MarketSettled(bytes32 indexed marketId, uint256 settlementPrice);
    event PositionOpened(
        uint256 indexed positionId,
        bytes32 indexed marketId,
        address indexed trader,
        bool    isLong,
        uint256 collateral,
        uint256 leverage,
        uint256 size,
        uint256 entryPrice
    );
    event PositionClosed(
        uint256 indexed positionId,
        address indexed trader,
        int256  pnl,
        uint256 exitPrice
    );
    event PositionSettled(
        uint256 indexed positionId,
        address indexed trader,
        int256  pnl,
        uint256 settlementPrice
    );
    event PositionLiquidated(uint256 indexed positionId, address indexed liquidator);

    // ─── Constants ────────────────────────────────────────────────────────

    uint256 public constant MAX_LEVERAGE_CAP    = 50;    // hard cap: 50x
    uint256 public constant MAINTENANCE_MARGIN  = 400;   // 4.00% of size
    uint256 public constant PROTOCOL_FEE_BPS    = 10;    // 0.10%
    uint256 public constant LIQUIDATION_BONUS   = 150;   // 1.50% of collateral
    uint256 public constant MAX_ORACLE_DELAY    = 1 hours; // SEC-2026-05-09
    uint256 public constant SETTLEMENT_WINDOW   = 1 hours; // SEC-2026-05-09 Pass-3 v3

    // ─── Types ────────────────────────────────────────────────────────────

    struct Market {
        string  name;             // e.g. "ZBX-JUN26"
        address oracle;           // oracle for mark price + settlement
        address collateralToken;  // token used as margin (e.g. ZUSD)
        uint256 expiry;           // settlement timestamp
        uint256 maxLeverage;      // market-specific max leverage
        uint256 settlementPrice;  // 0 until settled
        bool    settled;
        uint256 totalLongOI;
        uint256 totalShortOI;
    }

    struct Position {
        bytes32 marketId;
        address trader;
        bool    isLong;
        uint256 collateral;
        uint256 size;          // notional in collateral units
        uint256 entryPrice;    // oracle price at open (18-decimal)
        bool    liquidated;
        bool    settled;
    }

    // ─── State ────────────────────────────────────────────────────────────

    address public admin;
    address public treasury;

    mapping(bytes32 => Market) public markets;
    mapping(uint256 => Position) public positions;
    uint256 public nextPositionId;
    uint256 public protocolFeeBalance;
    mapping(address => uint256) public feeBalanceByToken;

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address treasury_) {
        require(treasury_ != address(0), "DatedFutures: zero treasury");
        admin    = msg.sender;
        treasury = treasury_;
    }

    // ─── Market management ────────────────────────────────────────────────

    /// @notice Admin creates a new dated futures market.
    /// @param  marketId         Unique identifier (e.g. keccak256("ZBX-JUN26")).
    /// @param  name             Human-readable name.
    /// @param  oracle_          Price oracle address.
    /// @param  collateralToken  Margin token.
    /// @param  expiry           Unix timestamp when market expires.
    /// @param  maxLeverage      Market-specific leverage cap (≤ MAX_LEVERAGE_CAP).
    function createMarket(
        bytes32 marketId,
        string  calldata name,
        address oracle_,
        address collateralToken,
        uint256 expiry,
        uint256 maxLeverage
    ) external {
        if (msg.sender != admin)                   revert NotAdmin();
        if (oracle_ == address(0) || collateralToken == address(0)) revert ZeroAmount();
        if (expiry <= block.timestamp)             revert MarketExpired();
        if (maxLeverage == 0 || maxLeverage > MAX_LEVERAGE_CAP) revert LeverageTooHigh();
        require(markets[marketId].expiry == 0, "DatedFutures: market exists");

        markets[marketId] = Market({
            name:             name,
            oracle:           oracle_,
            collateralToken:  collateralToken,
            expiry:           expiry,
            maxLeverage:      maxLeverage,
            settlementPrice:  0,
            settled:          false,
            totalLongOI:      0,
            totalShortOI:     0
        });

        emit MarketCreated(marketId, name, oracle_, collateralToken, expiry, maxLeverage);
    }

    // ─── Settlement ───────────────────────────────────────────────────────

    /// @notice Anyone can trigger settlement after expiry.
    ///         Reads the oracle's final price and locks it.
    function settleMarket(bytes32 marketId) external {
        Market storage m = markets[marketId];
        if (m.expiry == 0)              revert MarketNotFound();
        if (block.timestamp < m.expiry) revert MarketNotExpired();
        if (m.settled)                  revert MarketAlreadySettled();

        // Try settlement oracle first, fallback to spot
        uint256 price = ISettlementOracle(m.oracle).getSettlementPrice(marketId);
        if (price == 0) {
            // SEC-2026-05-09 Pass-3 v3 — bounded post-expiry settlement
            // window. The fallback round must be finalized and its
            // `updatedAt` must lie in `[expiry, expiry + SETTLEMENT_WINDOW]`.
            // v2 used an unbounded window (any post-expiry round forever),
            // which preserved the time-selection economic-manipulation vector
            // when keepers could simply wait for a favorable print.
            (uint80 roundId, int256 rawPrice, , uint256 updatedAt, uint80 answeredInRound)
                = ISettlementOracle(m.oracle).latestRoundData();
            require(rawPrice > 0, "DatedFutures: invalid oracle price");
            if (updatedAt == 0 || answeredInRound < roundId) revert StaleOracle();
            if (updatedAt < m.expiry) revert StaleOracle();
            if (updatedAt > m.expiry + SETTLEMENT_WINDOW) revert StaleOracle();
            uint8 dec = ISettlementOracle(m.oracle).decimals();
            price = dec < 18
                ? uint256(rawPrice) * (10 ** (18 - dec))
                : uint256(rawPrice);
        }

        m.settlementPrice = price;
        m.settled         = true;

        emit MarketSettled(marketId, price);
    }

    // ─── Open position ────────────────────────────────────────────────────

    /// @notice Open a new position in a dated futures market.
    /// @param  marketId   Target market.
    /// @param  isLong     True = long (profit if price rises).
    /// @param  collateral Margin posted.
    /// @param  leverage   1 to market.maxLeverage.
    function openPosition(
        bytes32 marketId,
        bool    isLong,
        uint256 collateral,
        uint256 leverage
    ) external nonReentrant returns (uint256 positionId) {
        Market storage m = markets[marketId];
        if (m.expiry == 0)                                  revert MarketNotFound();
        if (block.timestamp >= m.expiry)                    revert MarketExpired();
        if (collateral == 0)                                revert ZeroAmount();
        if (leverage == 0 || leverage > m.maxLeverage)     revert LeverageTooHigh();

        uint256 fee = (collateral * PROTOCOL_FEE_BPS) / 10_000;
        uint256 col = collateral - fee;
        uint256 size = col * leverage;

        // SEC-2026-05-09 — SafeERC20 (USDT-compat).
        IERC20Minimal(m.collateralToken).safeTransferFrom(msg.sender, address(this), collateral);

        feeBalanceByToken[m.collateralToken] += fee;

        uint256 entryPrice = _markPrice(m.oracle);

        positionId = ++nextPositionId;
        positions[positionId] = Position({
            marketId:   marketId,
            trader:     msg.sender,
            isLong:     isLong,
            collateral: col,
            size:       size,
            entryPrice: entryPrice,
            liquidated: false,
            settled:    false
        });

        if (isLong) m.totalLongOI  += size;
        else        m.totalShortOI += size;

        emit PositionOpened(positionId, marketId, msg.sender, isLong, col, leverage, size, entryPrice);
    }

    // ─── Close position (before expiry) ───────────────────────────────────

    /// @notice Close position at current mark price (before expiry).
    function closePosition(uint256 positionId) external nonReentrant {
        Position storage p = positions[positionId];
        if (p.trader == address(0)) revert PositionNotFound();
        if (p.trader != msg.sender) revert NotPositionOwner();
        if (p.liquidated)           revert AlreadyLiquidated();
        if (p.settled)              revert PositionAlreadySettled();

        Market storage m = markets[p.marketId];
        if (block.timestamp >= m.expiry) revert MarketExpired(); // use settlePosition instead

        uint256 exitPrice = _markPrice(m.oracle);
        int256  pnl       = _calcPnl(p, exitPrice);

        _closeAndPay(p, m, pnl, exitPrice, false);
        emit PositionClosed(positionId, msg.sender, pnl, exitPrice);
    }

    // ─── Settle position (after market settlement) ────────────────────────

    /// @notice Settle position at the locked settlement price.
    ///         Anyone can call — enables keeper-based batch settlement.
    function settlePosition(uint256 positionId) external nonReentrant {
        Position storage p = positions[positionId];
        if (p.trader == address(0)) revert PositionNotFound();
        if (p.liquidated)           revert AlreadyLiquidated();
        if (p.settled)              revert PositionAlreadySettled();

        Market storage m = markets[p.marketId];
        if (!m.settled) revert MarketNotSettled();

        int256 pnl = _calcPnl(p, m.settlementPrice);
        _closeAndPay(p, m, pnl, m.settlementPrice, true);
        emit PositionSettled(positionId, p.trader, pnl, m.settlementPrice);
    }

    // ─── Liquidation ──────────────────────────────────────────────────────

    /// @notice Liquidate an under-margined position (before expiry).
    function liquidate(uint256 positionId) external nonReentrant {
        Position storage p = positions[positionId];
        if (p.trader == address(0)) revert PositionNotFound();
        if (p.liquidated)           revert AlreadyLiquidated();

        Market storage m = markets[p.marketId];
        require(block.timestamp < m.expiry, "DatedFutures: use settlePosition");

        uint256 markPx = _markPrice(m.oracle);
        require(_isLiquidatable(p, markPx), "DatedFutures: not liquidatable");

        if (p.isLong) m.totalLongOI  -= p.size;
        else          m.totalShortOI -= p.size;

        p.liquidated = true;

        // SEC-2026-05-09 — CEI: account for protocol share before transfer.
        uint256 bonus = (p.collateral * LIQUIDATION_BONUS) / 10_000;
        if (bonus > p.collateral) bonus = 0;
        uint256 toProtocol = p.collateral > bonus ? p.collateral - bonus : 0;
        feeBalanceByToken[m.collateralToken] += toProtocol;
        if (bonus > 0) {
            IERC20Minimal(m.collateralToken).safeTransfer(msg.sender, bonus);
        }

        emit PositionLiquidated(positionId, msg.sender);
    }

    // ─── View helpers ─────────────────────────────────────────────────────

    function unrealisedPnl(uint256 positionId) external view returns (int256) {
        Position storage p = positions[positionId];
        Market storage m = markets[p.marketId];
        if (m.settled) return _calcPnl(p, m.settlementPrice);
        return _calcPnl(p, _markPrice(m.oracle));
    }

    function isLiquidatable(uint256 positionId) external view returns (bool) {
        Position storage p = positions[positionId];
        if (p.liquidated || p.settled || p.trader == address(0)) return false;
        Market storage m = markets[p.marketId];
        if (block.timestamp >= m.expiry) return false;
        return _isLiquidatable(p, _markPrice(m.oracle));
    }

    function markPrice(bytes32 marketId) external view returns (uint256) {
        return _markPrice(markets[marketId].oracle);
    }

    // ─── Admin ────────────────────────────────────────────────────────────

    function withdrawFees(address token) external {
        require(msg.sender == admin, "DatedFutures: not admin");
        uint256 amount = feeBalanceByToken[token];
        feeBalanceByToken[token] = 0;
        IERC20Minimal(token).safeTransfer(treasury, amount); // SEC-2026-05-09
    }

    // ─── Internal helpers ─────────────────────────────────────────────────

    function _closeAndPay(
        Position storage p, Market storage m,
        int256 pnl, uint256 exitPrice, bool isSettlement
    ) private {
        if (!isSettlement) {
            if (p.isLong) m.totalLongOI  -= p.size;
            else          m.totalShortOI -= p.size;
        }

        if (isSettlement) p.settled  = true;
        else              p.liquidated = true; // reuse flag for "closed"

        uint256 payout;
        if (pnl >= 0) {
            payout = p.collateral + uint256(pnl);
        } else {
            uint256 loss = uint256(-pnl);
            payout = loss >= p.collateral ? 0 : p.collateral - loss;
        }

        uint256 fee = (payout * PROTOCOL_FEE_BPS) / 10_000;
        feeBalanceByToken[m.collateralToken] += fee;
        uint256 out = payout > fee ? payout - fee : 0;

        if (out > 0) {
            IERC20Minimal(m.collateralToken).safeTransfer(p.trader, out); // SEC-2026-05-09
        }
    }

    function _markPrice(address oracle) private view returns (uint256) {
        // SEC-2026-05-09 — staleness check via latestRoundData.
        (uint80 roundId, int256 p, , uint256 updatedAt, uint80 answeredInRound)
            = ISettlementOracle(oracle).latestRoundData();
        require(p > 0, "DatedFutures: invalid oracle price");
        if (updatedAt == 0 || answeredInRound < roundId) revert StaleOracle();
        if (block.timestamp - updatedAt > MAX_ORACLE_DELAY) revert StaleOracle();
        uint8 dec = ISettlementOracle(oracle).decimals();
        return dec < 18 ? uint256(p) * (10 ** (18 - dec)) : uint256(p);
    }

    function _calcPnl(Position storage p, uint256 exitPrice) private view returns (int256) {
        int256 delta = int256(exitPrice) - int256(p.entryPrice);
        if (!p.isLong) delta = -delta;
        return (delta * int256(p.size)) / int256(p.entryPrice);
    }

    function _isLiquidatable(Position storage p, uint256 markPriceVal) private view returns (bool) {
        int256 pnl    = _calcPnl(p, markPriceVal);
        int256 equity = int256(p.collateral) + pnl;
        if (equity <= 0) return true;
        uint256 maintMargin = (p.size * MAINTENANCE_MARGIN) / 10_000;
        return uint256(equity) < maintMargin;
    }
}
