// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

// ─────────────────────────────────────────────────────────────────────────────
// ZusdVaultRedemption.t.sol — Foundry tests for the S15-P2 secure redeem().
//
//   Run:    forge test --match-contract ZusdVaultRedemptionTest -vvv
//   Goal:   Validate the S6-V2 fix (atomic per-CDP record updates) and all
//           safety properties documented in ZEP-005-ZUSD-REDEMPTION.md.
//
//   Tests are PURE Solidity require()-style (matching ZUSD.t.sol).
//   The `IVm.warp` cheatcode is used only by the S15-P2-D elapsed-time
//   tests (testRepay/Close/Redeem/LiquidateAfterFeeAccrual) via the
//   standard HEVM cheatcode address — no forge-std import required.
//
//   COVERAGE (21 tests):
//     1.  Happy-path: full single-CDP redemption
//     2.  Happy-path: partial single-CDP redemption (CR increases)
//     3.  Happy-path: multi-CDP cascade in ascending-CR order
//     4.  Vault-solvency invariant after multiple redemptions
//     5.  Reverts on non-monotone hints
//     6.  Reverts on unhealthy CDP (CR < 100%)
//     7.  Reverts when paused
//     8.  Reverts on below-min ZUSD amount
//     9.  Reverts on insufficient ZUSD balance
//    10.  Reverts on bad iteration bound (0 or > 50)
//    11.  0.5% fee correctly applied and routed to feeRecipient
//    12.  Dust-protection: leaves >= MIN_ZUSD_MINT or fully closes CDP
//    13.  Strict totalDebt invariant under M2 atomic update (S15-P2-A)
//    14.  Sub-min cdp.debt produced by repay still redeemable (S15-P2-A)
//    15.  Globally-omitted lower-CR CDP still redeemable (S15-P2-A fairness)
//    16.  repay() after fee accrual — no false invariant revert (S15-P2-D)
//    17.  closeCDP() after fee accrual — no false invariant revert (S15-P2-D)
//    18.  redeem() after fee accrual — no false invariant revert (S15-P2-D)
//    19.  liquidate() after fee accrual — no false invariant revert (S15-P2-D)
//    20.  mintMore() after fee accrual — totalDebt principal-delta correct (S15-P2-D-2)
//    21.  repay(amount < accruedFee) — totalDebt INCREASES per principal-delta (S15-P2-D-2)
// ─────────────────────────────────────────────────────────────────────────────

import "../ZUSD.sol";
import "../ZusdVault.sol";

// ─── HEVM cheatcode (no forge-std dep) ───────────────────────────────────────
//
// Only `vm.warp` is used (by S15-P2-D elapsed-time fee-accrual tests). The
// HEVM cheatcode address is the standard `keccak256("hevm cheat code")`
// truncated to 20 bytes — same value Foundry, Hevm, and Halmos all expose.

interface IVm {
    function warp(uint256) external;
}

address constant HEVM_ADDRESS =
    address(bytes20(uint160(uint256(keccak256("hevm cheat code")))));

IVm constant vm = IVm(HEVM_ADDRESS);

// ─── Test mocks ──────────────────────────────────────────────────────────────

contract MockZBX {
    string  public constant name     = "Mock ZBX";
    string  public constant symbol   = "ZBX";
    uint8   public constant decimals = 18;
    uint256 public totalSupply;
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    function mint(address to, uint256 amount) external {
        totalSupply   += amount;
        balanceOf[to] += amount;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        require(balanceOf[msg.sender] >= amount, "MockZBX: insufficient");
        balanceOf[msg.sender] -= amount;
        balanceOf[to]         += amount;
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        require(balanceOf[from] >= amount,                 "MockZBX: insufficient");
        require(allowance[from][msg.sender] >= amount,     "MockZBX: allowance");
        allowance[from][msg.sender] -= amount;
        balanceOf[from]             -= amount;
        balanceOf[to]               += amount;
        return true;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        return true;
    }
}

contract MockOracle {
    mapping(address => uint256) public prices;

    function setPrice(address asset, uint256 price) external {
        prices[asset] = price;
    }

    function getPrice(address asset) external view returns (uint256) {
        return prices[asset];
    }
}

/// @notice A "user" account that can be authenticated by msg.sender in the vault.
///         We use lightweight forwarder contracts so each call has a distinct
///         msg.sender, simulating multiple borrowers/redeemers.
contract Borrower {
    function callVault(address vault, bytes calldata data) external returns (bytes memory) {
        (bool ok, bytes memory ret) = vault.call(data);
        require(ok, "Borrower: vault call failed");
        return ret;
    }

    function callZusd(address zusd, bytes calldata data) external returns (bytes memory) {
        (bool ok, bytes memory ret) = zusd.call(data);
        require(ok, "Borrower: zusd call failed");
        return ret;
    }

    function callZbx(address zbx, bytes calldata data) external returns (bytes memory) {
        (bool ok, bytes memory ret) = zbx.call(data);
        require(ok, "Borrower: zbx call failed");
        return ret;
    }
}

// ─── Test contract ───────────────────────────────────────────────────────────

contract ZusdVaultRedemptionTest {

    ZUSD       zusd;
    MockZBX    zbx;
    MockOracle oracle;
    ZusdVault  vault;

    Borrower alice;
    Borrower bob;
    Borrower carol;
    Borrower redeemer;

    uint256 constant ZBX_PRICE_1USD = 1e18;  // 1 ZBX = $1 in initial setup

    function setUp() public {
        zusd   = new ZUSD();
        zbx    = new MockZBX();
        oracle = new MockOracle();
        vault  = new ZusdVault(address(zusd), address(zbx), address(oracle));

        zusd.setVault(address(vault));
        oracle.setPrice(address(zbx), ZBX_PRICE_1USD);

        alice    = new Borrower();
        bob      = new Borrower();
        carol    = new Borrower();
        redeemer = new Borrower();
    }

    // ─── helpers ─────────────────────────────────────────────────────────

    function _openCdpFor(Borrower b, uint256 collateral, uint256 debt) internal {
        zbx.mint(address(b), collateral);
        b.callZbx(
            address(zbx),
            abi.encodeWithSignature("approve(address,uint256)", address(vault), collateral)
        );
        b.callVault(
            address(vault),
            abi.encodeWithSignature("openCDP(uint256,uint256)", collateral, debt)
        );
    }

    function _giveZusd(Borrower b, uint256 amount, Borrower from) internal {
        from.callZusd(
            address(zusd),
            abi.encodeWithSignature("transfer(address,uint256)", address(b), amount)
        );
    }

    function _redeem(
        Borrower r,
        uint256 amount,
        address[] memory hints,
        uint256 maxIter
    ) internal returns (uint256 zusdRedeemed, uint256 zbxOut) {
        bytes memory ret = r.callVault(
            address(vault),
            abi.encodeWithSignature(
                "redeem(uint256,address[],uint256)",
                amount, hints, maxIter
            )
        );
        (zusdRedeemed, zbxOut) = abi.decode(ret, (uint256, uint256));
    }

    // ─── 1. Happy-path: full single-CDP redemption ───────────────────────

    function testFullSingleCdpRedemption() public {
        // Alice: 300 ZBX collateral, 100 ZUSD debt, CR = 300%
        _openCdpFor(alice, 300e18, 100e18);

        // Move 100 ZUSD to redeemer.
        _giveZusd(redeemer, 100e18, alice);

        address[] memory hints = new address[](1);
        hints[0] = address(alice);

        uint256 zbxBefore = zbx.balanceOf(address(redeemer));
        (uint256 zusdRedeemed, uint256 zbxOut) = _redeem(redeemer, 100e18, hints, 1);

        // Burned all redeemer's ZUSD.
        require(zusdRedeemed == 100e18, "should redeem 100 ZUSD");

        // Got 99.5 ZBX (100 ZBX worth - 0.5% fee).
        require(zbxOut == 100e18 - (100e18 * 50 / 10_000), "0.5% fee");
        require(zbx.balanceOf(address(redeemer)) - zbxBefore == zbxOut, "ZBX received");

        // Alice's CDP fully closed; leftover 200 ZBX returned to her.
        (uint256 col, uint256 debt, , ) = vault.getCDP(address(alice));
        require(col == 0,  "alice CDP collateral cleared");
        require(debt == 0, "alice CDP debt cleared");
        require(zbx.balanceOf(address(alice)) == 200e18, "alice got leftover");
    }

    // ─── 2. Partial redemption — CR INCREASES (Liquity-grade math) ───────

    function testPartialRedemptionIncreasesCR() public {
        // Alice: 300 ZBX collateral, 250 ZUSD debt, CR = 120%
        _openCdpFor(alice, 300e18, 250e18);
        _giveZusd(redeemer, 100e18, alice);

        (, , uint256 crBefore, ) = vault.getCDP(address(alice));

        address[] memory hints = new address[](1);
        hints[0] = address(alice);
        _redeem(redeemer, 100e18, hints, 1);

        (uint256 col, uint256 debt, uint256 crAfter, ) = vault.getCDP(address(alice));

        // Algebra: c'/d' >= c/d when c >= d, i.e. CR >= 100% pre-redemption.
        require(crAfter > crBefore, "CR must increase after redemption");
        require(col == 200e18, "alice 200 ZBX left");
        require(debt == 150e18, "alice 150 ZUSD debt left");
    }

    // ─── 3. Multi-CDP cascade in ascending-CR order ──────────────────────

    function testMultiCdpAscendingCascade() public {
        // Alice CR=120% (300 ZBX, 250 ZUSD)
        // Bob   CR=200% (200 ZBX, 100 ZUSD)
        // Carol CR=500% (500 ZBX, 100 ZUSD)
        _openCdpFor(alice, 300e18, 250e18);
        _openCdpFor(bob,   200e18, 100e18);
        _openCdpFor(carol, 500e18, 100e18);

        // Move 350 ZUSD to redeemer (will eat alice fully + part of bob).
        _giveZusd(redeemer, 250e18, alice);
        _giveZusd(redeemer, 100e18, bob);

        address[] memory hints = new address[](3);
        hints[0] = address(alice); // CR 120%
        hints[1] = address(bob);   // CR 200%
        hints[2] = address(carol); // CR 500%

        (uint256 zusdRedeemed,) = _redeem(redeemer, 350e18, hints, 3);
        require(zusdRedeemed == 350e18, "all 350 redeemed");

        // Alice fully closed.
        (uint256 ac,,,) = vault.getCDP(address(alice));
        require(ac == 0, "alice closed");

        // Bob debt-reduced (100 -> 0, full close possible too if hits dust);
        // we used MIN_ZUSD_MINT logic: 100 ZUSD redeemed from bob fully closes him.
        (uint256 bc, uint256 bd,,) = vault.getCDP(address(bob));
        require(bd == 0,  "bob debt cleared");
        require(bc == 0,  "bob collateral cleared (full close)");

        // Carol untouched.
        (uint256 cc, uint256 cd,,) = vault.getCDP(address(carol));
        require(cc == 500e18, "carol untouched col");
        require(cd == 100e18, "carol untouched debt");
    }

    // ─── 4. Vault-solvency invariant after multiple redemptions ──────────

    function testVaultSolvencyInvariant() public {
        _openCdpFor(alice, 300e18, 100e18);
        _openCdpFor(bob,   400e18, 100e18);
        _openCdpFor(carol, 500e18, 100e18);

        _giveZusd(redeemer, 100e18, alice);
        _giveZusd(redeemer, 100e18, bob);

        address[] memory hints = new address[](3);
        hints[0] = address(alice);
        hints[1] = address(bob);
        hints[2] = address(carol);

        _redeem(redeemer, 200e18, hints, 3);

        // Invariant: vault ZBX balance == sum(cdp.collateral)
        (uint256 ac,,,) = vault.getCDP(address(alice));
        (uint256 bc,,,) = vault.getCDP(address(bob));
        (uint256 cc,,,) = vault.getCDP(address(carol));
        uint256 sumCol  = ac + bc + cc;

        require(zbx.balanceOf(address(vault)) == sumCol,
                "vault ZBX must equal sum of CDP collateral");
        require(vault.totalCollateral() == sumCol,
                "totalCollateral must equal sum of CDP collateral");
    }

    // ─── 5. Reverts on non-monotone hints ────────────────────────────────

    function testRevertsOnNonMonotoneHints() public {
        _openCdpFor(alice, 300e18, 250e18); // CR 120%
        _openCdpFor(bob,   500e18, 100e18); // CR 500%

        _giveZusd(redeemer, 100e18, bob);

        address[] memory hints = new address[](2);
        hints[0] = address(bob);   // CR 500% first — out of order
        hints[1] = address(alice); // CR 120% second

        bool reverted;
        try Borrower(redeemer).callVault(
            address(vault),
            abi.encodeWithSignature("redeem(uint256,address[],uint256)", 100e18, hints, 2)
        ) {
            reverted = false;
        } catch {
            reverted = true;
        }
        require(reverted, "must revert on non-monotone hints");
    }

    // ─── 6. Reverts on unhealthy CDP (CR < 100%) ─────────────────────────

    function testRevertsOnUnhealthyCdp() public {
        // Alice 200 ZBX collateral / 100 ZUSD debt → CR 200%
        _openCdpFor(alice, 200e18, 100e18);

        // Drop ZBX price 60% → Alice CR = 80% (unhealthy).
        oracle.setPrice(address(zbx), 4e17);

        _giveZusd(redeemer, 50e18, alice);

        address[] memory hints = new address[](1);
        hints[0] = address(alice);

        bool reverted;
        try Borrower(redeemer).callVault(
            address(vault),
            abi.encodeWithSignature("redeem(uint256,address[],uint256)", 50e18, hints, 1)
        ) {
            reverted = false;
        } catch {
            reverted = true;
        }
        require(reverted, "must revert on unhealthy CDP");
    }

    // ─── 7. Reverts when paused ──────────────────────────────────────────

    function testRevertsWhenPaused() public {
        _openCdpFor(alice, 300e18, 100e18);
        _giveZusd(redeemer, 100e18, alice);

        // Owner (this test contract is the deployer) pauses.
        vault.setRedemptionPaused(true);
        require(vault.redemptionPaused() == true, "paused state set");

        address[] memory hints = new address[](1);
        hints[0] = address(alice);

        bool reverted;
        try Borrower(redeemer).callVault(
            address(vault),
            abi.encodeWithSignature("redeem(uint256,address[],uint256)", 100e18, hints, 1)
        ) {
            reverted = false;
        } catch {
            reverted = true;
        }
        require(reverted, "must revert when paused");
    }

    // ─── 8. Reverts on below-min ZUSD amount ─────────────────────────────

    function testRevertsBelowMin() public {
        _openCdpFor(alice, 300e18, 100e18);
        _giveZusd(redeemer, 5e18, alice);

        address[] memory hints = new address[](1);
        hints[0] = address(alice);

        bool reverted;
        try Borrower(redeemer).callVault(
            address(vault),
            abi.encodeWithSignature("redeem(uint256,address[],uint256)", 5e18, hints, 1)
        ) {
            reverted = false;
        } catch {
            reverted = true;
        }
        require(reverted, "must revert below MIN_REDEEM_AMOUNT");
    }

    // ─── 9. Reverts on insufficient ZUSD balance ─────────────────────────

    function testRevertsInsufficientZusd() public {
        _openCdpFor(alice, 300e18, 100e18);
        // Redeemer has 0 ZUSD.

        address[] memory hints = new address[](1);
        hints[0] = address(alice);

        bool reverted;
        try Borrower(redeemer).callVault(
            address(vault),
            abi.encodeWithSignature("redeem(uint256,address[],uint256)", 50e18, hints, 1)
        ) {
            reverted = false;
        } catch {
            reverted = true;
        }
        require(reverted, "must revert on insufficient ZUSD");
    }

    // ─── 10. Reverts on bad iteration bound ──────────────────────────────

    function testRevertsBadIterBound() public {
        _openCdpFor(alice, 300e18, 100e18);
        _giveZusd(redeemer, 50e18, alice);

        address[] memory hints = new address[](1);
        hints[0] = address(alice);

        // maxIterations = 0
        bool reverted0;
        try Borrower(redeemer).callVault(
            address(vault),
            abi.encodeWithSignature("redeem(uint256,address[],uint256)", 50e18, hints, 0)
        ) { reverted0 = false; } catch { reverted0 = true; }
        require(reverted0, "must revert on iter=0");

        // maxIterations = 51 (> MAX_REDEEM_ITER=50)
        bool reverted51;
        try Borrower(redeemer).callVault(
            address(vault),
            abi.encodeWithSignature("redeem(uint256,address[],uint256)", 50e18, hints, 51)
        ) { reverted51 = false; } catch { reverted51 = true; }
        require(reverted51, "must revert on iter>50");
    }

    // ─── 11. 0.5% fee correctly routed to feeRecipient ───────────────────

    function testFeeRoutesToFeeRecipient() public {
        // Set fee recipient to a fresh address.
        Borrower feeBox = new Borrower();
        vault.setFeeRecipient(address(feeBox));
        require(vault.feeRecipient() == address(feeBox), "fee recipient set");

        _openCdpFor(alice, 300e18, 100e18);
        _giveZusd(redeemer, 100e18, alice);

        uint256 feeBoxBefore = zbx.balanceOf(address(feeBox));

        address[] memory hints = new address[](1);
        hints[0] = address(alice);
        _redeem(redeemer, 100e18, hints, 1);

        uint256 expectedFee = 100e18 * 50 / 10_000;  // 0.5% of 100 ZBX
        require(
            zbx.balanceOf(address(feeBox)) - feeBoxBefore == expectedFee,
            "fee recipient got 0.5% of gross"
        );
        require(vault.totalRedemptionFees() == expectedFee, "totalRedemptionFees tracked");
    }

    // ─── 12. Dust-protection: leaves >= MIN_ZUSD_MINT or fully closes ────

    function testDustProtection() public {
        // Alice: 300 ZBX / 250 ZUSD debt — partial redeem 200 ZUSD would
        // leave 50 ZUSD dust (< MIN_ZUSD_MINT=100). Vault must instead
        // either (a) cap redemption at 150 (leaving 100 ZUSD), or
        //        (b) fully close.
        _openCdpFor(alice, 300e18, 250e18);
        _giveZusd(redeemer, 200e18, alice);

        address[] memory hints = new address[](1);
        hints[0] = address(alice);
        (uint256 zusdRedeemed, ) = _redeem(redeemer, 200e18, hints, 1);

        (, uint256 debt, , ) = vault.getCDP(address(alice));
        // Either fully closed (debt=0) or at least MIN_ZUSD_MINT remaining.
        require(debt == 0 || debt >= vault.MIN_ZUSD_MINT(),
                "no dust CDPs left behind");
        require(zusdRedeemed <= 200e18, "did not over-redeem");
    }

    // ─── 13. Architect-review M2 — totalDebt strict invariant ────────────
    //
    // After many redemptions across CDPs (with stability-fee accrual lag
    // potentially desyncing the global counter), the strict subtraction
    // (instead of clamp-to-zero) means the vault REVERTS on impossible
    // underflow rather than silently zeroing — making invariant violations
    // observable. This test verifies that the happy-path state is
    // arithmetically consistent: totalDebt >= sum(cdp.debt).
    function testTotalDebtStrictInvariant() public {
        _openCdpFor(alice, 300e18, 100e18);
        _openCdpFor(bob,   400e18, 100e18);
        _openCdpFor(carol, 500e18, 100e18);

        _giveZusd(redeemer, 100e18, alice);
        _giveZusd(redeemer, 100e18, bob);

        address[] memory hints = new address[](3);
        hints[0] = address(alice);
        hints[1] = address(bob);
        hints[2] = address(carol);

        _redeem(redeemer, 200e18, hints, 3);

        (, uint256 ad,,) = vault.getCDP(address(alice));
        (, uint256 bd,,) = vault.getCDP(address(bob));
        (, uint256 cd,,) = vault.getCDP(address(carol));
        uint256 sumDebt = ad + bd + cd;

        // totalDebt may be slightly higher than sum due to fee accrual on
        // CDPs not touched this redemption — but it must be at least sumDebt.
        require(vault.totalDebt() >= sumDebt,
                "totalDebt must be >= sum(cdp.debt)");
        // For this fee-free quick test (no time elapses), they should be equal.
        require(vault.totalDebt() == sumDebt,
                "no fee accrual → totalDebt == sum(cdp.debt) exactly");
    }

    // ─── 14. Architect-review M1 — sub-min debt via repay + redeem ───────
    //
    // The dust-protection branch (`cdpDebt < MIN_ZUSD_MINT`) is reachable
    // through `repay()` (not from fee accrual, which monotonically grows
    // debt). This test creates a sub-min CDP via repay and verifies the
    // vault either (a) full-closes it on redemption or (b) skips it as
    // unsafe-partial. Both behaviours are acceptable; the invariant is
    // "no dust LEFT BEHIND".
    function testSubMinDebtFromRepayThenRedeem() public {
        // Alice opens 300 ZBX / 250 ZUSD debt.
        _openCdpFor(alice, 300e18, 250e18);

        // Alice repays 200 ZUSD → debt = 50 (sub-min, but already-existing).
        // First, she needs ZUSD allowance.
        alice.callZusd(
            address(zusd),
            abi.encodeWithSignature("approve(address,uint256)", address(vault), 200e18)
        );
        alice.callVault(
            address(vault),
            abi.encodeWithSignature("repay(uint256)", 200e18)
        );

        // Verify alice's CDP now has sub-min debt.
        (, uint256 debtAfterRepay,,) = vault.getCDP(address(alice));
        require(debtAfterRepay < vault.MIN_ZUSD_MINT(),
                "repay should produce sub-min debt for this test");
        require(debtAfterRepay == 50e18, "exact 50 ZUSD debt expected");

        // Now redeemer tries to redeem 30 ZUSD against alice (partial, would
        // leave 20 ZUSD = dust which is also sub-min). Vault should SKIP
        // alice for partial and process nothing → revert "nothing redeemed".
        _giveZusd(redeemer, 30e18, alice);

        address[] memory hints = new address[](1);
        hints[0] = address(alice);

        bool reverted;
        try Borrower(redeemer).callVault(
            address(vault),
            abi.encodeWithSignature("redeem(uint256,address[],uint256)", 30e18, hints, 1)
        ) { reverted = false; } catch { reverted = true; }
        require(reverted,
                "partial redeem of sub-min CDP must revert (would create dust)");

        // BUT: a full redemption of the sub-min CDP IS allowed.
        // Redeemer needs MIN_REDEEM_AMOUNT (10 ZUSD); 50 satisfies that.
        // We need redeemer to have 50 ZUSD. Currently has 30; transfer more.
        _giveZusd(redeemer, 20e18, alice);
        (uint256 zusdRedeemed,) = _redeem(redeemer, 50e18, hints, 1);
        require(zusdRedeemed == 50e18, "full close of sub-min CDP succeeded");
        (uint256 acFinal, uint256 adFinal,,) = vault.getCDP(address(alice));
        require(acFinal == 0 && adFinal == 0, "alice CDP fully closed");
    }

    // ─── 15. Architect-review M1 — omitted-hint fairness disclosure ──────
    //
    // EXPLICIT documentation-as-test: the on-chain monotonicity check does
    // NOT prevent a caller from OMITTING a globally-lower-CR CDP and
    // targeting a higher-CR one instead. This is the documented limitation
    // (ZEP-005 §7, ZusdVault.sol fairness-disclosure docstring). This test
    // asserts the actual behaviour so any future change to "globally
    // lowest-first" enforcement is caught.
    function testOmittedLowerCrCdpStillRedeemable() public {
        // Alice CR=120%, Bob CR=500%. Within supplied hints (which omit Alice),
        // bob is the only candidate — the on-chain monotonicity check passes
        // because the supplied list is trivially sorted, but global fairness
        // is up to the off-chain SDK (see ZEP-005 §7 limitation 1).
        _openCdpFor(alice, 300e18, 250e18); // CR 120%
        _openCdpFor(bob,   500e18, 100e18); // CR 500%

        _giveZusd(redeemer, 100e18, bob);

        // Caller OMITS alice and targets only bob. This SHOULD succeed under
        // the current (testnet-grade) hint-based design — the SDK is
        // expected to provide canonical ordering, but the chain doesn't
        // enforce it. Mainnet ZEP will add on-chain sorted-list enforcement.
        address[] memory hintsBobOnly = new address[](1);
        hintsBobOnly[0] = address(bob);

        (uint256 zusdRedeemed, uint256 zbxOut) =
            _redeem(redeemer, 100e18, hintsBobOnly, 1);

        require(zusdRedeemed == 100e18, "redemption against bob succeeded");
        require(zbxOut > 0, "redeemer received ZBX");

        // Alice's CDP was untouched.
        (uint256 ac, uint256 ad,,) = vault.getCDP(address(alice));
        require(ac == 300e18, "alice collateral untouched");
        require(ad == 250e18, "alice debt untouched");

        // The economic mitigation is that alice (still lowest CR) becomes
        // the next redeemer's most-attractive target.
    }

    // ─── 16-19. S15-P2-D — fee-accrual / totalDebt principal-delta tests ──
    //
    // These tests EXIST because the architect re-review caught a regression
    // introduced by S15-P2-B's strict invariant guards: `totalDebt` is sum
    // of `cdp.debt` PRINCIPAL snapshots, but `_currentDebt` includes
    // accrued fee. Subtracting current-debt-units from principal-units
    // would falsely revert legitimate repay/close/redeem/liquidate calls
    // after time had elapsed. Fix: capture `oldPrincipal` BEFORE assignment
    // and adjust `totalDebt` by the principal delta.
    //
    // Each test warps 365 days (≈2% APY → currentDebt ≈ 1.02 × principal)
    // and asserts: (a) the operation does NOT revert with
    // "Vault: totalDebt invariant broken", and (b) `totalDebt` equals the
    // sum of remaining `cdp.debt` snapshots after the operation.

    uint256 constant ONE_YEAR = 365 days;

    function testRepayAfterFeeAccrual() public {
        // Alice: 300 ZBX, 100 ZUSD. Bob: 300 ZBX, 100 ZUSD (≥ MIN_ZUSD_MINT).
        // totalDebt = 200.
        _openCdpFor(alice, 300e18, 100e18);
        _openCdpFor(bob,   300e18, 100e18);
        require(vault.totalDebt() == 200e18, "init totalDebt");

        // Warp 1 year — currentDebt(alice) ≈ 102, totalDebt unchanged at 200.
        vm.warp(block.timestamp + ONE_YEAR);

        // Alice has 100 ZUSD from her open; top up via bob to comfortably
        // cover the 50 repay even if anything weird happens.
        _giveZusd(alice, 5e18, bob);

        // Repay 50 ZUSD (well below currentDebt).
        alice.callVault(
            address(vault),
            abi.encodeWithSignature("repay(uint256)", 50e18)
        );

        // Assert no false revert (we got here).
        // newPrincipal(alice) ≈ 102 - 50 = 52.
        // bob untouched at oldPrincipal snapshot = 100.
        // totalDebt = 52 + 100 = 152.
        (, uint256 aliceDebt,,) = vault.getCDP(address(alice));
        (, uint256 bobDebt,,)   = vault.getCDP(address(bob));
        require(aliceDebt > 50e18 && aliceDebt < 53e18, "alice principal ~52");
        require(bobDebt == 100e18, "bob principal snapshot unchanged");
        require(vault.totalDebt() == aliceDebt + bobDebt,
                "totalDebt = sum of cdp.debt snapshots");
    }

    function testCloseCdpAfterFeeAccrual() public {
        // Alice: 300/100. Bob: 200/100 (CR 200% ≥ 150% min). totalDebt = 200.
        _openCdpFor(alice, 300e18, 100e18);
        _openCdpFor(bob,   200e18, 100e18);
        require(vault.totalDebt() == 200e18, "init totalDebt");

        vm.warp(block.timestamp + ONE_YEAR);

        // Bob closes — needs to burn currentDebt (≈102) ZUSD. He has 100.
        // Top him up from alice (5 ZUSD).
        _giveZusd(bob, 5e18, alice);

        bob.callVault(
            address(vault),
            abi.encodeWithSignature("closeCDP()")
        );

        // Bob's CDP gone; totalDebt -= 100 (bob's oldPrincipal).
        // Alice untouched (still 100 principal snapshot).
        (uint256 bobCol, uint256 bobDebt,,) = vault.getCDP(address(bob));
        require(bobCol == 0 && bobDebt == 0, "bob CDP fully closed");
        require(vault.totalDebt() == 100e18,
                "totalDebt = alice's principal snapshot only");
    }

    function testRedeemAfterFeeAccrual() public {
        // Alice: 300 ZBX, 100 ZUSD (CR 300%). Bob: 500/100 (CR 500%).
        _openCdpFor(alice, 300e18, 100e18);
        _openCdpFor(bob,   500e18, 100e18);
        require(vault.totalDebt() == 200e18, "init totalDebt");

        vm.warp(block.timestamp + ONE_YEAR);

        // Move 50 ZUSD to redeemer.
        _giveZusd(redeemer, 50e18, alice);

        address[] memory hints = new address[](1);
        hints[0] = address(alice);  // alice is lowest-CR

        (uint256 zusdRedeemed, ) = _redeem(redeemer, 50e18, hints, 1);
        require(zusdRedeemed == 50e18, "full 50 redeemed against alice");

        // alice's currentDebt was ≈ 102; redeemFromCdp = 50;
        // newPrincipal ≈ 52. principalReduction = 100 - 52 = 48.
        // totalDebt = 200 - 48 = 152. (Bob's principal still 100.)
        (, uint256 aliceDebt,,) = vault.getCDP(address(alice));
        (, uint256 bobDebt,,)   = vault.getCDP(address(bob));
        require(aliceDebt > 50e18 && aliceDebt < 53e18, "alice principal ~52");
        require(bobDebt == 100e18, "bob untouched");
        require(vault.totalDebt() == aliceDebt + bobDebt,
                "totalDebt = sum of cdp.debt snapshots");
    }

    function testLiquidateAfterFeeAccrual() public {
        // Alice: 250 ZBX, 100 ZUSD (CR 250% at $1) — meets MIN_ZUSD_MINT.
        // Bob (liquidator): 500 ZBX, 200 ZUSD (CR 250%) — provides liquidation ZUSD.
        _openCdpFor(alice, 250e18, 100e18);
        _openCdpFor(bob,   500e18, 200e18);
        require(vault.totalDebt() == 300e18, "init totalDebt");

        vm.warp(block.timestamp + ONE_YEAR);

        // Drop ZBX price to $0.40 so alice's CR drops below 100%.
        // alice colValue = 250 × 0.4 = 100, currentDebt ≈ 102, CR ≈ 0.98.
        // bob's CR also ≈ 0.98 — he could be liquidated too, but the test
        // only liquidates alice; bob's CR isn't checked when HE calls liquidate.
        oracle.setPrice(address(zbx), 4e17);  // $0.40

        // Bob has 200 ZUSD; needs ≈102 to burn. Comfortable margin.
        bob.callVault(
            address(vault),
            abi.encodeWithSignature("liquidate(address)", address(alice))
        );

        // Alice fully liquidated; totalDebt -= 100 (alice's oldPrincipal).
        // Bob untouched (still 200 principal snapshot).
        (uint256 aliceCol, uint256 aliceDebt,,) = vault.getCDP(address(alice));
        require(aliceCol == 0 && aliceDebt == 0, "alice fully liquidated");
        require(vault.totalDebt() == 200e18,
                "totalDebt = bob's principal snapshot only");
    }

    // ─── 20-21. S15-P2-D-2 — mintMore + repay-less-than-accrued-fee ────────
    //
    // Architect re-review #3 caught:
    //  (a) `mintMore()` had the same unit-mismatch bug — does
    //      `totalDebt += zusdAmount` but sets `cdp.debt = currentDebt +
    //      zusdAmount`, silently DROPPING the accrued fee from totalDebt
    //      forever. Trips the strict invariant guards on next touch.
    //  (b) The repay() principal-delta INCREASE branch (newPrincipal >
    //      oldPrincipal when user pays less than accrued fee) had no test.

    function testMintMoreAfterFeeAccrual() public {
        // Alice: 300 ZBX, 100 ZUSD. totalDebt = 100.
        _openCdpFor(alice, 300e18, 100e18);
        require(vault.totalDebt() == 100e18, "init totalDebt");

        vm.warp(block.timestamp + ONE_YEAR);

        // Alice mintMore 50 ZUSD. Post-CR: 300 / (102 + 50) = 197% ≥ 150%. ✓
        // newPrincipal = currentDebt + 50 ≈ 152. principalDelta ≈ 52.
        // totalDebt should become 100 + 52 = 152, NOT 100 + 50 = 150 (the bug).
        alice.callVault(
            address(vault),
            abi.encodeWithSignature("mintMore(uint256)", 50e18)
        );

        (, uint256 aliceDebt,,) = vault.getCDP(address(alice));
        require(aliceDebt > 150e18 && aliceDebt < 153e18,
                "alice newPrincipal ~152 (currentDebt + 50)");
        require(vault.totalDebt() == aliceDebt,
                "totalDebt == cdp.debt (single CDP, principal-delta correct)");

        // PROOF the fix is consistent: now close the CDP and assert it
        // doesn't trip the strict guard. Pre-fix this would revert because
        // totalDebt (150) < oldPrincipal (152).
        // Alice has 150 ZUSD (100 from open + 50 from mintMore); needs ~152
        // to burn. Open bob to source the gap.
        _openCdpFor(bob, 300e18, 100e18);
        _giveZusd(alice, 10e18, bob);

        alice.callVault(
            address(vault),
            abi.encodeWithSignature("closeCDP()")
        );
        (uint256 ac, uint256 ad,,) = vault.getCDP(address(alice));
        require(ac == 0 && ad == 0, "alice closed cleanly post-mintMore");
        require(vault.totalDebt() == 100e18,
                "totalDebt = bob's snapshot (alice's 152 oldPrincipal removed)");
    }

    function testRepayLessThanAccruedFee() public {
        // Alice: 300 ZBX, 100 ZUSD. totalDebt = 100.
        _openCdpFor(alice, 300e18, 100e18);

        // Warp 10 years to amplify accrued fee well above any repay amount.
        // STABILITY_FEE_PER_SEC × 10 years × RAY = ~0.198 → currentDebt ≈ 119.8.
        vm.warp(block.timestamp + 10 * ONE_YEAR);

        // Alice repays 1 ZUSD (≪ accrued fee ≈ 19.8 ZUSD).
        // newPrincipal = 119.8 - 1 = 118.8. principalDelta = +18.8 (INCREASE).
        // totalDebt should become 100 + 18.8 ≈ 118.8.
        alice.callVault(
            address(vault),
            abi.encodeWithSignature("repay(uint256)", 1e18)
        );

        (, uint256 aliceDebt,,) = vault.getCDP(address(alice));
        // After 10 years with linear (non-compounded) accrual, debt is
        // somewhere in [115, 125] depending on small rounding — assert the
        // INCREASE direction holds (the key principal-delta property).
        require(aliceDebt > 110e18,
                "alice principal INCREASED past initial 100 (fee capitalised)");
        require(vault.totalDebt() == aliceDebt,
                "totalDebt == cdp.debt (principal-delta INCREASE branch correct)");
        require(vault.totalDebt() > 100e18,
                "totalDebt grew (proves += branch executed, not the -= branch)");
    }
}
