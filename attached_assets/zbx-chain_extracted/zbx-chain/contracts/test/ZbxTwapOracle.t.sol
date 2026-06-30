// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { ZbxTwapOracle }  from "../ZbxTwapOracle.sol";
import { IZbxTwapOracle } from "../interfaces/IZbxTwapOracle.sol";

// =============================================================================
// HEVM cheatcode interface
// =============================================================================

interface Vm {
    function warp(uint256) external;
    function expectRevert() external;
    function expectRevert(bytes4 selector) external;
    function expectRevert(bytes calldata data) external;
    function prank(address) external;
    function startPrank(address) external;
    function stopPrank() external;
}

// =============================================================================
// MOCKS
// =============================================================================

/// @dev Mirrors the subset of `ZbxAMM` that ZbxTwapOracle reads.
///      `setReserves` ports `ZbxAMM._update` so the cumulative slots
///      evolve identically to a real pair.
contract MockPair {
    address public token0;
    address public token1;
    uint112 private _r0;
    uint112 private _r1;
    uint32  private _ts;
    uint256 public price0CumulativeLast;
    uint256 public price1CumulativeLast;

    constructor(address t0, address t1) {
        require(t0 < t1, "MockPair: token0 < token1");
        token0 = t0;
        token1 = t1;
        _ts    = uint32(block.timestamp);
    }

    function getReserves() external view returns (uint112, uint112, uint32) {
        return (_r0, _r1, _ts);
    }

    /// @dev Test helper: set reserves at the current block.timestamp,
    ///      accumulating cumulative-price slots like ZbxAMM._update would.
    function setReserves(uint112 r0, uint112 r1) external {
        uint32 nowTs = uint32(block.timestamp);
        if (nowTs != _ts && _r0 != 0 && _r1 != 0) {
            uint32 dt;
            unchecked { dt = nowTs - _ts; }
            unchecked {
                price0CumulativeLast += (uint256(_r1) << 112) / _r0 * dt;
                price1CumulativeLast += (uint256(_r0) << 112) / _r1 * dt;
            }
        }
        _r0 = r0;
        _r1 = r1;
        _ts = nowTs;
    }
}

// =============================================================================
// TESTS — S23a-fix1 cached-window semantics
// =============================================================================

contract ZbxTwapOracleTest {
    Vm constant vm = Vm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);

    ZbxTwapOracle internal oracle;
    MockPair      internal pair;

    address internal constant TOKEN_A = address(0x000000000000000000000000000000000000a000);
    address internal constant TOKEN_B = address(0x000000000000000000000000000000000000b000);
    address internal constant ALICE   = address(0xA11CE);

    function setUp() public {
        // Anchor block.timestamp at a non-zero value so uint32 timestamps
        // stay in sane range and we don't accidentally hit any sentinel.
        vm.warp(1_700_000_000);   // ~ 2023-11-14

        oracle = new ZbxTwapOracle();
        pair   = new MockPair(TOKEN_A, TOKEN_B);

        // Seed the pair with reserves: 1000 token0 = 1000 token1 (1:1 price).
        pair.setReserves(1000 ether, 1000 ether);
    }

    // ─── 1. registerPair: only owner ─────────────────────────────────────

    function test_RegisterPair_RevertsWhen_NotOwner() public {
        vm.prank(ALICE);
        vm.expectRevert();   // Ownable2Step: NotOwner — selector inherited from base
        oracle.registerPair(address(pair), 0);
    }

    // ─── 2. registerPair: zero address rejected ──────────────────────────

    function test_RegisterPair_RevertsWhen_ZeroPair() public {
        vm.expectRevert(IZbxTwapOracle.ZeroPair.selector);
        oracle.registerPair(address(0), 0);
    }

    // ─── 3. registerPair: period bounds enforced ─────────────────────────

    function test_RegisterPair_RevertsWhen_PeriodTooShort() public {
        vm.expectRevert(
            abi.encodeWithSelector(
                IZbxTwapOracle.PeriodOutOfBounds.selector,
                uint32(60), uint32(5 minutes), uint32(24 hours)
            )
        );
        oracle.registerPair(address(pair), 60);
    }

    function test_RegisterPair_RevertsWhen_PeriodTooLong() public {
        vm.expectRevert(
            abi.encodeWithSelector(
                IZbxTwapOracle.PeriodOutOfBounds.selector,
                uint32(48 hours), uint32(5 minutes), uint32(24 hours)
            )
        );
        oracle.registerPair(address(pair), 48 hours);
    }

    // ─── 4. registerPair: zero period uses DEFAULT_PERIOD ────────────────

    function test_RegisterPair_DefaultPeriodWhen_Zero() public {
        oracle.registerPair(address(pair), 0);
        (uint32 period, bool active, bool primed) = oracle.pairConfig(address(pair));
        require(period == 30 minutes, "default period not 30 min");
        require(active, "pair not active after register");
        require(!primed, "primed must be false at register (no window matured)");
    }

    // ─── 5. registerPair: rejects double-register ────────────────────────

    function test_RegisterPair_RevertsWhen_AlreadyRegistered() public {
        oracle.registerPair(address(pair), 0);
        vm.expectRevert(
            abi.encodeWithSelector(IZbxTwapOracle.PairAlreadyRegistered.selector, address(pair))
        );
        oracle.registerPair(address(pair), 0);
    }

    // ─── 6. registerPair: seeds baseline observation ─────────────────────

    function test_RegisterPair_SeedsBaselineObservation() public {
        oracle.registerPair(address(pair), 0);
        (uint32 ts, uint256 cum0, uint256 cum1) = oracle.lastObservation(address(pair));
        require(ts == uint32(block.timestamp), "seeded ts mismatch");
        require(cum0 == 0 && cum1 == 0, "seeded cum should be 0 at register");

        // cachedAvg must be all zero (not yet meaningful).
        (uint256 a0, uint256 a1) = oracle.cachedAvg(address(pair));
        require(a0 == 0 && a1 == 0, "cachedAvg must be 0 before first update");
    }

    // ─── 7. update: returns false within period (no overwrite) ───────────

    function test_Update_ReturnsFalseWithin_Period() public {
        oracle.registerPair(address(pair), 30 minutes);
        (uint32 tsBefore,,) = oracle.lastObservation(address(pair));

        vm.warp(block.timestamp + 10 minutes);
        bool committed = oracle.update(address(pair));
        require(!committed, "update should NOT commit before period elapses");

        (uint32 tsAfter,,) = oracle.lastObservation(address(pair));
        require(tsAfter == tsBefore, "observation should NOT have been overwritten");

        (,, bool primed) = oracle.pairConfig(address(pair));
        require(!primed, "primed must remain false after no-op update");
    }

    // ─── 8. update: commits + primes after period elapses ────────────────

    function test_Update_CommitsAfter_Period_AndPrimes() public {
        oracle.registerPair(address(pair), 30 minutes);

        vm.warp(block.timestamp + 31 minutes);
        bool committed = oracle.update(address(pair));
        require(committed, "update should commit after period elapses");

        (uint32 ts,,) = oracle.lastObservation(address(pair));
        require(ts == uint32(block.timestamp), "fresh observation ts mismatch");

        (,, bool primed) = oracle.pairConfig(address(pair));
        require(primed, "primed must flip true after first successful commit");
    }

    // ─── 9. update: reverts if pair inactive ─────────────────────────────

    function test_Update_RevertsWhen_PairInactive() public {
        vm.expectRevert(
            abi.encodeWithSelector(IZbxTwapOracle.PairInactive.selector, address(pair))
        );
        oracle.update(address(pair));
    }

    // ─── 10. consult: reverts NotPrimed before any window matures ────────

    function test_Consult_RevertsWhen_NotPrimed() public {
        oracle.registerPair(address(pair), 30 minutes);
        // Did NOT call update() yet → primed = false.
        vm.expectRevert(
            abi.encodeWithSelector(IZbxTwapOracle.NotPrimed.selector, address(pair))
        );
        oracle.consult(address(pair), TOKEN_A, 1 ether);
    }

    // ─── 11. consult: happy path returns 1:1 price after one window ──────
    //
    // Setup: 1000:1000 reserves (1:1 price). Register, warp period, update,
    // consult → expect 1 ether out for 1 ether in.

    function test_Consult_HappyPath_OneToOne() public {
        oracle.registerPair(address(pair), 30 minutes);

        // Warp full period so cumulative grows: cum0 += 1*1800 = 1800<<112.
        vm.warp(block.timestamp + 30 minutes);

        bool committed = oracle.update(address(pair));
        require(committed, "first update should commit");

        // Now consult: cached priceAvg = (1800<<112 - 0) / 1800 = 1<<112.
        // amountOut = (1<<112 * 1 ether) >> 112 = 1 ether.
        uint256 out = oracle.consult(address(pair), TOKEN_A, 1 ether);
        require(out == 1 ether, "1:1 TWAP wrong");
    }

    // ─── 12. consult: stable across time after priming ───────────────────
    //
    // Once primed, consult returns the SAME value at any query time
    // until the next successful update commits a new window.

    function test_Consult_StableAcrossTime_AfterPriming() public {
        oracle.registerPair(address(pair), 30 minutes);
        vm.warp(block.timestamp + 30 minutes);
        oracle.update(address(pair));

        uint256 out1 = oracle.consult(address(pair), TOKEN_A, 1 ether);

        vm.warp(block.timestamp + 5 minutes);
        uint256 out2 = oracle.consult(address(pair), TOKEN_A, 1 ether);

        vm.warp(block.timestamp + 20 minutes);
        uint256 out3 = oracle.consult(address(pair), TOKEN_A, 1 ether);

        require(out1 == out2 && out2 == out3, "consult must be stable between updates");
    }

    // ─── 13. consult: cached-window manipulation resistance ──────────────
    //
    // S23a-fix1 architectural guarantee: a flash-style spike that lasts
    // a fraction of `period` contributes at most `spike_duration / period`
    // weight to the next cached priceAvg. With a 30-min period and a
    // 60-second spike (5 EVM blocks at 12-sec block time), spike weight
    // ≤ 60/1800 ≈ 3.3 %.

    function test_Consult_CachedWindow_ManipulationResistance() public {
        oracle.registerPair(address(pair), 30 minutes);

        // Warp to T = 1800. cum has accumulated 1.0 × 1800 (no spike).
        vm.warp(block.timestamp + 30 minutes);
        oracle.update(address(pair));   // primes cache at priceAvg = 1.0

        // Window 2: 30 min, but attacker spikes for 60 sec mid-window.
        // Sub-window A: 1740 sec at price 1.0 (1000:1000 reserves).
        vm.warp(block.timestamp + 1740);
        // pair untouched — _update happens lazily on next swap or sync.

        // Attacker dumps token0 → reserves 10000:100. Spot price token0
        // ↓ to 0.01 token1. Pair's _update accumulates PRE-spike ratio
        // × 1740 sec into cum.
        pair.setReserves(10_000 ether, 100 ether);

        // Spike held for 60 sec.
        vm.warp(block.timestamp + 60);

        // Attacker reverses: pair._update accumulates spike ratio (0.01)
        // × 60 sec into cum. Then reserves restore to 1000:1000.
        pair.setReserves(1000 ether, 1000 ether);

        // Update at T + 30 min: window 2 closes, new cache committed.
        bool committed = oracle.update(address(pair));
        require(committed, "second update should commit");

        // Expected priceAvg over window 2:
        //   cum delta = (1.0 × 1740) + (0.01 × 60) = 1740.6 [in ratio×sec units]
        //   priceAvg  = 1740.6 / 1800 ≈ 0.9670 (UQ112x112: ≈ 0.967 << 112)
        // Consult 1 ether → ≈ 0.967 ether out.
        uint256 out = oracle.consult(address(pair), TOKEN_A, 1 ether);
        // Manipulation defence: TWAP must be MUCH closer to pre-spike (1.0)
        // than to post-spike spot (0.01). Tolerance: within 4 % of 1.0.
        require(out > 0.96 ether,  "TWAP collapsed toward spot — manipulation defence broken");
        require(out < 1.00 ether,  "TWAP overshot pre-spike (spike not weighted in)");
    }

    // ─── 14. consult: tokenIn must be in pair ────────────────────────────

    function test_Consult_RevertsWhen_TokenNotInPair() public {
        oracle.registerPair(address(pair), 30 minutes);
        vm.warp(block.timestamp + 30 minutes);
        oracle.update(address(pair));

        address bogus = address(0xDEAD);
        vm.expectRevert(
            abi.encodeWithSelector(IZbxTwapOracle.TokenNotInPair.selector, bogus, address(pair))
        );
        oracle.consult(address(pair), bogus, 1 ether);
    }

    // ─── 15. deactivate: blocks further consult/update ───────────────────

    function test_Deactivate_BlocksConsultAndUpdate() public {
        oracle.registerPair(address(pair), 30 minutes);
        vm.warp(block.timestamp + 30 minutes);
        oracle.update(address(pair));
        oracle.deactivatePair(address(pair));

        vm.expectRevert(
            abi.encodeWithSelector(IZbxTwapOracle.PairInactive.selector, address(pair))
        );
        oracle.consult(address(pair), TOKEN_A, 1 ether);

        vm.expectRevert(
            abi.encodeWithSelector(IZbxTwapOracle.PairInactive.selector, address(pair))
        );
        oracle.update(address(pair));
    }

    // ─── 16. setPeriod: only owner + bounds + only-registered ────────────

    function test_SetPeriod_RevertsWhen_NotOwner() public {
        oracle.registerPair(address(pair), 30 minutes);
        vm.prank(ALICE);
        vm.expectRevert();
        oracle.setPeriod(address(pair), 1 hours);
    }

    function test_SetPeriod_UpdatesPeriod() public {
        oracle.registerPair(address(pair), 30 minutes);
        oracle.setPeriod(address(pair), 1 hours);
        (uint32 period,,) = oracle.pairConfig(address(pair));
        require(period == 1 hours, "period not updated");
    }

    function test_SetPeriod_RevertsWhen_NotRegistered() public {
        vm.expectRevert(
            abi.encodeWithSelector(IZbxTwapOracle.PairNotRegistered.selector, address(pair))
        );
        oracle.setPeriod(address(pair), 1 hours);
    }

    // ─── 17. EIP-165: advertises IZbxTwapOracle ──────────────────────────

    function test_EIP165_AdvertisesIZbxTwapOracle() public view {
        require(oracle.supportsInterface(type(IZbxTwapOracle).interfaceId),
                "must advertise IZbxTwapOracle");
        require(oracle.supportsInterface(0x01ffc9a7), "must advertise EIP-165 itself");
        require(!oracle.supportsInterface(0xffffffff), "must reject 0xffffffff");
    }

    // ─── 18. setPeriod INCREASE invalidates cache (S23a-fix2 MED-1) ──────
    //
    // Architect's new MED-1: if owner increases period after a successful
    // update, the cached priceAvg from the SHORTER prior window no longer
    // satisfies the new "window >= period" invariant. The fix is to set
    // primed = false on increase, forcing consult to revert NotPrimed
    // until the next update commits a fresh window of the new length.

    function test_SetPeriod_InvalidatesCacheOnIncrease() public {
        oracle.registerPair(address(pair), 30 minutes);
        vm.warp(block.timestamp + 30 minutes);
        oracle.update(address(pair));   // primes cache at 30-min window

        // Sanity: consult works pre-increase.
        uint256 outBefore = oracle.consult(address(pair), TOKEN_A, 1 ether);
        require(outBefore == 1 ether, "pre-increase consult should return 1:1");

        // Owner increases period to 1 hour.
        oracle.setPeriod(address(pair), 1 hours);

        // Cache MUST now be invalidated; primed flag flipped false.
        (,, bool primed) = oracle.pairConfig(address(pair));
        require(!primed, "primed must be false after period increase");

        // consult MUST revert NotPrimed until next update commits a
        // fresh ≥ 1-hour window.
        vm.expectRevert(
            abi.encodeWithSelector(IZbxTwapOracle.NotPrimed.selector, address(pair))
        );
        oracle.consult(address(pair), TOKEN_A, 1 ether);

        // Re-priming after the new (longer) period elapses re-enables consult.
        vm.warp(block.timestamp + 1 hours);
        oracle.update(address(pair));
        (,, bool primedAfter) = oracle.pairConfig(address(pair));
        require(primedAfter, "primed must flip true after fresh long-window commit");

        uint256 outAfter = oracle.consult(address(pair), TOKEN_A, 1 ether);
        require(outAfter == 1 ether, "post-reprime consult should return 1:1");
    }

    // ─── 19. setPeriod DECREASE preserves cache (S23a-fix2) ──────────────
    //
    // A longer-window TWAP still satisfies a shorter-window requirement,
    // so period decrease MUST NOT invalidate the cache. consult should
    // remain available immediately after the decrease.

    function test_SetPeriod_PreservesCacheOnDecrease() public {
        oracle.registerPair(address(pair), 1 hours);
        vm.warp(block.timestamp + 1 hours);
        oracle.update(address(pair));   // primes cache at 1-hour window

        // Owner decreases period to 30 min.
        oracle.setPeriod(address(pair), 30 minutes);

        // Cache MUST be preserved; primed stays true.
        (,, bool primed) = oracle.pairConfig(address(pair));
        require(primed, "primed must stay true after period decrease");

        // consult MUST keep working with the existing (longer-window) cache.
        uint256 out = oracle.consult(address(pair), TOKEN_A, 1 ether);
        require(out == 1 ether, "post-decrease consult should still return 1:1");
    }
}
