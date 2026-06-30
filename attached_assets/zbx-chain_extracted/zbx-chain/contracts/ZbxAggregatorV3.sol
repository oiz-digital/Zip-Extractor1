// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

/// @title ZbxAggregatorV3 — Chainlink AggregatorV3Interface compatible
/// @notice Drop-in replacement for Chainlink price feeds on ZBX chain.
///         Any contract written for Chainlink works on ZBX without changes.
/// @dev    Updated by the ZBX oracle network via Oracle nodes.
///         Access control: only approved oracle addresses can submit prices.

interface AggregatorV3Interface {
    function decimals()        external view returns (uint8);
    function description()     external view returns (string memory);
    function version()         external view returns (uint256);

    function getRoundData(uint80 _roundId) external view returns (
        uint80  roundId,
        int256  answer,
        uint256 startedAt,
        uint256 updatedAt,
        uint80  answeredInRound
    );

    function latestRoundData() external view returns (
        uint80  roundId,
        int256  answer,
        uint256 startedAt,
        uint256 updatedAt,
        uint80  answeredInRound
    );
}

contract ZbxAggregatorV3 is AggregatorV3Interface {

    // ── State ──────────────────────────────────────────────────────────────

    address public owner;
    string  public feedName;      // e.g. "ZBX / USD"
    uint8   public override decimals;       // 8 (Chainlink standard)
    uint256 public override version = 1;
    /// SEC-2026-05-09 Pass-15 (HIGH-S04): max per-round deviation
    /// from previous round, in basis points. 25% chosen as a balance
    /// between absorbing genuine market-stress moves (BTC has had
    /// single-hour 15% candles) and refusing oracle-poisoning attempts
    /// that need 2x+ moves to be profitable. Governance-tunable in a
    /// follow-up.
    uint256 public constant MAX_DEVIATION_BPS = 2_500;

    /// Approved oracle node addresses (reporters)
    mapping(address => bool) public isReporter;

    /// Oracle round data
    struct Round {
        int256  answer;        // Price × 10^8
        uint256 startedAt;
        uint256 updatedAt;
        uint32  reportCount;   // how many reporters contributed
    }

    mapping(uint80 => Round) private rounds;
    uint80 public latestRound;

    /// Minimum reporters required per round
    uint32 public minReporters;

    /// Pending submissions for the current open round
    mapping(address => int256) private pendingReports;
    address[] private pendingReporters;
    uint80 public openRoundId;

    // ── Events ─────────────────────────────────────────────────────────────

    event RoundUpdated(uint80 indexed roundId, int256 price, uint256 updatedAt);
    event ReporterAdded(address indexed reporter);
    event ReporterRemoved(address indexed reporter);
    event PriceSubmitted(address indexed reporter, uint80 roundId, int256 price);

    // ── Errors ─────────────────────────────────────────────────────────────

    error NotOwner();
    error NotReporter();
    error AlreadySubmitted();
    error NoRoundData();
    error StalePrice();

    // ── Constructor ────────────────────────────────────────────────────────

    constructor(
        string memory _feedName,
        uint8         _decimals,
        uint32        _minReporters,
        address[]     memory _reporters
    ) {
        owner        = msg.sender;
        feedName     = _feedName;
        decimals     = _decimals;
        minReporters = _minReporters;
        for (uint i = 0; i < _reporters.length; i++) {
            isReporter[_reporters[i]] = true;
        }
        openRoundId  = 1;
    }

    // ── Reporter submission ────────────────────────────────────────────────

    /// @notice Submit a price for the current open round.
    ///         Called by approved oracle nodes.
    /// @param price Price × 10^8 (e.g. $2.50 → 250_000_000)
    function submitPrice(int256 price) external {
        if (!isReporter[msg.sender]) revert NotReporter();

        // Check not already submitted this round
        bool alreadySubmitted = false;
        for (uint i = 0; i < pendingReporters.length; i++) {
            if (pendingReporters[i] == msg.sender) {
                alreadySubmitted = true;
                break;
            }
        }
        if (alreadySubmitted) revert AlreadySubmitted();

        pendingReports[msg.sender] = price;
        pendingReporters.push(msg.sender);

        emit PriceSubmitted(msg.sender, openRoundId, price);

        // Auto-close if enough reporters have submitted
        if (pendingReporters.length >= minReporters) {
            _closeRound();
        }
    }

    /// @dev Close the current round by computing the median of submissions.
    function _closeRound() internal {
        uint n = pendingReporters.length;
        int256[] memory prices = new int256[](n);
        for (uint i = 0; i < n; i++) {
            prices[i] = pendingReports[pendingReporters[i]];
        }

        // Sort (insertion sort — gas efficient for small n)
        for (uint i = 1; i < n; i++) {
            int256 key = prices[i];
            uint j = i;
            while (j > 0 && prices[j - 1] > key) {
                prices[j] = prices[j - 1];
                j--;
            }
            prices[j] = key;
        }

        // Median
        int256 median;
        if (n % 2 == 1) {
            median = prices[n / 2];
        } else {
            median = (prices[n / 2 - 1] + prices[n / 2]) / 2;
        }

        // SEC-2026-05-09 Pass-15 (HIGH-S04 / Pass-12 Tier-2 Aggregator-no-deviation):
        // Per-round deviation cap. Pre-fix the aggregator accepted any
        // median regardless of how far it diverged from the prior round
        // — a coordinated minority of compromised reporters could push
        // the median 100x in one round and trigger every downstream
        // liquidation. New code rejects rounds whose median deviates
        // by more than `MAX_DEVIATION_BPS` (default 25%) from the
        // previous round, forcing attackers to walk the price up over
        // many rounds (= many minutes) where it can be detected.
        // First-round (latestRound == 0) is exempt — there is no prior
        // value to compare against.
        if (latestRound != 0) {
            int256 prev = rounds[latestRound].answer;
            if (prev > 0 && median > 0) {
                uint256 prevAbs = uint256(prev);
                uint256 medAbs  = uint256(median);
                uint256 diff    = medAbs > prevAbs ? medAbs - prevAbs : prevAbs - medAbs;
                require(
                    (diff * 10_000) / prevAbs <= MAX_DEVIATION_BPS,
                    "Aggregator: deviation too large"
                );
            }
        }

        // Write round
        uint80 rid = openRoundId;
        rounds[rid] = Round({
            answer:      median,
            startedAt:   block.timestamp,
            updatedAt:   block.timestamp,
            reportCount: uint32(n)
        });
        latestRound = rid;

        emit RoundUpdated(rid, median, block.timestamp);

        // Clear pending state, advance round
        for (uint i = 0; i < pendingReporters.length; i++) {
            delete pendingReports[pendingReporters[i]];
        }
        delete pendingReporters;
        openRoundId++;
    }

    // ── AggregatorV3Interface ──────────────────────────────────────────────

    function description() external view override returns (string memory) {
        return feedName;
    }

    function getRoundData(uint80 _roundId) external view override returns (
        uint80 roundId, int256 answer,
        uint256 startedAt, uint256 updatedAt, uint80 answeredInRound
    ) {
        Round storage r = rounds[_roundId];
        if (r.updatedAt == 0) revert NoRoundData();
        return (_roundId, r.answer, r.startedAt, r.updatedAt, _roundId);
    }

    function latestRoundData() external view override returns (
        uint80 roundId, int256 answer,
        uint256 startedAt, uint256 updatedAt, uint80 answeredInRound
    ) {
        if (latestRound == 0) revert NoRoundData();
        Round storage r = rounds[latestRound];
        // Staleness check: price must be < 2 hours old
        if (block.timestamp - r.updatedAt > 7200) revert StalePrice();
        return (latestRound, r.answer, r.startedAt, r.updatedAt, latestRound);
    }

    // ── Admin ──────────────────────────────────────────────────────────────

    function addReporter(address reporter) external {
        if (msg.sender != owner) revert NotOwner();
        isReporter[reporter] = true;
        emit ReporterAdded(reporter);
    }

    function removeReporter(address reporter) external {
        if (msg.sender != owner) revert NotOwner();
        isReporter[reporter] = false;
        emit ReporterRemoved(reporter);
    }

    function setMinReporters(uint32 min) external {
        if (msg.sender != owner) revert NotOwner();
        minReporters = min;
    }

    function forceClose() external {
        if (msg.sender != owner) revert NotOwner();
        if (pendingReporters.length > 0) {
            _closeRound();
        }
    }
}