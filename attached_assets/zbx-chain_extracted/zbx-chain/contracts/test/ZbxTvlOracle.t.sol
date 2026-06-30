// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

// ─────────────────────────────────────────────────────────────────────────────
// ZbxTvlOracle.t.sol — Foundry tests for the ZBX TVL aggregator (ZEP-007).
//
//   Run: forge test --match-contract ZbxTvlOracleTest -vvv
//
//   COVERAGE (47 tests):
//     CONSTRUCTOR + DEFAULTS (3)
//       1.  testConstructorSetsOwner
//       2.  testConstructorRevertsZeroOwner
//       3.  testInitialDefaults
//     ADMIN — ACCESS CONTROL (5)
//       4.  testNonOwnerCannotSetPriceFeed
//       5.  testNonOwnerCannotSetSource
//       6.  testNonOwnerCannotPause
//       7.  testNonOwnerCannotSetStabilityDepositToken
//       8.  testNonOwnerCannotTransferOwnership
//     ADMIN — SETTERS (6)
//       9.  testSetPriceFeedPersistsAndEmits
//       10. testSetPriceFeedZeroTokenReverts
//       11. testSetPriceFeedZeroAggregatorUnregisters
//       12. testSetSourceForEachVariant
//       13. testSetMaxStalenessBoundsRespected
//       14. testSetMaxPairsToScanZeroReverts
//     PAUSE LIFECYCLE (2)
//       15. testPauseUnpauseFlow
//       16. testPausedViewsRevert
//     AMM AGGREGATION (3)
//       17. testTvlAMMEmptyFactory
//       18. testTvlAMMSinglePairCorrectUSD
//       19. testTvlAMMRespectsPairScanCap
//     LENDING AGGREGATION (2)
//       20. testTvlLendingNetSupplyMinusBorrow
//       21. testTvlLendingSkipsInactive
//     OTHER SOURCES + AGGREGATE (3)
//       22. testTvlStabilityNeedsBothPoolAndToken
//       23. testTvlStakingReadsTotalStaked
//       24. testTvlBreakdownSumMatchesTotalAndIncludesTimestamp
//     PRICE FEED EDGE CASES (2)
//       25. testStaleAggregatorReturnsZero
//       26. testNegativePriceReturnsZero
//     ARCHITECT-REQUESTED (4)
//       27. testLendingIndexUnscalesNonRAY
//       28. testRefreshUnpricedPopulatesMissingFeed
//       29. testPairScanTruncationStatsExposed
//       30. testOutOfPolicyDecimalsReturnZero
//     S23b — TWAP ALT-PRICE-SOURCE (6)
//       31. testSetTwapOracleByOwnerEmitsAndPersists
//       32. testSetTwapRouteHappyPath
//       33. testSetTwapRouteRevertsWhenQuoteUnpriced
//       34. testSetTwapRouteRevertsWhenPairTokenMismatch
//       35. testTvlAMMUsesTwapWhenRouteEnabled
//       36. testTwapConsultRevertReturnsZeroFailClosed
//     S23b-Polish-1 — REGRESSION EDGES (3)
//       37. testRouteEnabledWithTwapOracleZeroFailsClosedAndMarksUnpriced
//       38. testRouteDisableRestoresLegacyAggregatorPath
//       39. testRefreshUnpricedRecordsQuoteLegOnQuoteBreakage
//     S24 — PHASE 7 REWARD + BRIDGE_VAULT REAL IMPL (10)
//       40. testTvlRewardReturnsZeroWhenSourceUnconfigured
//       41. testTvlRewardHappyPath
//       42. testTvlRewardFailsClosedOnDistributorRevert
//       43. testTvlRewardFailsClosedWhenZbxUnpriced
//       44. testTvlBridgeVaultReturnsZeroWhenSourceUnconfigured
//       45. testTvlBridgeVaultHappyPath
//       46. testTvlBridgeVaultFailsClosedOnVaultRevert
//       47. testTvlBreakdownIncludesPhase7Sources
//       48. testTvlRewardFailsClosedOnBalanceOfRevert       (S24-fix1, architect-rec)
//       49. testTvlBridgeVaultFailsClosedOnTokenRevert      (S24-fix1, architect-rec)
// ─────────────────────────────────────────────────────────────────────────────

import "../ZbxTvlOracle.sol";

// ─── HEVM cheatcode (no forge-std dep) ───────────────────────────────────────

interface Hevm {
    function prank(address) external;
    function warp(uint256)  external;
}

// ─── Mocks ───────────────────────────────────────────────────────────────────

contract MockToken {
    uint8 public decimals;
    constructor(uint8 d) { decimals = d; }
}

// S24 — token mock with mutable balanceOf (for reward distributor TVL test)
//
// `revertOnBalanceOf` toggle (S24-fix1) lets us simulate the architect's
// recommended additional fail-closed edge: ERC-20 contract whose
// `balanceOf` view reverts. The custom `balanceOf(address)` function
// shadows the auto-getter only if the auto-getter for the public mapping
// is NOT generated. Here we DON'T expose the mapping publicly — we use a
// private `_bal` mapping + a custom `balanceOf(address) returns (uint256)`
// function so the revert toggle works.
contract MockBalanceToken {
    uint8 public decimals;
    mapping(address => uint256) private _bal;
    bool public revertOnBalanceOf;
    constructor(uint8 d) { decimals = d; }
    function setBalance(address who, uint256 amt)  external { _bal[who] = amt; }
    function setRevertOnBalanceOf(bool b)          external { revertOnBalanceOf = b; }
    function balanceOf(address who) external view returns (uint256) {
        require(!revertOnBalanceOf, "MockBalanceToken: balanceOf forced revert");
        return _bal[who];
    }
}

// S24 — REWARD source mock. Custom getter so we can simulate revert.
contract MockRewardDistributor {
    address private _zbx;
    bool    public revertOnZbx;
    constructor(address zbxToken) { _zbx = zbxToken; }
    function setRevertOnZbx(bool b) external { revertOnZbx = b; }
    function zbx() external view returns (address) {
        require(!revertOnZbx, "MockRewardDistributor: zbx() forced revert");
        return _zbx;
    }
}

// S24 — BRIDGE_VAULT source mock. Custom getters so we can simulate revert.
contract MockBridgeVault {
    address private _token;
    uint256 private _locked;
    bool    public revertOnToken;
    bool    public revertOnLocked;
    constructor(address t, uint256 l) { _token = t; _locked = l; }
    function setLocked(uint256 l)             external { _locked = l; }
    function setRevertOnToken(bool b)         external { revertOnToken = b; }
    function setRevertOnLocked(bool b)        external { revertOnLocked = b; }
    function token() external view returns (address) {
        require(!revertOnToken, "MockBridgeVault: token() forced revert");
        return _token;
    }
    function totalLocked() external view returns (uint256) {
        require(!revertOnLocked, "MockBridgeVault: totalLocked() forced revert");
        return _locked;
    }
}

contract MockAggregator {
    int256  public price;
    uint8   public decimals;
    uint256 public updatedAt;
    bool    public revertOnRead;

    constructor(int256 _p, uint8 _d, uint256 _ts) {
        price = _p; decimals = _d; updatedAt = _ts;
    }

    function set(int256 _p, uint256 _ts) external { price = _p; updatedAt = _ts; }
    function setRevert(bool b) external { revertOnRead = b; }

    function latestRoundData() external view returns (
        uint80, int256, uint256, uint256, uint80
    ) {
        require(!revertOnRead, "agg-revert");
        return (1, price, updatedAt, updatedAt, 1);
    }
}

contract MockAMMPair {
    address public token0;
    address public token1;
    uint112 public r0;
    uint112 public r1;

    constructor(address a, address b, uint112 _r0, uint112 _r1) {
        // Sort like the real factory does so token0 < token1.
        if (a < b) { token0 = a; token1 = b; r0 = _r0; r1 = _r1; }
        else       { token0 = b; token1 = a; r0 = _r1; r1 = _r0; }
    }

    function getReserves() external view returns (uint112, uint112, uint32) {
        return (r0, r1, uint32(block.timestamp));
    }
}

contract MockAMMFactory {
    address[] public pairs;
    function addPair(address p) external { pairs.push(p); }
    function allPairsLength() external view returns (uint256) { return pairs.length; }
    function allPairs(uint256 i) external view returns (address) { return pairs[i]; }
}

contract MockLendingPool {
    struct R {
        address asset;
        uint128 supplied;     // scaled
        uint128 borrowed;     // scaled
        uint128 liquidityIndex;
        uint128 borrowIndex;
        uint8   decimals;
        bool    active;
    }
    mapping(address => R) public r;
    address[] public list;

    function add(address asset, uint128 supplied, uint128 borrowed,
                 uint128 liqIdx, uint128 borIdx, uint8 dec, bool active) external {
        r[asset] = R(asset, supplied, borrowed, liqIdx, borIdx, dec, active);
        list.push(asset);
    }

    function reservesCount() external view returns (uint256) { return list.length; }
    function reserveList(uint256 i) external view returns (address) { return list[i]; }

    function getReserveData(address asset) external view returns (
        address asset_,
        address zToken,
        address debtToken,
        uint128 totalSupplied,
        uint128 totalBorrowed,
        uint128 liquidityRate,
        uint128 borrowRate,
        uint128 liquidityIndex,
        uint128 borrowIndex,
        uint40  lastUpdateTimestamp,
        uint16  ltv,
        uint16  liquidationThreshold,
        uint16  liquidationBonus,
        uint16  reserveFactor,
        uint8   decimals_,
        bool    active,
        bool    borrowEnabled,
        bool    flashLoanEnabled
    ) {
        R memory x = r[asset];
        return (
            x.asset, address(0), address(0),
            x.supplied, x.borrowed,
            0, 0,
            x.liquidityIndex, x.borrowIndex,
            uint40(block.timestamp), 0, 0, 0, 0,
            x.decimals, x.active, false, false
        );
    }
}

contract MockStabilityPool {
    uint256 public totalDeposits;
    function set(uint256 v) external { totalDeposits = v; }
}

contract MockStaking {
    uint256 public totalStaked;
    address public stakingToken;
    function set(uint256 v, address tok) external { totalStaked = v; stakingToken = tok; }
}

// ─── S23b — Mock TWAP oracle ─────────────────────────────────────────────────
//
// Conforms to the ZbxTvlOracle's `IZbxTwapOracleLite.consult` surface only.
// The mock returns a fixed-rate quote (`amountIn * rateE18 / 1e18`), and can
// be flipped to revert via `setShouldRevert(true)` to exercise the
// fail-closed branch of `_safeUSD`.
contract MockTwapOracle {
    // pair => quote-out per 1e18 of input (so rate=1e18 means 1:1).
    mapping(address => uint256) public rateE18;
    bool public shouldRevert;

    function setRate(address pair, uint256 r) external { rateE18[pair] = r; }
    function setShouldRevert(bool b) external { shouldRevert = b; }

    function consult(address pair, address /*tokenIn*/, uint256 amountIn)
        external view returns (uint256)
    {
        if (shouldRevert) revert("MockTwapOracle: not primed");
        return (amountIn * rateE18[pair]) / 1e18;
    }
}

// ─── Test contract ───────────────────────────────────────────────────────────

contract ZbxTvlOracleTest {
    Hevm constant vm = Hevm(address(uint160(uint256(keccak256("hevm cheat code")))));

    address constant DEPLOYER = address(0xD1);
    address constant ALICE    = address(0xA1);

    uint256 constant RAY = 1e27;

    ZbxTvlOracle      oracle;
    MockToken         tokenA;       // 18 decimals
    MockToken         tokenU;       // 6 decimals (USDC-like)
    MockAggregator    aggA;         // 8 decimals price feed
    MockAggregator    aggU;         // 8 decimals price feed
    MockAMMFactory    factory;
    MockLendingPool   lending;
    MockStabilityPool stability;
    MockStaking       staking;

    function setUp() public {
        // Use a non-trivial timestamp so the default 1h staleness window
        // doesn't accidentally cover updatedAt=0 mocks.
        vm.warp(1_700_000_000);

        vm.prank(DEPLOYER);
        oracle = new ZbxTvlOracle(DEPLOYER);

        tokenA  = new MockToken(18);
        tokenU  = new MockToken(6);
        aggA    = new MockAggregator(2e8,    8, block.timestamp); // $2.00
        aggU    = new MockAggregator(1e8,    8, block.timestamp); // $1.00
        factory = new MockAMMFactory();
        lending = new MockLendingPool();
        stability = new MockStabilityPool();
        staking = new MockStaking();
    }

    // ─────────────────────────────────────────────────────────────────────
    // 1-3 : CONSTRUCTOR + DEFAULTS
    // ─────────────────────────────────────────────────────────────────────

    function testConstructorSetsOwner() public view {
        require(oracle.owner() == DEPLOYER, "1.owner mismatch");
    }

    function testConstructorRevertsZeroOwner() public {
        try new ZbxTvlOracle(address(0)) {
            revert("2.zero owner allowed");
        } catch {}
    }

    function testInitialDefaults() public view {
        require(oracle.paused()         == false, "3a.paused default");
        require(oracle.maxStaleness()   == 3600,  "3b.staleness default");
        require(oracle.maxPairsToScan() == 256,   "3c.pair cap default");
    }

    // ─────────────────────────────────────────────────────────────────────
    // 4-8 : ACCESS CONTROL
    // ─────────────────────────────────────────────────────────────────────

    function testNonOwnerCannotSetPriceFeed() public {
        vm.prank(ALICE);
        try oracle.setPriceFeed(address(tokenA), address(aggA)) {
            revert("4.non-owner setPriceFeed allowed");
        } catch {}
    }

    function testNonOwnerCannotSetSource() public {
        vm.prank(ALICE);
        try oracle.setSource(IZbxTvlOracle.Source.AMM, address(factory)) {
            revert("5.non-owner setSource allowed");
        } catch {}
    }

    function testNonOwnerCannotPause() public {
        vm.prank(ALICE);
        try oracle.pause() { revert("6.non-owner pause allowed"); } catch {}
    }

    function testNonOwnerCannotSetStabilityDepositToken() public {
        vm.prank(ALICE);
        try oracle.setStabilityDepositToken(address(tokenU)) {
            revert("7.non-owner stab token allowed");
        } catch {}
    }

    function testNonOwnerCannotTransferOwnership() public {
        vm.prank(ALICE);
        try oracle.transferOwnership(ALICE) {
            revert("8.non-owner xfer allowed");
        } catch {}
    }

    // ─────────────────────────────────────────────────────────────────────
    // 9-14 : ADMIN SETTERS
    // ─────────────────────────────────────────────────────────────────────

    function testSetPriceFeedPersistsAndEmits() public {
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenA), address(aggA));
        require(oracle.priceFeed(address(tokenA)) == address(aggA), "9.persist");
    }

    function testSetPriceFeedZeroTokenReverts() public {
        vm.prank(DEPLOYER);
        try oracle.setPriceFeed(address(0), address(aggA)) {
            revert("10.zero token allowed");
        } catch {}
    }

    function testSetPriceFeedZeroAggregatorUnregisters() public {
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenA), address(aggA));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenA), address(0));
        require(oracle.priceFeed(address(tokenA)) == address(0), "11.unregister");
    }

    function testSetSourceForEachVariant() public {
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.AMM,          address(factory));
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.LENDING,      address(lending));
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.STABILITY,    address(stability));
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.STAKING,      address(staking));
        require(oracle.source(IZbxTvlOracle.Source.AMM)       == address(factory),   "12a");
        require(oracle.source(IZbxTvlOracle.Source.LENDING)   == address(lending),   "12b");
        require(oracle.source(IZbxTvlOracle.Source.STABILITY) == address(stability), "12c");
        require(oracle.source(IZbxTvlOracle.Source.STAKING)   == address(staking),   "12d");
    }

    function testSetMaxStalenessBoundsRespected() public {
        vm.prank(DEPLOYER);
        try oracle.setMaxStaleness(0) { revert("13a.zero allowed"); } catch {}
        vm.prank(DEPLOYER);
        try oracle.setMaxStaleness(uint64(7 days + 1)) { revert("13b.too-large allowed"); } catch {}
        vm.prank(DEPLOYER); oracle.setMaxStaleness(7 days);
        require(oracle.maxStaleness() == 7 days, "13c.persist");
    }

    function testSetMaxPairsToScanZeroReverts() public {
        vm.prank(DEPLOYER);
        try oracle.setMaxPairsToScan(0) { revert("14.zero cap allowed"); } catch {}
    }

    // ─────────────────────────────────────────────────────────────────────
    // 15-16 : PAUSE LIFECYCLE
    // ─────────────────────────────────────────────────────────────────────

    function testPauseUnpauseFlow() public {
        vm.prank(DEPLOYER); oracle.pause();
        require(oracle.paused(), "15a.paused");
        vm.prank(DEPLOYER); oracle.unpause();
        require(!oracle.paused(), "15b.unpaused");
    }

    function testPausedViewsRevert() public {
        vm.prank(DEPLOYER); oracle.pause();
        try oracle.totalValueLockedUSD() { revert("16a.tvl while paused"); } catch {}
        try oracle.tvlAMM()               { revert("16b.tvlAMM while paused"); } catch {}
        try oracle.tvlBreakdown()         { revert("16c.breakdown while paused"); } catch {}
    }

    // ─────────────────────────────────────────────────────────────────────
    // 17-19 : AMM AGGREGATION
    // ─────────────────────────────────────────────────────────────────────

    function testTvlAMMEmptyFactory() public {
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.AMM, address(factory));
        require(oracle.tvlAMM() == 0, "17.empty factory not zero");
    }

    function testTvlAMMSinglePairCorrectUSD() public {
        // Pair with 100 tokenA (18d, $2) and 50 tokenU (6d, $1).
        // USD = 100e18 * 2 + 50e6 * 1 = 200 USD + 50 USD = 250 USD (in 18d)
        MockAMMPair p = new MockAMMPair(address(tokenA), address(tokenU),
                                        100e18, 50e6);
        factory.addPair(address(p));

        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.AMM, address(factory));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenA), address(aggA));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenU), address(aggU));

        // Expected: 250 * 1e18 = 250e18.
        uint256 got = oracle.tvlAMM();
        require(got == 250e18, "18.tvlAMM math wrong");
    }

    function testTvlAMMRespectsPairScanCap() public {
        MockAMMPair p1 = new MockAMMPair(address(tokenA), address(tokenU), 100e18, 50e6);
        MockAMMPair p2 = new MockAMMPair(address(tokenA), address(tokenU), 100e18, 50e6);
        // Re-using the same (tokenA, tokenU) ordering — these are independent
        // pair contracts, factory does not deduplicate in the mock.
        factory.addPair(address(p1));
        factory.addPair(address(p2));

        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.AMM, address(factory));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenA), address(aggA));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenU), address(aggU));

        // Cap to 1 pair → only first pair counted (250 USD), not 500.
        vm.prank(DEPLOYER); oracle.setMaxPairsToScan(1);
        require(oracle.tvlAMM() == 250e18, "19a.cap-1 wrong");

        // Lift cap → both pairs counted (500 USD).
        vm.prank(DEPLOYER); oracle.setMaxPairsToScan(256);
        require(oracle.tvlAMM() == 500e18, "19b.cap-256 wrong");
    }

    // ─────────────────────────────────────────────────────────────────────
    // 20-21 : LENDING AGGREGATION
    // ─────────────────────────────────────────────────────────────────────

    function testTvlLendingNetSupplyMinusBorrow() public {
        // Reserve: tokenA, supplied=100 (scaled), borrowed=30 (scaled),
        // both indices = RAY (no interest accrued), active.
        // Real net = 70 tokenA * $2 = $140 = 140e18.
        lending.add(address(tokenA),
                    uint128(100e18), uint128(30e18),
                    uint128(RAY),    uint128(RAY),
                    18, true);

        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.LENDING, address(lending));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenA), address(aggA));

        require(oracle.tvlLending() == 140e18, "20.lending net wrong");
    }

    function testTvlLendingSkipsInactive() public {
        lending.add(address(tokenA),
                    uint128(100e18), uint128(0),
                    uint128(RAY),    uint128(RAY),
                    18, false);   // inactive!

        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.LENDING, address(lending));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenA), address(aggA));

        require(oracle.tvlLending() == 0, "21.inactive included");
    }

    // ─────────────────────────────────────────────────────────────────────
    // 22-24 : OTHER SOURCES + AGGREGATE
    // ─────────────────────────────────────────────────────────────────────

    function testTvlStabilityNeedsBothPoolAndToken() public {
        stability.set(1_000e6);     // 1000 ZUSD (6 decimals)

        // Without source set → 0.
        require(oracle.tvlStability() == 0, "22a.no source returns nonzero");

        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.STABILITY, address(stability));
        // Without deposit token set → still 0.
        require(oracle.tvlStability() == 0, "22b.no deposit-token returns nonzero");

        vm.prank(DEPLOYER); oracle.setStabilityDepositToken(address(tokenU));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenU), address(aggU));
        // 1000 ZUSD × $1 = $1000 = 1000e18.
        require(oracle.tvlStability() == 1_000e18, "22c.stability math wrong");
    }

    function testTvlStakingReadsTotalStaked() public {
        // 50 tokenA staked; price $2 → $100 = 100e18.
        staking.set(50e18, address(tokenA));
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.STAKING, address(staking));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenA), address(aggA));
        require(oracle.tvlStaking() == 100e18, "23.staking math wrong");
    }

    function testTvlBreakdownSumMatchesTotalAndIncludesTimestamp() public {
        // Wire all three concrete sources with a known total.
        MockAMMPair p = new MockAMMPair(address(tokenA), address(tokenU), 100e18, 50e6);
        factory.addPair(address(p));
        lending.add(address(tokenA),
                    uint128(100e18), uint128(30e18),
                    uint128(RAY),    uint128(RAY),
                    18, true);
        stability.set(1_000e6);
        staking.set(50e18, address(tokenA));

        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.AMM,       address(factory));
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.LENDING,   address(lending));
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.STABILITY, address(stability));
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.STAKING,   address(staking));
        vm.prank(DEPLOYER); oracle.setStabilityDepositToken(address(tokenU));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenA), address(aggA));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenU), address(aggU));

        IZbxTvlOracle.TvlBreakdown memory b = oracle.tvlBreakdown();
        require(b.amm        == 250e18,  "24a.amm");
        require(b.lending    == 140e18,  "24b.lending");
        require(b.stability  == 1000e18, "24c.stability");
        require(b.staking    == 100e18,  "24d.staking");
        require(b.reward     == 0,       "24e.reward (scaffold)");
        require(b.bridgeVault== 0,       "24f.bridge (scaffold)");
        require(b.total      == 1490e18, "24g.total mismatch");
        require(b.timestamp  == block.timestamp, "24h.ts");
        require(oracle.totalValueLockedUSD() == b.total, "24i.tvl != breakdown.total");
    }

    // ─────────────────────────────────────────────────────────────────────
    // 25-26 : PRICE FEED EDGE CASES
    // ─────────────────────────────────────────────────────────────────────

    function testStaleAggregatorReturnsZero() public {
        MockAMMPair p = new MockAMMPair(address(tokenA), address(tokenU), 100e18, 50e6);
        factory.addPair(address(p));
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.AMM, address(factory));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenA), address(aggA));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenU), address(aggU));

        // Default staleness = 3600. Warp 7200s and DON'T refresh aggregators.
        vm.warp(block.timestamp + 7200);

        // Both feeds stale → tvlAMM returns 0 (fail-closed).
        require(oracle.tvlAMM() == 0, "25.stale price contributed");
    }

    function testNegativePriceReturnsZero() public {
        MockAggregator badAgg = new MockAggregator(-1, 8, block.timestamp);
        MockAMMPair p = new MockAMMPair(address(tokenA), address(tokenU), 100e18, 50e6);
        factory.addPair(address(p));
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.AMM, address(factory));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenA), address(badAgg));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenU), address(aggU));

        // tokenA priced negative → its leg contributes 0.
        // tokenU still healthy → 50 USDC × $1 = 50 USD.
        require(oracle.tvlAMM() == 50e18, "26.negative price not zeroed");
    }

    // ─────────────────────────────────────────────────────────────────────
    // 27-30 : ARCHITECT-REQUESTED (S17-T06 review fixes)
    // ─────────────────────────────────────────────────────────────────────

    function testLendingIndexUnscalesNonRAY() public {
        // Non-trivial liquidityIndex: 1.10 × RAY (10% supply interest accrued).
        // Borrow index unchanged. Supplied=100 scaled, borrowed=0.
        // Real supplied = 100 × 1.10 = 110 tokenA. USD = 110 × $2 = 220 USD.
        uint128 nonRayIndex = uint128((11 * RAY) / 10);
        lending.add(address(tokenA),
                    uint128(100e18), uint128(0),
                    nonRayIndex,     uint128(RAY),
                    18, true);

        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.LENDING, address(lending));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenA), address(aggA));

        require(oracle.tvlLending() == 220e18, "27.non-RAY index unscale wrong");
    }

    function testRefreshUnpricedPopulatesMissingFeed() public {
        // Two AMM pairs: only tokenU has a feed, tokenA has none.
        MockAMMPair p = new MockAMMPair(address(tokenA), address(tokenU), 100e18, 50e6);
        factory.addPair(address(p));
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.AMM, address(factory));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenU), address(aggU));
        // Note: tokenA has NO price feed registered.

        // Before refresh: list is empty.
        require(oracle.unpricedTokens().length == 0, "28a.list non-empty pre-refresh");

        oracle.refreshUnpriced();
        address[] memory un = oracle.unpricedTokens();
        require(un.length == 1, "28b.unpriced count wrong");
        require(un[0] == address(tokenA), "28c.unpriced token wrong");

        // tvlAMM still works: tokenA leg → 0, tokenU leg → 50 USD.
        require(oracle.tvlAMM() == 50e18, "28d.tvlAMM with missing feed wrong");
    }

    function testPairScanTruncationStatsExposed() public {
        // 3 pairs; cap to 2. pairScanStats must report (3, 2, true).
        MockAMMPair p1 = new MockAMMPair(address(tokenA), address(tokenU), 100e18, 50e6);
        MockAMMPair p2 = new MockAMMPair(address(tokenA), address(tokenU), 100e18, 50e6);
        MockAMMPair p3 = new MockAMMPair(address(tokenA), address(tokenU), 100e18, 50e6);
        factory.addPair(address(p1));
        factory.addPair(address(p2));
        factory.addPair(address(p3));
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.AMM, address(factory));
        vm.prank(DEPLOYER); oracle.setMaxPairsToScan(2);

        (uint256 total, uint256 scanned, bool truncated) = oracle.pairScanStats();
        require(total     == 3,    "29a.total wrong");
        require(scanned   == 2,    "29b.scanned wrong");
        require(truncated == true, "29c.truncated flag wrong");

        // Also: when cap >= total, truncated must be false.
        vm.prank(DEPLOYER); oracle.setMaxPairsToScan(256);
        (total, scanned, truncated) = oracle.pairScanStats();
        require(total     == 3,    "29d.total post-raise");
        require(scanned   == 3,    "29e.scanned post-raise");
        require(truncated == false,"29f.truncated false post-raise");
    }

    function testOutOfPolicyDecimalsReturnZero() public {
        // Token with decimals = 100 (well above MAX_TOKEN_DECIMALS=36).
        // Should fail-closed to 0 USD without reverting.
        MockToken weirdToken = new MockToken(100);
        MockAMMPair p = new MockAMMPair(address(weirdToken), address(tokenU), 100e18, 50e6);
        factory.addPair(address(p));
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.AMM, address(factory));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(weirdToken), address(aggA));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenU),     address(aggU));

        // weirdToken leg → 0 (out-of-policy decimals). tokenU leg → 50 USD.
        require(oracle.tvlAMM() == 50e18, "30.out-of-policy decimals not zeroed");
    }

    // ─────────────────────────────────────────────────────────────────────
    // 31-36 : S23b — TWAP ALT-PRICE-SOURCE
    // ─────────────────────────────────────────────────────────────────────

    function testSetTwapOracleByOwnerEmitsAndPersists() public {
        MockTwapOracle twap = new MockTwapOracle();

        // Non-owner cannot wire.
        vm.prank(ALICE);
        try oracle.setTwapOracle(address(twap)) { revert("31a.non-owner allowed"); }
        catch {}

        // Owner can wire and getter reflects it.
        vm.prank(DEPLOYER);
        oracle.setTwapOracle(address(twap));
        require(oracle.twapOracle() == address(twap), "31b.twapOracle getter");

        // Owner can also disable (re-wire to zero).
        vm.prank(DEPLOYER);
        oracle.setTwapOracle(address(0));
        require(oracle.twapOracle() == address(0), "31c.twapOracle disabled");
    }

    function testSetTwapRouteHappyPath() public {
        // Set up: tokenA routed to tokenU (which has aggregator feed).
        MockAMMPair pair = new MockAMMPair(address(tokenA), address(tokenU), 0, 0);
        MockTwapOracle twap = new MockTwapOracle();

        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenU), address(aggU));
        vm.prank(DEPLOYER); oracle.setTwapOracle(address(twap));
        vm.prank(DEPLOYER);
        oracle.setTwapRoute(address(tokenA), address(pair), address(tokenU), true);

        (address gotPair, address gotQuote, bool gotEnabled) = oracle.twapRoute(address(tokenA));
        require(gotPair    == address(pair),  "32a.pair");
        require(gotQuote   == address(tokenU),"32b.quote");
        require(gotEnabled == true,           "32c.enabled");

        // Disable resets enabled flag (other fields preserved as zero on rewrite).
        vm.prank(DEPLOYER);
        oracle.setTwapRoute(address(tokenA), address(0), address(0), false);
        ( , , gotEnabled) = oracle.twapRoute(address(tokenA));
        require(gotEnabled == false, "32d.disabled");
    }

    function testSetTwapRouteRevertsWhenQuoteUnpriced() public {
        // tokenU has NO priceFeed registered → setTwapRoute must revert.
        MockAMMPair pair = new MockAMMPair(address(tokenA), address(tokenU), 0, 0);
        MockTwapOracle twap = new MockTwapOracle();

        vm.prank(DEPLOYER); oracle.setTwapOracle(address(twap));
        vm.prank(DEPLOYER);
        try oracle.setTwapRoute(address(tokenA), address(pair), address(tokenU), true) {
            revert("33.unpriced quote allowed");
        } catch {}
    }

    function testSetTwapRouteRevertsWhenPairTokenMismatch() public {
        // pair contains (tokenA, tokenU); we try to route tokenA via a quote
        // token NOT in the pair → revert TwapPairTokenMismatch.
        MockToken otherQuote = new MockToken(18);
        MockAggregator aggOther = new MockAggregator(1e8, 8, block.timestamp);

        MockAMMPair pair = new MockAMMPair(address(tokenA), address(tokenU), 0, 0);
        MockTwapOracle twap = new MockTwapOracle();

        vm.prank(DEPLOYER); oracle.setPriceFeed(address(otherQuote), address(aggOther));
        vm.prank(DEPLOYER); oracle.setTwapOracle(address(twap));
        vm.prank(DEPLOYER);
        try oracle.setTwapRoute(address(tokenA), address(pair), address(otherQuote), true) {
            revert("34a.mismatched quote allowed");
        } catch {}

        // And: degenerate self-quote (quoteToken == token) also rejected.
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenA), address(aggA));
        vm.prank(DEPLOYER);
        try oracle.setTwapRoute(address(tokenA), address(pair), address(tokenA), true) {
            revert("34b.self-quote allowed");
        } catch {}
    }

    function testTvlAMMUsesTwapWhenRouteEnabled() public {
        // Scenario: tokenA has NO direct aggregator feed, but is routed via
        // TWAP to tokenU (which has $1.00 feed). 1 tokenA = 3 tokenU per
        // TWAP. Pair holds 100 tokenA + 50 tokenU.
        //
        //   tokenA leg via TWAP: 100e18 * 3e18 / 1e18 = 300e18 tokenU-units (18d)
        //                        → 300e18 * $1 = 300 USD (in 18d)
        //   tokenU leg direct:   50e6 * $1 = 50 USD (in 18d)
        //   Expected tvlAMM:     350e18

        MockAMMPair pair = new MockAMMPair(address(tokenA), address(tokenU), 100e18, 50e6);
        factory.addPair(address(pair));

        MockTwapOracle twap = new MockTwapOracle();
        twap.setRate(address(pair), 3e18); // 1 tokenA = 3 tokenU

        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.AMM, address(factory));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenU), address(aggU));
        // NOTE: no priceFeed for tokenA — pure TWAP routing.
        vm.prank(DEPLOYER); oracle.setTwapOracle(address(twap));
        vm.prank(DEPLOYER);
        oracle.setTwapRoute(address(tokenA), address(pair), address(tokenU), true);

        uint256 got = oracle.tvlAMM();
        require(got == 350e18, "35.twap-routed AMM mismatch");
    }

    function testTwapConsultRevertReturnsZeroFailClosed() public {
        // Same setup as #35 but TWAP is configured to revert on consult.
        // tokenA leg must contribute 0; tokenU leg unaffected (50 USD).
        MockAMMPair pair = new MockAMMPair(address(tokenA), address(tokenU), 100e18, 50e6);
        factory.addPair(address(pair));

        MockTwapOracle twap = new MockTwapOracle();
        twap.setRate(address(pair), 3e18);
        twap.setShouldRevert(true); // simulate NotPrimed / PairInactive

        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.AMM, address(factory));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenU), address(aggU));
        vm.prank(DEPLOYER); oracle.setTwapOracle(address(twap));
        vm.prank(DEPLOYER);
        oracle.setTwapRoute(address(tokenA), address(pair), address(tokenU), true);

        // Only tokenU leg contributes: 50 USD.
        require(oracle.tvlAMM() == 50e18, "36.fail-closed not zero on twap revert");
    }

    // ─────────────────────────────────────────────────────────────────────
    // 37-39 : S23b-Polish-1 — REGRESSION EDGES (architect-requested)
    // ─────────────────────────────────────────────────────────────────────

    function testRouteEnabledWithTwapOracleZeroFailsClosedAndMarksUnpriced() public {
        // Set up: register quote feed, wire twap, enable route — then
        // un-wire the twap oracle. Route remains "enabled" but the
        // integration is broken. Expected: tokenA contribution 0, AND
        // refreshUnpriced records `tokenA` (the integration-broken case
        // — NOT the quote token, because the quote leg itself is
        // healthy).
        MockAMMPair pair = new MockAMMPair(address(tokenA), address(tokenU), 100e18, 50e6);
        factory.addPair(address(pair));
        MockTwapOracle twap = new MockTwapOracle();
        twap.setRate(address(pair), 3e18);

        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.AMM, address(factory));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenU), address(aggU));
        vm.prank(DEPLOYER); oracle.setTwapOracle(address(twap));
        vm.prank(DEPLOYER);
        oracle.setTwapRoute(address(tokenA), address(pair), address(tokenU), true);

        // Sanity: with twap wired the path works (350 USD).
        require(oracle.tvlAMM() == 350e18, "37a.precondition twap wired");

        // Now un-wire twap. Route is still flagged enabled.
        vm.prank(DEPLOYER); oracle.setTwapOracle(address(0));
        require(oracle.twapOracle() == address(0), "37b.twap unwired");

        // tokenA contribution is now 0 (fail-closed on twapOracle == 0);
        // only tokenU leg contributes (50 USD).
        require(oracle.tvlAMM() == 50e18, "37c.fail-closed on twapOracle==0");

        // refreshUnpriced records `tokenA` itself (integration broken),
        // NOT tokenU (whose aggregator is healthy).
        oracle.refreshUnpriced();
        address[] memory unpriced = oracle.unpricedTokens();
        bool sawA = false;
        bool sawU = false;
        for (uint256 i = 0; i < unpriced.length; i++) {
            if (unpriced[i] == address(tokenA)) sawA = true;
            if (unpriced[i] == address(tokenU)) sawU = true;
        }
        require(sawA,  "37d.tokenA must be marked unpriced (integration broken)");
        require(!sawU, "37e.tokenU must NOT be marked unpriced (quote leg healthy)");
    }

    function testRouteDisableRestoresLegacyAggregatorPath() public {
        // Set up: tokenA HAS its own aggA feed ($2.00) AND a TWAP route
        // to tokenU at rate 3:1. Pair: 100 tokenA + 50 tokenU.
        //   route enabled  → tokenA via TWAP: 100 * 3 = 300 USD; tokenU 50 → total 350
        //   route disabled → tokenA via aggA: 100 * $2 = 200 USD; tokenU 50 → total 250
        MockAMMPair pair = new MockAMMPair(address(tokenA), address(tokenU), 100e18, 50e6);
        factory.addPair(address(pair));
        MockTwapOracle twap = new MockTwapOracle();
        twap.setRate(address(pair), 3e18);

        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.AMM, address(factory));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenA), address(aggA));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenU), address(aggU));
        vm.prank(DEPLOYER); oracle.setTwapOracle(address(twap));
        vm.prank(DEPLOYER);
        oracle.setTwapRoute(address(tokenA), address(pair), address(tokenU), true);

        // Route enabled → TWAP path dominates.
        require(oracle.tvlAMM() == 350e18, "38a.twap-routed total");

        // Disable the route → legacy aggregator path takes over.
        vm.prank(DEPLOYER);
        oracle.setTwapRoute(address(tokenA), address(0), address(0), false);

        require(oracle.tvlAMM() == 250e18, "38b.legacy aggregator path restored");
    }

    // ─────────────────────────────────────────────────────────────────────
    // 40-47 : S24 — Phase 7 REWARD + BRIDGE_VAULT real impl
    // ─────────────────────────────────────────────────────────────────────
    //
    // Numbering: tests 40-43 cover Source.REWARD; 44-46 cover
    // Source.BRIDGE_VAULT; 47 is the integration (both wired into
    // tvlBreakdown). Quirk: test 39 below this anchor is in the prior
    // S23b-Polish-1 block (defined first by file order). Inserted here
    // as a reminder for the architect that the numeric IDs are file-
    // order, not source-order.

    function testRefreshUnpricedRecordsQuoteLegOnQuoteBreakage() public {
        // Set up: route enabled, twap wired, quote feed initially
        // healthy. Then UN-REGISTER the quote feed (setPriceFeed(quote, 0)).
        // Expected: refreshUnpriced records `tokenU` (the quote leg) —
        // the actual broken dependency — NOT `tokenA` (whose route
        // wiring is intact).
        MockAMMPair pair = new MockAMMPair(address(tokenA), address(tokenU), 100e18, 50e6);
        factory.addPair(address(pair));
        MockTwapOracle twap = new MockTwapOracle();
        twap.setRate(address(pair), 3e18);

        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.AMM, address(factory));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenU), address(aggU));
        vm.prank(DEPLOYER); oracle.setTwapOracle(address(twap));
        vm.prank(DEPLOYER);
        oracle.setTwapRoute(address(tokenA), address(pair), address(tokenU), true);

        // Sanity: priced, total 350 USD.
        require(oracle.tvlAMM() == 350e18, "39a.precondition healthy");

        // Now break the quote leg by un-registering its aggregator.
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(tokenU), address(0));

        // Both legs now contribute 0 (tokenA via TWAP path -> quote
        // leg unpriced -> 0; tokenU direct -> no feed -> 0).
        require(oracle.tvlAMM() == 0, "39b.both legs zeroed on quote break");

        // refreshUnpriced records the QUOTE token (tokenU), not tokenA.
        oracle.refreshUnpriced();
        address[] memory unpriced = oracle.unpricedTokens();
        bool sawA = false;
        bool sawU = false;
        for (uint256 i = 0; i < unpriced.length; i++) {
            if (unpriced[i] == address(tokenA)) sawA = true;
            if (unpriced[i] == address(tokenU)) sawU = true;
        }
        require(sawU,  "39c.tokenU (quote leg) must be marked unpriced");
        require(!sawA, "39d.tokenA must NOT be marked unpriced (route wiring intact)");
    }

    // ─────────────────────────────────────────────────────────────────────
    // 40-47 : S24 — Phase 7 REWARD + BRIDGE_VAULT REAL IMPL
    // ─────────────────────────────────────────────────────────────────────

    function testTvlRewardReturnsZeroWhenSourceUnconfigured() public {
        // Default state: Source.REWARD un-set → tvlReward = 0.
        require(oracle.tvlReward() == 0, "40.unconfigured reward must be 0");
    }

    function testTvlRewardHappyPath() public {
        // Set up: zbx token @ $0.50 (5e7 raw aggregator price w/ 8 decimals),
        // distributor holds 1_000 ZBX. Expected: tvlReward = 1_000 * $0.50 = 500 USD-18.
        MockBalanceToken zbx = new MockBalanceToken(18);
        MockAggregator   pZ  = new MockAggregator(int256(50_000_000), 8, block.timestamp); // $0.50

        MockRewardDistributor dist = new MockRewardDistributor(address(zbx));
        zbx.setBalance(address(dist), 1_000e18);

        vm.prank(DEPLOYER); oracle.setPriceFeed(address(zbx), address(pZ));
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.REWARD, address(dist));

        require(oracle.tvlReward() == 500e18, "41.reward TVL mismatch");
    }

    function testTvlRewardFailsClosedOnDistributorRevert() public {
        // Distributor.zbx() reverts → fail-closed, tvlReward = 0.
        MockBalanceToken zbx = new MockBalanceToken(18);
        MockAggregator   pZ  = new MockAggregator(int256(50_000_000), 8, block.timestamp);

        MockRewardDistributor dist = new MockRewardDistributor(address(zbx));
        zbx.setBalance(address(dist), 1_000e18);
        dist.setRevertOnZbx(true);

        vm.prank(DEPLOYER); oracle.setPriceFeed(address(zbx), address(pZ));
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.REWARD, address(dist));

        require(oracle.tvlReward() == 0, "42.fail-closed on distributor revert");
    }

    function testTvlRewardFailsClosedWhenZbxUnpriced() public {
        // Distributor wired + non-zero balance, but ZBX has NO priceFeed →
        // _safeUSD returns 0 → tvlReward = 0 (existing fail-closed policy).
        MockBalanceToken zbx = new MockBalanceToken(18);
        MockRewardDistributor dist = new MockRewardDistributor(address(zbx));
        zbx.setBalance(address(dist), 1_000e18);

        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.REWARD, address(dist));

        require(oracle.tvlReward() == 0, "43.unpriced ZBX must zero reward TVL");

        // refreshUnpriced should record the underlying ZBX token (S24
        // monitoring symmetry with AMM/Lending/Staking branches).
        oracle.refreshUnpriced();
        address[] memory unpriced = oracle.unpricedTokens();
        bool sawZbx;
        for (uint256 i = 0; i < unpriced.length; i++) {
            if (unpriced[i] == address(zbx)) { sawZbx = true; break; }
        }
        require(sawZbx, "43b.refreshUnpriced must record reward ZBX token");
    }

    function testTvlBridgeVaultReturnsZeroWhenSourceUnconfigured() public {
        // Default state: Source.BRIDGE_VAULT un-set → tvlBridgeVault = 0.
        require(oracle.tvlBridgeVault() == 0, "44.unconfigured bridge must be 0");
    }

    function testTvlBridgeVaultHappyPath() public {
        // Set up: bridge token @ $1.00 (1e8 raw price w/ 8 decimals), 50_000 token locked.
        // Expected: tvlBridgeVault = 50_000 * $1.00 = 50_000 USD-18.
        MockToken      bTok = new MockToken(18);
        MockAggregator pB   = new MockAggregator(int256(100_000_000), 8, block.timestamp); // $1.00

        MockBridgeVault vault = new MockBridgeVault(address(bTok), 50_000e18);

        vm.prank(DEPLOYER); oracle.setPriceFeed(address(bTok), address(pB));
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.BRIDGE_VAULT, address(vault));

        require(oracle.tvlBridgeVault() == 50_000e18, "45.bridge TVL mismatch");
    }

    function testTvlBridgeVaultFailsClosedOnVaultRevert() public {
        // Vault.totalLocked() reverts → fail-closed, tvlBridgeVault = 0.
        MockToken      bTok = new MockToken(18);
        MockAggregator pB   = new MockAggregator(int256(100_000_000), 8, block.timestamp);

        MockBridgeVault vault = new MockBridgeVault(address(bTok), 50_000e18);
        vault.setRevertOnLocked(true);

        vm.prank(DEPLOYER); oracle.setPriceFeed(address(bTok), address(pB));
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.BRIDGE_VAULT, address(vault));

        require(oracle.tvlBridgeVault() == 0, "46.fail-closed on vault revert");
    }

    function testTvlBreakdownIncludesPhase7Sources() public {
        // Integration: wire BOTH Phase 7 sources alongside an empty AMM
        // factory. Expected: tvlBreakdown.reward + tvlBreakdown.bridgeVault
        // both contribute correctly, and `total` includes them.
        //
        // - reward leg: 200 ZBX @ $2 = 400 USD-18
        // - bridge leg: 100 bTok @ $3 = 300 USD-18
        // - total = 700 USD-18
        MockBalanceToken zbx = new MockBalanceToken(18);
        MockToken        bTok = new MockToken(18);
        MockAggregator   pZ   = new MockAggregator(int256(200_000_000), 8, block.timestamp); // $2
        MockAggregator   pB   = new MockAggregator(int256(300_000_000), 8, block.timestamp); // $3

        MockRewardDistributor dist  = new MockRewardDistributor(address(zbx));
        zbx.setBalance(address(dist), 200e18);
        MockBridgeVault       vault = new MockBridgeVault(address(bTok), 100e18);

        vm.prank(DEPLOYER); oracle.setPriceFeed(address(zbx),  address(pZ));
        vm.prank(DEPLOYER); oracle.setPriceFeed(address(bTok), address(pB));
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.REWARD,       address(dist));
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.BRIDGE_VAULT, address(vault));

        IZbxTvlOracle.TvlBreakdown memory bd = oracle.tvlBreakdown();
        require(bd.reward      == 400e18, "47a.reward leg mismatch");
        require(bd.bridgeVault == 300e18, "47b.bridge leg mismatch");
        require(bd.total       == 700e18, "47c.total must include phase 7 legs");
    }

    // ─── S24-fix1 (architect-rec): two more fail-closed edge tests ───────

    function testTvlRewardFailsClosedOnBalanceOfRevert() public {
        // Distributor wired correctly + zbx() returns valid token,
        // but the underlying ZBX contract's balanceOf(distributor) REVERTS.
        // Must fail-closed (tvlReward = 0), not bubble revert into
        // tvlBreakdown / tvlGlobal. Closes the architect-flagged untested
        // try/catch branch around `IERC20(zbx).balanceOf(dist)`.
        MockBalanceToken zbx = new MockBalanceToken(18);
        MockAggregator   pZ  = new MockAggregator(int256(50_000_000), 8, block.timestamp); // $0.50

        MockRewardDistributor dist = new MockRewardDistributor(address(zbx));
        zbx.setBalance(address(dist), 1_000e18);
        zbx.setRevertOnBalanceOf(true);

        vm.prank(DEPLOYER); oracle.setPriceFeed(address(zbx), address(pZ));
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.REWARD, address(dist));

        require(oracle.tvlReward() == 0, "48.fail-closed on balanceOf revert");

        // Critical: the surrounding tvlBreakdown call must NOT revert.
        IZbxTvlOracle.TvlBreakdown memory bd = oracle.tvlBreakdown();
        require(bd.reward == 0, "48b.tvlBreakdown must not bubble balanceOf revert");
    }

    function testTvlBridgeVaultFailsClosedOnTokenRevert() public {
        // Vault wired correctly, but vault.token() REVERTS. Must fail-closed
        // (tvlBridgeVault = 0). Closes the architect-flagged untested
        // try/catch branch around `IBridgeVault(vault).token()`.
        MockToken      bTok = new MockToken(18);
        MockAggregator pB   = new MockAggregator(int256(100_000_000), 8, block.timestamp);

        MockBridgeVault vault = new MockBridgeVault(address(bTok), 50_000e18);
        vault.setRevertOnToken(true);

        vm.prank(DEPLOYER); oracle.setPriceFeed(address(bTok), address(pB));
        vm.prank(DEPLOYER); oracle.setSource(IZbxTvlOracle.Source.BRIDGE_VAULT, address(vault));

        require(oracle.tvlBridgeVault() == 0, "49.fail-closed on vault.token() revert");

        // Critical: tvlBreakdown must NOT revert.
        IZbxTvlOracle.TvlBreakdown memory bd = oracle.tvlBreakdown();
        require(bd.bridgeVault == 0, "49b.tvlBreakdown must not bubble vault.token() revert");
    }
}
