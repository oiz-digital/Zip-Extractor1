# ZEP-007 — TVL Aggregator Oracle

| Field | Value |
|---|---|
| **Number** | ZEP-007 |
| **Title** | On-chain Total-Value-Locked aggregator with Chainlink-style price feeds |
| **Author** | Zebvix Technologies Pvt Ltd |
| **Status** | Draft |
| **Type** | Standards Track |
| **Created** | 2026-05-01 |
| **Replaces** | none |
| **Requires** | ZEP-006 (ZRC20 advanced surface), `IZbxAggregatorV3` (deployed) |
| **Audit** | architect Phase-1 PASS-WITH-FIXES (3 fixes applied); final review pending |

---

## 1. Abstract

This proposal introduces `ZbxTvlOracle`, an on-chain aggregator that
computes the total value locked (TVL) across the canonical Zebvix DeFi
modules (AMM, lending, stability, staking, plus scaffolded reward and
bridge-vault sources) in a single canonical USD-18 number, plus a
per-source breakdown.

It is paired with:

- a Rust **indexer time-series** (`tvl_snapshots` table) that records
  one row per `tvlBreakdown()` call,
- a public **REST API** (`/v1/tvl`, `/v1/tvl/global`, `/v1/tvl/by-source`,
  `/v1/tvl/history`) for off-chain consumption,
- a **CLI subcommand** (`zbxctl defi tvl --oracle 0x…`) for operator
  inspection.

The design optimises for *correctness under partial failure*: a missing
or stale price feed for a single token MUST NOT revert the entire TVL
query — instead the unpriced token's contribution silently rounds to
zero and is published via `unpricedTokens()` so off-chain monitoring
can act on it.

## 2. Motivation

Block explorers, dashboards, and integrating wallets currently must:

1. Crawl every AMM pair, lending market, stability pool, and staking
   contract individually,
2. Query each token's price from `IZbxAggregatorV3`,
3. Normalise per-token decimals to a common precision,
4. Sum.

This is duplicated effort, brittle to upgrades, and inconsistent across
clients. A single canonical on-chain aggregator with explicit
fail-closed semantics gives every consumer the same answer at the same
block height, and serves as the definitive input for governance metrics
(e.g. `treasuryUtilization = grants / tvl`) and risk frameworks
(e.g. `lendingHeadroom = tvl_lending / total_borrowed`).

## 3. Specification

### 3.1 Interface — `IZbxTvlOracle`

Defined in `contracts/interfaces/IZbxTvlOracle.sol` (126 LOC). Verbatim
surface (signatures here track the deployed interface — see also §3.6
for the source enum):

```solidity
enum Source { AMM, LENDING, STABILITY, STAKING, REWARD, BRIDGE_VAULT }
//             0    1        2          3        4       5

struct TvlBreakdown {
    uint256 amm;          // USD-18
    uint256 lending;      // USD-18
    uint256 stability;    // USD-18
    uint256 staking;      // USD-18
    uint256 reward;       // USD-18 (scaffolded; 0 in v1 until configured)
    uint256 bridgeVault;  // USD-18 (scaffolded; 0 in v1 until configured)
    uint256 total;        // sum of the above
    uint256 timestamp;    // block.timestamp at call time
}

// ── Reads ────────────────────────────────────────────────────────────────
function totalValueLockedUSD() external view returns (uint256);
function tvlBySource(Source src) external view returns (uint256);
function tvlByToken(address token) external view returns (uint256);
function tvlBreakdown() external view returns (TvlBreakdown memory);

function tvlAMM()         external view returns (uint256);
function tvlLending()     external view returns (uint256);
function tvlStability()   external view returns (uint256);
function tvlStaking()     external view returns (uint256);
function tvlReward()      external view returns (uint256);
function tvlBridgeVault() external view returns (uint256);

// ── Config inspection ────────────────────────────────────────────────────
function priceFeed(address token)  external view returns (address);
function source(Source src)        external view returns (address);
function maxStaleness()            external view returns (uint64);
function maxPairsToScan()          external view returns (uint16);
function paused()                  external view returns (bool);

// S23b — TWAP alt-price-source (see §3.7)
function twapOracle() external view returns (address);
function twapRoute(address token)
    external view returns (address pair, address quoteToken, bool enabled);

// ── Monitoring ───────────────────────────────────────────────────────────
function unpricedTokens() external view returns (address[] memory);
function refreshUnpriced() external;  // permissionless
function pairScanStats()
    external view
    returns (uint256 totalPairs, uint256 scanned, bool truncated);

// ── Admin (owner-only) ───────────────────────────────────────────────────
function setPriceFeed(address token, address aggregator) external;
function setSource(Source src, address contractAddr) external;
function setMaxStaleness(uint64 seconds_) external;  // default 3600
function setMaxPairsToScan(uint16 cap) external;     // default 256
function pause() external;
function unpause() external;

// S23b — TWAP alt-price-source admin (see §3.7)
function setTwapOracle(address oracle) external;
function setTwapRoute(
    address token,
    address pair,
    address quoteToken,
    bool    enabled
) external;

// ── Implementation-only operator surface (NOT in IZbxTvlOracle) ─────────
//   These functions live on the concrete `ZbxTvlOracle` contract, not
//   on the interface, because they are implementation-coupled. Operator
//   runbooks MUST cover them:
//
//   function setStabilityDepositToken(address token) external onlyOwner;
//     // Tells the STABILITY source which ERC-20 the ZusdStabilityPool
//     // accounts in. Until set (zero address), tvlStability() returns 0.

// ── Events ───────────────────────────────────────────────────────────────
event PriceFeedSet(address indexed token, address indexed aggregator);
event SourceSet(Source indexed src, address indexed contractAddr);
event MaxStalenessSet(uint64 oldValue, uint64 newValue);
event MaxPairsToScanSet(uint16 oldValue, uint16 newValue);
event Paused();
event Unpaused();
event PairScanTruncated(uint256 totalPairs, uint256 scanned);

// S23b — TWAP routing events
event TwapOracleSet(address indexed oldOracle, address indexed newOracle);
event TwapRouteSet(
    address indexed token,
    address indexed pair,
    address indexed quoteToken,
    bool    enabled
);

// ── Errors ───────────────────────────────────────────────────────────────
// S18: `error NotOwner()` is provided by the `Ownable2Step` base in the
//      implementation. It is intentionally NOT declared on this interface
//      to avoid a duplicate declaration when a contract inherits both.
error ZeroAddress();
error InvalidStaleness();
error InvalidPairCap();
error AlreadyPaused();
error NotPaused();
error PausedQuery();   // reverted by every read view while paused
error UnknownSource(); // reverted when Source enum is out of range

// S23b — TWAP routing errors
error TwapQuoteUnpriced();      // setTwapRoute(enabled=true) without quote feed
error TwapPairTokenMismatch();  // pair lacks token+quoteToken (or quote==token)
```

### 3.2 USD precision and normalisation

All published USD values are 18-decimal `uint256`. The
`_normalize18(amount, tokenDecimals, priceDecimals)` helper enforces:

- `MAX_TOKEN_DECIMALS = 36`
- `MAX_PRICE_DECIMALS = 18`

Out-of-policy decimals fail-closed (return zero) rather than reverting,
keeping the aggregate query alive in the presence of an
off-policy ERC-20.

### 3.3 Stale price guard

`setMaxStaleness(secs)` sets the maximum age (default 3600 seconds = 1
hour) of a price-feed `updatedAt` that the oracle will accept. A feed
that is too old contributes zero to TVL **and** the underlying token is
appended to `unpricedTokens()` for monitoring.

This is *fail-closed* (under-reports TVL) by deliberate design — a
silent over-report is far more dangerous than a known under-report.

### 3.4 Pair-scan cap (DoS protection)

The AMM source iterates the deployed pair set. To prevent a malicious
factory from making `tvlBreakdown()` un-callable via gas-bomb pair
spam, the iteration is bounded by `maxPairsToScan` (default 256, owner
range `[1, 65535]`). When truncation occurs:

- `pairScanStats()` returns `truncated = true` along with the actual
  `totalPairs` and `scanned` counts (both `uint256`). These are
  computed live by the view from the registered AMM factory's
  `allPairsLength()` and the current `maxPairsToScan` cap — not from
  cached state of a prior `refreshUnpriced()`, so the values reflect
  the AMM's true state at the call's block.
- `PairScanTruncated(uint256 totalPairs, uint256 scanned)` is emitted
  from `refreshUnpriced()` (NOT from views — views are gas-free in
  `eth_call` but the event fires from the state-mutating refresh path
  to give off-chain monitors a permissionless trigger).

When truncated, `tvlAMM()` (and therefore `totalValueLockedUSD()`) is
a *lower bound* on true AMM TVL — operators should react by raising
`maxPairsToScan` via `setMaxPairsToScan`.

### 3.5 Pause semantics

`pause()` causes every read view (`totalValueLockedUSD`,
`tvlBreakdown`, `tvlBySource`, `tvlByToken`, `tvlAMM`/`tvlLending`/
etc.) to revert with the `PausedQuery()` selector. (`Paused()` is the
*event*, emitted on the pause/unpause transitions; `PausedQuery()` is
the *error* selector that view callers actually receive.)

We deliberately do NOT freeze views to a last-cached value — surfacing
the paused state explicitly forces consumers to handle stale-data risk
rather than silently displaying yesterday's number.

### 3.6 Source enumeration

The on-chain enum is **0-indexed** (Solidity convention):

| Index | Variant         | v1 status                                  |
|-------|-----------------|--------------------------------------------|
| 0     | `AMM`           | implemented                                |
| 1     | `LENDING`       | implemented                                |
| 2     | `STABILITY`     | implemented                                |
| 3     | `STAKING`       | implemented                                |
| 4     | `REWARD`        | implemented (S24) — see §3.8                |
| 5     | `BRIDGE_VAULT`  | implemented (S24) — see §3.8                |

`tvlBySource(Source src)` and `setSource(Source src, address)` accept
this enum directly; calls with an out-of-range value revert with
`UnknownSource()`. As of **S24** all six sources are implemented; see
§3.8 for the REWARD + BRIDGE_VAULT wiring.

### 3.7 Alt-price-source: TWAP routing (S23b)

By default `_safeUSD(token, amount)` resolves USD via `priceFeed[token]`
(a Chainlink-style aggregator registered by `setPriceFeed`). For tokens
that lack a Chainlink-quality aggregator, the operator may opt the token
into an on-chain TWAP route via the `IZbxTwapOracle` (ZEP-008):

```solidity
// 1. Register a working aggregator for the QUOTE token (e.g. zUSD).
oracle.setPriceFeed(zUSD, zUSDAggregator);

// 2. Wire the TWAP oracle (one-time, chain-wide).
oracle.setTwapOracle(twapOracleAddress);

// 3. Per token: route it via the (token, quoteToken) AMM pair.
oracle.setTwapRoute(token, pair, zUSD, true);
```

**Routing semantics — single hop, no recursion**

```
token   ──TWAP.consult(pair, token, amt)──▶  amt-in-quoteToken
quoteToken ──aggregator.latestRoundData()──▶  USD-18
```

The setter enforces at config time that `quoteToken` has an aggregator
`priceFeed` registered, so the path always terminates at the aggregator
in a single hop. There is no recursion, no nested TWAP, and no depth
limit needed.

**Setter invariants** (rejected at config time, before persisting):

| Check                                              | Error                  |
|----------------------------------------------------|------------------------|
| `token == address(0)`                              | `ZeroAddress`          |
| (enabled) `pair == address(0)`                     | `ZeroAddress`          |
| (enabled) `quoteToken == address(0)`               | `ZeroAddress`          |
| (enabled) `quoteToken == token`                    | `TwapPairTokenMismatch`|
| (enabled) `priceFeed[quoteToken] == address(0)`    | `TwapQuoteUnpriced`    |
| (enabled) `pair.token0/token1` lack `token` OR `quoteToken` | `TwapPairTokenMismatch` |

**Runtime fail-closed policy**

The TWAP branch is fail-closed by construction:

- `twapOracle == address(0)` while `route.enabled == true` → 0 USD.
- `twapOracle.consult(...)` reverts (e.g. `NotPrimed`, `PairInactive`,
  pair deactivated) → 0 USD via try/catch.
- Quote leg's aggregator later un-registered or goes stale →
  `_safeUSDAggregatorOnly(quoteToken, ...)` returns 0 (existing policy).

The token is NEVER silently re-routed, and the TWAP branch never
under-reports louder than the legacy aggregator branch.

**`refreshUnpriced` / `unpricedTokens` chaining**

`_checkPriced(token)` chains the priced check to the route's quote
token: a TWAP-routed token whose quote leg has a healthy aggregator IS
considered priced. If the quote leg is missing or stale, the QUOTE
TOKEN is appended to `unpricedTokens()` (so monitoring sees the actual
broken dependency, not just the surface symptom on `token`). If
`twapOracle == address(0)` while the route is enabled, the original
`token` is appended (the integration itself is broken).

**Observability scope (S23b-Polish-2)**

`_checkPriced` deliberately does NOT probe `twapOracle.consult(...)`
to detect TWAP-side health failures (e.g. `NotPrimed`, `PairInactive`,
pair deactivated). Probing every routed token's consult inside
`refreshUnpriced` would (a) blow up its gas budget linearly with the
number of routed tokens, undermining the `maxPairsToScan` DoS cap; and
(b) generate noisy false-positives on transient `NotPrimed` states
that resolve at the next keeper `update`. The runtime fail-closed
contract on `_safeUSD` (TWAP consult revert → 0 contribution via
try/catch) is unaffected — a TWAP-side failure under-reports TVL but
never over-reports it.

TWAP-side health is observable directly via the TWAP's own event
surface — off-chain monitor should subscribe to:

- `ZbxTwapOracle.PairDeactivated(pair)` — operator deactivation.
- `ZbxTwapOracle.PairRegistered(pair, period)` — operator
  (re-)activation.
- `ZbxTwapOracle.PeriodUpdated(pair, oldPeriod, newPeriod)` — period
  reconfiguration (without forcing cache invalidation; only an
  INCREASE triggers `PairCacheInvalidated` per S23a-fix2).
- `ZbxTwapOracle.PairCacheInvalidated(pair, oldPeriod, newPeriod)` —
  period-INCREASE while the cache is currently primed → cache rebuild
  (cached `priceAvg` invalidated until the next successful `update`).
  Period DECREASE preserves the cache (per S23a-fix2).
- `ZbxTwapOracle.ObservationCommitted(pair, ...)` — heartbeat of
  successful `update` ticks. Absence over a full `period` window is
  the freshness signal.

### 3.8 Phase 7 sources: REWARD + BRIDGE_VAULT (S24)

The two Phase 7 sources are now wired through the same canonical
`setSource(Source src, address contractAddr)` admin path used by AMM /
LENDING / STABILITY / STAKING. No new admin surface is introduced.

**REWARD — `Source.REWARD` (enum index 4)**

```solidity
oracle.setSource(IZbxTvlOracle.Source.REWARD, distributorAddress);
```

The TVL contribution is the un-distributed ZBX held by the configured
`ZbxRewardDistributor`. Since `claimRewards()` `transfer()`s ZBX out of
the distributor on each claim, the distributor's own balance is
exactly the residual claim-pool plus any pre-funded rewards waiting to
be allocated.

```
_tvlReward() = _safeUSD(distributor.zbx(),
                        IERC20(distributor.zbx()).balanceOf(distributor))
```

**BRIDGE_VAULT — `Source.BRIDGE_VAULT` (enum index 5)**

```solidity
oracle.setSource(IZbxTvlOracle.Source.BRIDGE_VAULT, vaultAddress);
```

The TVL contribution is `BridgeVault.totalLocked()` priced via the
vault's immutable `token()`. BridgeVault is single-token by design;
multi-token aggregation across multiple bridge vaults is out-of-scope
for v1 (tracked as `S24-FOLLOWUP-MULTIVAULT`).

```
_tvlBridgeVault() = _safeUSD(vault.token(), vault.totalLocked())
```

**Fail-closed contract (both)**

Every external call is wrapped in try/catch. Un-wired source address,
zero-address derived token, reverting view, missing aggregator feed,
stale aggregator round, or out-of-policy decimals all yield a 0
contribution. None of these can revert the surrounding `tvlBreakdown`
/ `tvlGlobal` / `tvlBySource` call.

**Monitoring**

`refreshUnpriced()` (extended in S24) checks the derived ZBX feed
(REWARD) and the derived bridge token feed (BRIDGE_VAULT) the same
way it checks AMM / Lending / Staking. A missing feed surfaces the
underlying token in `unpricedTokens()`.

## 4. Off-chain stack

### 4.1 Indexer time-series — `crates/zbx-indexer/src/tvl.rs`

A periodic collector (`snapshot_loop`) calls `tvlBreakdown()` via
`zbx-sdk::Provider::raw_call("eth_call", …)`, decodes the
8-`uint256` return ABI manually (no ethers/alloy dependency), and
inserts one row into `tvl_snapshots`:

```sql
CREATE TABLE tvl_snapshots (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    block_number  INTEGER NOT NULL,
    block_hash    TEXT,
    timestamp     INTEGER NOT NULL,    -- on-chain block.timestamp
    captured_at   INTEGER NOT NULL,    -- indexer wall-clock unix sec
    amm_usd       TEXT    NOT NULL,    -- U256 decimal string, USD-18
    lending_usd   TEXT    NOT NULL,
    stability_usd TEXT    NOT NULL,
    staking_usd   TEXT    NOT NULL,
    reward_usd    TEXT    NOT NULL,
    bridge_usd    TEXT    NOT NULL,
    total_usd     TEXT    NOT NULL,
    oracle_addr   TEXT    NOT NULL,
    UNIQUE (block_number, oracle_addr)
);
```

USD values are stored as decimal-string `TEXT` for lossless `U256`
round-trip — SQLite's `INTEGER` is signed 64-bit and would overflow
above `~1.8e19` USD-18 (≈18.4 USD).

The loop is **fail-soft**: RPC errors, decode errors, and database
write errors are logged at WARN but the loop continues.

### 4.2 REST API — `crates/zbx-indexer/src/server.rs`

| Path | Method | Description |
|---|---|---|
| `/v1/tvl?oracle=0x…` | GET | Latest full snapshot |
| `/v1/tvl/global?oracle=0x…` | GET | Latest `total_usd` only |
| `/v1/tvl/by-source?oracle=0x…` | GET | Latest per-source breakdown |
| `/v1/tvl/history?oracle=0x…&from_ts=…&to_ts=…&page=…&page_size=…` | GET | Paginated time-series |

Conventions:

- USD values are JSON strings (preserve `U256` precision).
- `oracle` filter is case-insensitive.
- `404` when no snapshots exist for the requested oracle.
- `500` on DB errors (logged with full detail server-side).
- Pagination: `page` is 1-indexed; `page_size` clamped to `[1, 1000]`,
  default 100.

### 4.3 CLI — `crates/zbx-cli/src/defi.rs`

```text
zbxctl defi tvl --oracle 0xABC… [--json]
```

Wired against the configured `--rpc-url` via direct JSON-RPC
`eth_call` (no SDK dependency — preserves the existing audit-boundary
policy in `zbx-cli/Cargo.toml`). Default output is a human-readable
aligned table; `--json` emits machine-readable JSON.

Surfaces `eth_call` reverts (e.g. the `PausedQuery()` selector raised
by views while the oracle is paused) with a clear error message rather
than dumping raw revert bytes.

## 5. Rationale

### 5.1 Why USD-18?

The chain's stablecoin (`ZUSD`) is 18-decimal; the price feed standard
(`IZbxAggregatorV3`) is 8-decimal. Picking 18 as the canonical TVL
precision means:

- Direct comparability with `ZUSD`-denominated quantities.
- No precision loss when scaling up from 8-dec feeds.
- Matches the precision of every other on-chain monetary quantity in
  the system, so downstream consumers don't need a context-specific
  decimal-conversion step.

### 5.2 Why fail-closed under-report?

Three options were considered for unpriced/stale tokens:

1. **Revert the whole query.** Rejected: a single misconfigured token
   would brick every dashboard.
2. **Use the last-cached price.** Rejected: silent staleness is the
   Mt. Gox failure mode — by the time the operator notices, the
   reported TVL is already wrong.
3. **Zero out the unpriced contribution and surface the offending
   token via `unpricedTokens()`.** Adopted: the published TVL is a
   verifiable lower bound, and ops gets a machine-readable list of
   tokens that need feeds.

### 5.3 Why no TWAP in v1? — DELIVERED in S23b (ZEP-008)

**Original v1 deferral rationale (preserved for historical context):**

TWAP-aware pricing (manipulation-resistant via the AMM's own
observation slots) is the right long-term answer for sources whose
price discovery is on-chain. It was deferred from v1 to **ZEP-008**
because:

- It requires every AMM pair to be running the observation-slot patch.
- It introduces a `minObservationDuration` parameter that interacts
  with chain-id / block-time decisions we hadn't yet baked in at the
  time of v1 sign-off.

**S23b status (DELIVERED, see §3.7):**

The TWAP integration shipped in S23b as an OPT-IN per-token routing
toggle (`setTwapRoute`), keeping the v1 Chainlink-style aggregator
path as the default. Operators choose, per-token, whether to use the
aggregator (default) or to route via `IZbxTwapOracle` (ZEP-008). This
preserves the v1 deployment posture for tokens with mature aggregator
feeds while unlocking on-chain TWAP pricing for the long tail of
tokens that lack one. See §3.7 for routing semantics, setter
invariants, fail-closed policy, and observability scope.

## 6. Backwards compatibility

`ZbxTvlOracle` is a brand-new contract with no replacement target — it
introduces zero migration burden. The only contract patch is to
`ZbxLendingPool` (additive `reservesCount()` + `getReserveData(address)`
view functions, +17 LOC) and the on-chain Lending interface, which is
non-breaking.

## 7. Security considerations

| Risk | Mitigation |
|---|---|
| Pair-spam DoS on AMM iteration | `maxPairsToScan` cap + `PairScanTruncated` event |
| Stale price → over-reported TVL | `maxStaleness` guard, fail-closed to 0 |
| Misconfigured token decimals (e.g. 255) | `MAX_TOKEN_DECIMALS = 36` clamp, fail-closed |
| Oracle owner compromise | Pausable + view-revert-on-paused; multisig deploy mandatory |
| Reorg breaking `tvl_snapshots` lineage | `block_number` recorded; consumer can detect divergence by joining against the indexer's `blocks` table |
| Indexer time skew (`captured_at` ≠ on-chain `timestamp`) | Both columns stored separately so consumers can choose |

### Audit trail

- Phase 1 architect review: **PASS-WITH-FIXES**, 3 fixes applied
  (MED-1 decimal bounds, MED-2 `refreshUnpriced` interface,
  LOW-3 `pairScanStats`/`PairScanTruncated` event).
- 30 HEVM tests in `contracts/test/ZbxTvlOracle.t.sol`, including
  the four architect-requested cases (non-RAY lending unscale,
  missing-feed → unpriced population, pair-scan truncation visibility,
  out-of-policy decimals fail-closed).
- Off-sandbox verification on VPS srv1266996 (forge build/test/slither/
  bytecode size) is mandatory before mainnet (chain-id 8989) deploy;
  testnet (chain-id 8990) requires the same plus a successful multisig
  handshake script run.

## 8. Reference implementation

| File | LOC | Purpose |
|---|---|---|
| `contracts/interfaces/IZbxTvlOracle.sol` | 126 | Interface (this ZEP §3.1) |
| `contracts/ZbxTvlOracle.sol` | 543 | Reference implementation |
| `contracts/ZbxLendingPool.sol` | +17 | `reservesCount()` + `getReserveData(address)` |
| `contracts/test/ZbxTvlOracle.t.sol` | 581 | 30 HEVM tests, 7 mocks |
| `crates/zbx-indexer/src/tvl.rs` | 346 | Indexer collector + `TvlClient` |
| `crates/zbx-indexer/src/schema.rs` | +25 | `tvl_snapshots` table |
| `crates/zbx-indexer/src/query.rs` | +160 | Query helpers + row types |
| `crates/zbx-indexer/src/server.rs` | (rewrite) | 4 REST routes |
| `crates/zbx-cli/src/defi.rs` | +145 | `Tvl` subcommand |

## 9. Out-of-scope (separately tracked)

- TWAP-aware price source — **ZEP-008**.
- `SOURCE_REWARD` + `SOURCE_BRIDGE_VAULT` implementations — Phase 7.
- Front-end TVL dashboard widget — separate UI work.
- VPS srv1266996 forge-verify + slither — mandatory off-sandbox.

## 10. Copyright

This document is licensed under CC0-1.0.
