// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import "./libraries/SafeERC20.sol";
import "./libraries/ReentrancyGuard.sol";

/// @title  ZbxOptions — European put/call options with oracle cash settlement
/// @author Zebvix Technologies Pvt Ltd
///
/// @notice Full on-chain options market:
///           Writer (seller) creates an option series by posting collateral.
///           Buyer pays a premium and receives the right to exercise.
///           At expiry the oracle fixes the settlement price.
///           If in-the-money, the buyer (or anyone) calls exercise() and
///           the intrinsic value is paid out in the collateral token.
///
///         Option Types:
///           CALL: right to "buy" — payoff = max(0, spot − strike)
///           PUT:  right to "sell" — payoff = max(0, strike − spot)
///
///         Style: European (exercise only at/after expiry).
///
///         Positions:
///           Writer:  posts collateral, receives premium immediately.
///           Buyer:   pays premium, holds right; gets payoff if ITM.
///
///         Collateral requirement:
///           CALL writer: max payoff per contract (size / strike in base)
///             → actually cash-settled so writer posts `maxPayoutPerContract`
///             in collateral token.
///           PUT writer:  strike × contracts in collateral token.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:module     Trading / Options (ZEP-044)

interface IOraclePrx {
    function latestAnswer() external view returns (int256);
    function decimals() external view returns (uint8);
    /// @dev SEC-2026-05-09 — staleness check (Chainlink-compatible).
    function latestRoundData() external view returns (
        uint80 roundId, int256 answer, uint256 startedAt, uint256 updatedAt, uint80 answeredInRound
    );
}

contract ZbxOptions is ReentrancyGuard {
    using SafeERC20 for IERC20Minimal;

    // ─── Errors ───────────────────────────────────────────────────────────

    error SeriesNotFound();
    error SeriesExpired();
    error SeriesNotExpired();
    error SeriesAlreadySettled();
    error NotWriter();
    error NotBuyer();
    error InsufficientAmount();
    error ZeroAmount();
    error ZeroAddress();
    error NotAdmin();
    error InvalidStrike();
    error ContractAlreadyExpired();
    error NothingToExercise();
    error StaleOracle();

    // ─── Events ───────────────────────────────────────────────────────────

    event SeriesCreated(
        uint256 indexed seriesId,
        address indexed writer,
        bool    isCall,
        uint256 strikePrice,
        uint256 expiry,
        uint256 contracts,
        uint256 premium
    );
    event OptionsBought(
        uint256 indexed seriesId,
        address indexed buyer,
        uint256 contracts,
        uint256 totalPremium
    );
    event OptionsExercised(
        uint256 indexed seriesId,
        address indexed buyer,
        uint256 contracts,
        uint256 payoff,
        uint256 settlementPrice
    );
    event SeriesSettled(uint256 indexed seriesId, uint256 settlementPrice);
    event WriterWithdrawn(uint256 indexed seriesId, address indexed writer, uint256 collateralReturned);

    // ─── Constants ────────────────────────────────────────────────────────

    uint256 public constant PROTOCOL_FEE_BPS   = 50;    // 0.50% of premium
    uint256 public constant MAX_SERIES_DURATION = 365 days;
    uint256 public constant MAX_ORACLE_DELAY    = 1 hours; // SEC-2026-05-09
    /// @notice SEC-2026-05-09 Pass-3 v3 — bounded settlement window. The
    ///         oracle round used for settlement must have `updatedAt` in
    ///         `[expiry, expiry + SETTLEMENT_WINDOW]`. This prevents a
    ///         keeper from cherry-picking a favorable post-expiry print
    ///         hours later. After the window, settlement reverts on this
    ///         path (governance / admin override can ship later as a ZEP).
    uint256 public constant SETTLEMENT_WINDOW   = 1 hours;

    // ─── Types ────────────────────────────────────────────────────────────

    struct OptionSeries {
        address writer;
        address oracle;
        address collateralToken;   // settlement + collateral currency (e.g. ZUSD)
        bool    isCall;            // true = CALL, false = PUT
        uint256 strikePrice;       // 18-decimal price (quote per base)
        uint256 expiry;            // settlement timestamp
        uint256 contracts;         // total contracts written (1 contract = 1e18 units)
        uint256 contractsSold;     // contracts sold to buyers so far
        uint256 premium;           // collateral tokens per contract (18-decimal = per 1e18)
        uint256 collateralPerContract; // writer's collateral posted per contract
        uint256 settlementPrice;   // 0 until settled
        bool    settled;
        bool    writerWithdrawn;   // writer claimed remaining collateral
        // SEC-2026-05-09 — accounting trackers (see writerWithdraw notes).
        uint256 totalPayoffPaid;
        // SEC-2026-05-09 (Pass-3 fix v2) — buyer payout reserve locked at
        // settlement. Equals max payoff owed to ALL sold contracts at the
        // settlement price. writerWithdraw can only refund collateral in
        // EXCESS of this reserve; otherwise an early writer withdrawal could
        // strand buyers (insufficient balance to honor late `exercise`).
        // Any unexercised reserve remains in the contract by design (writers
        // accept this risk; future ZEP can add an exercise-window claw-back).
        uint256 buyerReserveAtSettlement;
    }

    struct BuyerPosition {
        uint256 contracts;         // contracts held
        uint256 exercised;         // contracts already exercised
    }

    // ─── State ────────────────────────────────────────────────────────────

    address public admin;
    address public treasury;

    mapping(uint256 => OptionSeries) public series;
    uint256 public seriesCount;

    /// seriesId => buyer => position
    mapping(uint256 => mapping(address => BuyerPosition)) public buyerPositions;

    mapping(address => uint256) public feeBalance;

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address treasury_) {
        require(treasury_ != address(0), "Options: zero treasury");
        admin    = msg.sender;
        treasury = treasury_;
    }

    // ─── Write option series ──────────────────────────────────────────────

    /// @notice Writer creates an option series.  Posts collateral upfront.
    ///
    /// @param  oracle           Price oracle for the underlying asset.
    /// @param  collateralToken  Currency used for premium + payoff (e.g. ZUSD).
    /// @param  isCall           True = CALL option, False = PUT option.
    /// @param  strikePrice      Exercise price (18-decimal, quote per base).
    /// @param  expiry           Settlement timestamp (max 365 days from now).
    /// @param  contracts        Number of option contracts (1 unit = 1e18).
    /// @param  premium          Premium per contract in collateral tokens.
    ///
    /// @dev    Collateral requirement:
    ///           CALL: max cash payoff is unbounded → writer posts
    ///                 strikePrice × contracts / 1e18 as capped collateral.
    ///                 This assumes the underlying can't exceed 2× strike.
    ///                 Use over-collateralised vaults for uncapped exposure.
    ///           PUT:  max payoff = strikePrice × contracts / 1e18.
    function writeSeries(
        address oracle,
        address collateralToken,
        bool    isCall,
        uint256 strikePrice,
        uint256 expiry,
        uint256 contracts,
        uint256 premium
    ) external nonReentrant returns (uint256 seriesId) {
        if (oracle == address(0) || collateralToken == address(0)) revert ZeroAddress();
        if (strikePrice == 0)              revert InvalidStrike();
        if (contracts == 0)               revert ZeroAmount();
        if (expiry <= block.timestamp)    revert ContractAlreadyExpired();
        if (expiry > block.timestamp + MAX_SERIES_DURATION) revert ContractAlreadyExpired();

        uint256 collateralPerContract = strikePrice;
        uint256 totalCollateral = (collateralPerContract * contracts) / 1e18;

        // SEC-2026-05-09 — SafeERC20.
        IERC20Minimal(collateralToken).safeTransferFrom(msg.sender, address(this), totalCollateral);

        seriesId = ++seriesCount;
        series[seriesId] = OptionSeries({
            writer:                 msg.sender,
            oracle:                 oracle,
            collateralToken:        collateralToken,
            isCall:                 isCall,
            strikePrice:            strikePrice,
            expiry:                 expiry,
            contracts:              contracts,
            contractsSold:          0,
            premium:                premium,
            collateralPerContract:  collateralPerContract,
            settlementPrice:        0,
            settled:                false,
            writerWithdrawn:        false,
            totalPayoffPaid:        0,  // SEC-2026-05-09
            buyerReserveAtSettlement: 0 // SEC-2026-05-09 Pass-3 v2
        });

        emit SeriesCreated(seriesId, msg.sender, isCall, strikePrice, expiry, contracts, premium);
    }

    // ─── Buy options ──────────────────────────────────────────────────────

    /// @notice Buy `amount` contracts from a series.  Pay `premium × amount`.
    /// @param  seriesId   The option series to buy from.
    /// @param  amount     Number of contracts to buy (in 1e18 units).
    function buyOptions(uint256 seriesId, uint256 amount) external nonReentrant {
        if (amount == 0) revert ZeroAmount();
        OptionSeries storage s = series[seriesId];
        if (s.writer == address(0))        revert SeriesNotFound();
        if (block.timestamp >= s.expiry)   revert SeriesExpired();
        if (s.settled)                     revert SeriesAlreadySettled();
        if (s.contractsSold + amount > s.contracts) revert InsufficientAmount();

        uint256 totalPremium = (s.premium * amount) / 1e18;
        uint256 fee = (totalPremium * PROTOCOL_FEE_BPS) / 10_000;
        uint256 writerPremium = totalPremium - fee;
        address collTok = s.collateralToken;
        address writer  = s.writer;

        // SEC-2026-05-09 — CEI: state before external transfers; SafeERC20.
        s.contractsSold += amount;
        buyerPositions[seriesId][msg.sender].contracts += amount;
        feeBalance[collTok] += fee;

        IERC20Minimal(collTok).safeTransferFrom(msg.sender, address(this), totalPremium);
        IERC20Minimal(collTok).safeTransfer(writer, writerPremium);

        emit OptionsBought(seriesId, msg.sender, amount, totalPremium);
    }

    // ─── Settle series ────────────────────────────────────────────────────

    /// @notice Lock the settlement price after expiry. Anyone can call.
    function settleSeries(uint256 seriesId) external {
        OptionSeries storage s = series[seriesId];
        if (s.writer == address(0))        revert SeriesNotFound();
        if (block.timestamp < s.expiry)    revert SeriesNotExpired();
        if (s.settled)                     revert SeriesAlreadySettled();

        // SEC-2026-05-09 Pass-3 v3 — settlement integrity.
        // Round must be finalized AND its `updatedAt` must lie in the bounded
        // window `[expiry, expiry + SETTLEMENT_WINDOW]`. This eliminates the
        // keeper-time-selection vector: a malicious keeper can no longer wait
        // hours/days for a favorable post-expiry print to lock in.
        (uint80 roundId, int256 rawPrice, , uint256 updatedAt, uint80 answeredInRound)
            = IOraclePrx(s.oracle).latestRoundData();
        require(rawPrice > 0, "Options: invalid oracle price");
        if (updatedAt == 0 || answeredInRound < roundId) revert StaleOracle();
        if (updatedAt < s.expiry) revert StaleOracle();
        if (updatedAt > s.expiry + SETTLEMENT_WINDOW) revert StaleOracle();
        uint8 dec = IOraclePrx(s.oracle).decimals();
        uint256 price = dec < 18
            ? uint256(rawPrice) * (10 ** (18 - dec))
            : uint256(rawPrice);

        s.settlementPrice = price;
        s.settled         = true;
        // SEC-2026-05-09 Pass-3 v2 — lock buyer payout reserve at settlement.
        // After this point, writerWithdraw may NOT refund this amount even if
        // buyers have not yet exercised. Prevents an early writer withdrawal
        // from rendering future `exercise` calls insolvent.
        s.buyerReserveAtSettlement = _calcPayoff(s, s.contractsSold);

        emit SeriesSettled(seriesId, price);
    }

    // ─── Exercise options ─────────────────────────────────────────────────

    /// @notice Buyer (or anyone on behalf of buyer) exercises ITM options.
    ///         Only callable after `settleSeries()`.
    /// @param  seriesId   The option series.
    /// @param  buyer      The option holder to exercise for.
    /// @param  amount     Contracts to exercise (≤ buyer's unexercised balance).
    function exercise(uint256 seriesId, address buyer, uint256 amount) external nonReentrant {
        if (amount == 0) revert ZeroAmount();
        OptionSeries storage s = series[seriesId];
        if (s.writer == address(0)) revert SeriesNotFound();
        if (!s.settled)             revert SeriesNotExpired(); // must settle first

        BuyerPosition storage bp = buyerPositions[seriesId][buyer];
        uint256 exercisable = bp.contracts - bp.exercised;
        if (amount > exercisable) amount = exercisable;
        if (amount == 0) revert NothingToExercise();

        uint256 payoff = _calcPayoff(s, amount);
        if (payoff == 0) revert NothingToExercise(); // OTM — out of the money

        // SEC-2026-05-09 — track payoff so writerWithdraw can refund the
        // exact unused collateral (was structurally undercounted before).
        bp.exercised += amount;
        s.totalPayoffPaid += payoff;

        IERC20Minimal(s.collateralToken).safeTransfer(buyer, payoff);

        emit OptionsExercised(seriesId, buyer, amount, payoff, s.settlementPrice);
    }

    // ─── Writer reclaim collateral ────────────────────────────────────────

    /// @notice After settlement, writer reclaims unexercised collateral.
    function writerWithdraw(uint256 seriesId) external nonReentrant {
        OptionSeries storage s = series[seriesId];
        if (s.writer != msg.sender)    revert NotWriter();
        if (!s.settled)                revert SeriesNotExpired();
        if (s.writerWithdrawn)         return;

        s.writerWithdrawn = true;

        // SEC-2026-05-09 Pass-3 v2 — CRITICAL FIX (revised after architect
        // review). Original Pass-3 patch refunded `posted − totalPayoffPaid`,
        // but `totalPayoffPaid` only grows when buyers actually call
        // `exercise()`. A writer could call `writerWithdraw` IMMEDIATELY after
        // settlement (before buyers exercised) and drain the entire ITM
        // liability reserve, leaving subsequent `exercise()` calls insolvent.
        // Solvency fix: refund = posted − max(buyerReserveAtSettlement, totalPayoffPaid).
        // The reserve is locked at settlement and ensures buyers can always
        // be paid the maximum they could have earned at the settlement price.
        // Unexercised reserve stays locked (documented design tradeoff).
        uint256 totalCollateralPosted = (s.collateralPerContract * s.contracts) / 1e18;
        uint256 reserve = s.buyerReserveAtSettlement > s.totalPayoffPaid
            ? s.buyerReserveAtSettlement
            : s.totalPayoffPaid;
        uint256 writerRefund = totalCollateralPosted > reserve
            ? totalCollateralPosted - reserve
            : 0;

        if (writerRefund > 0) {
            IERC20Minimal(s.collateralToken).safeTransfer(s.writer, writerRefund);
        }

        emit WriterWithdrawn(seriesId, s.writer, writerRefund);
    }

    // ─── View helpers ─────────────────────────────────────────────────────

    /// @notice Payoff per contract at current/settlement price.
    function intrinsicValue(uint256 seriesId) external view returns (uint256) {
        OptionSeries storage s = series[seriesId];
        uint256 price = s.settled ? s.settlementPrice : _currentPrice(s.oracle);
        if (s.isCall) {
            return price > s.strikePrice ? price - s.strikePrice : 0;
        } else {
            return s.strikePrice > price ? s.strikePrice - price : 0;
        }
    }

    /// @notice Is the option in-the-money?
    function isITM(uint256 seriesId) external view returns (bool) {
        OptionSeries storage s = series[seriesId];
        uint256 price = s.settled ? s.settlementPrice : _currentPrice(s.oracle);
        return s.isCall ? price > s.strikePrice : price < s.strikePrice;
    }

    /// @notice Exercisable contracts for a buyer.
    function exercisableContracts(uint256 seriesId, address buyer) external view returns (uint256) {
        BuyerPosition storage bp = buyerPositions[seriesId][buyer];
        return bp.contracts - bp.exercised;
    }

    /// @notice Total payoff for `amount` contracts at settlement price.
    function estimatePayoff(uint256 seriesId, uint256 amount) external view returns (uint256) {
        OptionSeries storage s = series[seriesId];
        return _calcPayoff(s, amount);
    }

    // ─── Admin ────────────────────────────────────────────────────────────

    function withdrawFees(address token) external {
        require(msg.sender == admin, "Options: not admin");
        uint256 amount = feeBalance[token];
        feeBalance[token] = 0;
        IERC20Minimal(token).safeTransfer(treasury, amount); // SEC-2026-05-09
    }

    // ─── Internal helpers ─────────────────────────────────────────────────

    function _calcPayoff(OptionSeries storage s, uint256 amount)
        private view returns (uint256)
    {
        uint256 intrinsic;
        if (s.isCall) {
            intrinsic = s.settlementPrice > s.strikePrice
                ? s.settlementPrice - s.strikePrice : 0;
        } else {
            intrinsic = s.strikePrice > s.settlementPrice
                ? s.strikePrice - s.settlementPrice : 0;
        }
        if (intrinsic == 0) return 0;
        // Cash payoff = intrinsic × amount / 1e18 (contracts are 18-decimal units)
        uint256 raw = (intrinsic * amount) / 1e18;
        // Cap at collateral posted per contract × amount
        uint256 cap = (s.collateralPerContract * amount) / 1e18;
        return raw < cap ? raw : cap;
    }

    function _maxPayoffPerContract(OptionSeries storage s) private view returns (uint256) {
        // PUT: max payoff = strikePrice (price goes to 0)
        // CALL: we capped at strikePrice in this v1 implementation
        return s.strikePrice;
    }

    function _currentPrice(address oracle) private view returns (uint256) {
        // SEC-2026-05-09 — staleness check.
        (uint80 roundId, int256 p, , uint256 updatedAt, uint80 answeredInRound)
            = IOraclePrx(oracle).latestRoundData();
        require(p > 0, "Options: invalid oracle price");
        require(updatedAt > 0 && answeredInRound >= roundId, "Options: stale oracle");
        require(block.timestamp - updatedAt <= MAX_ORACLE_DELAY, "Options: stale oracle");
        uint8 dec = IOraclePrx(oracle).decimals();
        return dec < 18 ? uint256(p) * (10 ** (18 - dec)) : uint256(p);
    }
}
