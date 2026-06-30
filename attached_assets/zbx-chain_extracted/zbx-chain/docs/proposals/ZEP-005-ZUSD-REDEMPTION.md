# ZEP-005 — ZUSD Redemption Mechanism (S15-P2 — re-enables redeem after S6-V2 fix)

| Field      | Value                                                              |
|------------|--------------------------------------------------------------------|
| ZEP        | 005                                                                |
| Title      | ZUSD Redemption — hint-based, monotonicity-checked                 |
| Author     | Zebvix Core Team                                                   |
| Status     | **Implemented (testnet-grade)** — pending VPS verify + audit       |
| Category   | Stablecoin / DeFi                                                  |
| Created    | 2026-05-01                                                         |
| Supersedes | S6-V2 (P1 follow-up tracked in `AUDIT_2026-04-30.md`)              |
| Related    | ZEP-002 (ZUSD spec), `docs/ZUSD.md` (user-facing)                  |

---

## 1. Summary

Re-enables `ZusdVault.redeem()` (previously stubbed-and-reverting due to the
S6-V2 vault-drain bug) using a **hint-based, monotonicity-checked** design
that ATOMICALLY updates per-CDP `collateral` and `debt` records — the exact
fix for the original bug.

This proposal is the **lower-bound peg-defence** for ZUSD. Without it, ZUSD
has no on-chain mechanism to recover from sub-$1 depegs (Iron Finance /
Terra-UST class of failure).

---

## 2. Background — what S6-V2 broke

The original `redeem()` (Session 6) burned the redeemer's ZUSD and
transferred ZBX out of the vault, but **never decremented any CDP's
`collateral` or `debt`** — only the global `totalDebt` counter.

After enough redemptions, every CDP record still showed full original
collateral while the vault held a fraction of it. The first CDP holder to
call `closeCDP()` would get their ZBX back; subsequent holders would face
`require(IZBX_Transfer(zbx).transfer(...))` reverts. **Permanent loss for
late-comers, irreversible.**

The audit decision (S6-V2) was to revert in the function body — *disabled
and honest is strictly safer than enabled and broken*. This ZEP replaces
that stub.

---

## 3. Design

### 3.1 Why hint-based (not on-chain sorted list)

A full Liquity-style sorted-CDP linked list would require:
- New `mapping(address => address) prevCdpInList, nextCdpInList`
- Update list on every `openCDP`, `addCollateral`, `mintMore`, `repay`,
  `withdrawCollateral`, `closeCDP`, `liquidate` — 7 functions touched.
- O(n) insert without hint, O(1) with hint.
- ~150 LOC of list-maintenance code; ~5–10 k extra gas per CDP mutation.

For testnet-grade rollout, we instead use the **off-chain hint** approach
that Liquity itself uses (Liquity layers both — list + hints — but the
list is a defence-in-depth against bad hints, not a correctness primitive).

The caller (off-chain bot, SDK helper, wallet UI) provides
`address[] cdpHints` sorted ASCENDING by current collateral ratio. The
vault verifies monotonicity on chain — a bad hint reverts.

This avoids touching any other CDP-mutating function while still ensuring
**ascending-CR ordering within the supplied hints** (the on-chain
fairness property; for global lowest-CR-first see §7 limitation 1 — the
SDK provides canonical ordering off-chain, mainnet adds a sorted CDP
linked list).

### 3.2 API

```solidity
function redeem(
    uint256 zusdAmount,
    address[] calldata cdpHints,
    uint256 maxIterations
) external nonReentrant returns (uint256 zusdRedeemed, uint256 zbxOut);

function setRedemptionPaused(bool paused) external;  // owner-only
function setFeeRecipient(address recipient) external; // owner-only
```

### 3.3 Per-CDP processing algorithm

```
For each cdpOwner in cdpHints (capped by maxIterations):
  1. Skip if cdp.collateral == 0       (closed CDP, stale hint)
  2. Skip if currentDebt == 0          (no debt to redeem against)
  3. Compute crBps = colValue * BPS / debt
  4. require(crBps >= LIQUIDATION_RATIO)   ← reject unhealthy
  5. require(crBps >= prevCRBps)           ← monotonicity
  6. redeemFromCdp = min(remaining, currentDebt)
  7. Cap to either fully close OR leave >= MIN_ZUSD_MINT     ← dust protection
  8. zbxFromCdp = redeemFromCdp * 1e18 / zbxPrice            ← rounds down
  9. ATOMIC UPDATE:                                          ← THE S6-V2 FIX
     cdp.collateral   -= zbxFromCdp
     cdp.debt          = postDebt
     cdp.lastFeeIndex  = feeIndex
     totalCollateral  -= zbxFromCdp
     totalDebt        -= redeemFromCdp
  10. If postDebt == 0 → return leftover collateral to original owner,
                         delete cdps[owner]
  11. iter++

Final settlement:
  - Burn zusdRedeemed from caller
  - fee     = grossZbxOut * 0.5%
  - zbxOut  = grossZbxOut - fee
  - Transfer zbxOut to caller, fee to feeRecipient (or owner if unset)
```

### 3.4 Constants

| Constant | Value | Purpose |
|---|---|---|
| `MIN_REDEEM_AMOUNT` | 10 ZUSD (10e18) | Anti-spam |
| `MAX_REDEEM_ITER` | 50 | Gas-bound (~3 M gas worst case) |
| `REDEMPTION_FEE_BPS` | 50 | 0.5% — Liquity-style floor fee |
| `MIN_ZUSD_MINT` | 100 ZUSD | Re-used for dust-protection cap |

### 3.5 Storage additions

```solidity
bool    public redemptionPaused;     // emergency stop
address public feeRecipient;         // ZBX fee sink (owner if zero)
uint256 public totalRedeemed;        // lifetime analytics
uint256 public totalRedemptionFees;  // lifetime analytics
```

Storage is APPENDED after existing slots — no layout breakage of
already-deployed instances.

---

## 4. Safety invariants

| # | Invariant | Enforcement |
|---|---|---|
| I1 | Vault solvency: `Σ cdp.collateral + leftover_returns == ZBX_in_vault` | Test `testVaultSolvencyInvariant` |
| I2 | Ascending-CR WITHIN supplied hints (NOT globally lowest-first — see §7 limitation 1; architect-review M1) | On-chain require, test `testRevertsOnNonMonotoneHints` |
| I3 | Healthy-only: never redeem from CDP with CR < 100% | On-chain require, test `testRevertsOnUnhealthyCdp` |
| I4 | Per-CDP CR-monotone: CR after >= CR before (when CR ≥ 100%) | Algebra + test `testPartialRedemptionIncreasesCR` |
| I5 | No dust CDPs created | Cap to leave ≥ MIN_ZUSD_MINT, test `testDustProtection` |
| I6 | No vault drain via redemption | Each ZBX out matched by debt reduction in some CDP |
| I7 | Reentrancy-safe | `nonReentrant` + tests via mock callbacks |
| I8 | Fee correctly routed | Test `testFeeRoutesToFeeRecipient` |
| I9 | Owner-only admin | `require(msg.sender == owner)` on setters |

---

## 5. Why redemption is safe FOR the CDP owner

Algebra: when CR ≥ 100% pre-redemption (always enforced), the post-state CR
strictly INCREASES.

```
Before:  c_v / d ≥ 1   (CR ≥ 100%)
Take Δ ZUSD worth of collateral and burn Δ debt:
After:   (c_v - Δ) / (d - Δ)
         = c_v/d * (1 - Δ/c_v) / (1 - Δ/d)
         ≥ c_v/d   iff Δ/d ≥ Δ/c_v   iff c_v ≥ d   ← always true
```

Redemption is **mathematically equivalent to the borrower repaying their
own debt at par**. The only "cost" to the CDP owner is loss of optionality
(they're forced to do it). Ascending-CR-within-hints ordering (canonical
lowest-CR-first via off-chain SDK; see §7 limitation 1) aims to put that
cost on borrowers who had the least optionality anyway (closest to
liquidation). Mainnet on-chain sorted-list enforcement makes this property
trustless rather than SDK-dependent.

---

## 6. Test plan

### 6.1 Unit tests (in `contracts/test/ZusdVaultRedemption.t.sol`, 12 tests)

1. Full single-CDP redemption (happy path)
2. Partial single-CDP redemption — CR increases
3. Multi-CDP cascade in ascending-CR order
4. Vault-solvency invariant after multiple redemptions
5. Reverts on non-monotone hints
6. Reverts on unhealthy CDP (CR < 100%)
7. Reverts when paused
8. Reverts on below-min ZUSD amount
9. Reverts on insufficient ZUSD balance
10. Reverts on bad iteration bound (0 or > 50)
11. 0.5% fee correctly applied and routed to feeRecipient
12. Dust-protection: leaves ≥ MIN_ZUSD_MINT or fully closes

### 6.2 Pending — VPS-only (cannot run in sandbox)

```bash
# On VPS srv1266996:
cd /path/to/zbx-chain/contracts
forge test --match-contract ZusdVaultRedemptionTest -vvv

# Expected: 12 passed; 0 failed.
```

### 6.3 Pending — Foundry invariant testing (next session)

```bash
forge test --invariant
```

Invariants to add (separate file, follow-up):
- `invariant_vaultSolvency()`
- `invariant_totalCollateralEqualsCdpSum()`
- `invariant_redeemNeverDrainsVault()`
- `invariant_crNeverDecreasesAfterRedemption()`

---

## 7. Limitations (intentional, testnet-acceptable)

| Limitation | Mitigation | Mainnet upgrade path |
|---|---|---|
| No on-chain sorted CDP list — caller can OMIT a lower-CR CDP | (a) Off-chain SDK provides canonical ordering; (b) economic — any omitted low-CR CDP becomes the next redeemer's most-attractive target; (c) the redemption is still SAFE for the protocol (no drain), only fairness across CDPs is best-effort | **REQUIRED** — full sorted CDP linked list (Liquity-style) before mainnet. Tracked as ZEP-XXX (next session). |
| Single oracle source | Defensive `zbxPrice > 0` check | TWAP + multi-source (Pyth/Chainlink median) — separate ZEP |
| O(n) cdpHints validation | `MAX_REDEEM_ITER = 50` cap | Acceptable; matches Liquity per-call cost |
| No flash-loan-resistance gating | Liquidation cooldown (separate ZEP) | Add 1-block delay between liq and redeem |
| Single-tier liquidation pool | Stability pool exists separately | OK for testnet; mainnet may want fallback auctions |

---

## 8. Mainnet checklist (pre-launch)

- [ ] All 12 unit tests pass on VPS
- [ ] Foundry invariant tests pass with 1 M+ runs
- [ ] External audit by stablecoin-experienced firm (Halborn / Trail of Bits / Spearbit)
- [ ] Bug bounty active (Immunefi, $100 k+ tier)
- [ ] TWAP oracle integrated (separate ZEP)
- [ ] Time-locked governance for parameter changes
- [ ] Emergency multisig for `setRedemptionPaused(true)`
- [ ] Monitoring alerts: ZUSD-peg drift, redemption volume, vault collateral ratio
- [ ] Public testnet stress-test ≥ 4 weeks
- [ ] All known limitations from §7 explicitly addressed or documented as accepted risk

---

## 9. References

- Liquity Whitepaper §4.4 — "Redemption mechanism"
  https://docs.liquity.org/documentation/sorted-troves
- MakerDAO `Vat.sol` — original CDP accounting reference
- Iron Finance post-mortem (June 2021) — stablecoin redemption-bug case study
- Mango Markets post-mortem (Oct 2022) — oracle-manipulation case study
- `AUDIT_2026-04-30.md` — S6-V2 entry (original bug), S6-V2-FIXED entry (this ZEP)
- `docs/ZUSD.md` §Redemption — user-facing documentation
