# ZEP-006 — ZRC-20 Advanced Features

| Field | Value |
|---|---|
| **Number** | ZEP-006 |
| **Title** | ZRC-20 Advanced — Freeze, Native Lock, Mint Enable/Disable |
| **Author** | Zebvix Technologies Pvt Ltd |
| **Status** | Final |
| **Type** | Standards Track |
| **Created** | 2026-05-01 |
| **Replaces** | none (extends ZRC-20 v1.0 → v1.1 in `ZRC20Standard.md`) |
| **Requires** | none |
| **Audit** | architect review pending; no external audit |

---

## 1. Abstract

This proposal upgrades the user-deployable `ZRC20Token` reference contract
from a "mid-tier ERC-20-superset" to a "compliance-grade modern token" by
adding three feature families plus two deployment-bug fixes:

1. **Freeze (USDC-style blacklist)** — owner can freeze any account; frozen
   accounts cannot send, receive, mint-to, or burn-from.
2. **Native time-lock** — owner can lock a portion of a holder's balance
   until a future timestamp. Locked tokens stay in the holder's wallet but
   cannot be transferred until `unlockTime`. Auto-expires (no on-chain
   unlock tx required).
3. **Mint enable/disable** — two flags:
   - `mintingPaused` (toggleable) — temporary pause
   - `mintingFinalized` (one-way) — permanent kill switch for trustless tokens
4. **Constructor-mint** for `initialSupply` (closes a critical
   `ZRC20Factory::createToken` bug where every token deployment with
   `initialSupply > 0` reverted because the factory was never a minter).
5. **`updateLogoURI` no-op fix** — was a silent comment-only stub; now
   persists via new `_setLogoURI` internal helper in `ZRC20Base` and emits
   a new `LogoURIUpdated` event.

The `ZRC20Base` abstract token, the wrapped `ZRC20.sol`, and all auxiliary
contracts (`ZRC20Factory`, `ZRC20Vesting`, `ZRC20Staking`, `ZRC20Airdrop`,
`ZRC20TokenLocker`) are NOT modified beyond the two bug-fix edits — feature
additions are concentrated in `ZRC20Token.sol`.

The wrapped bridge token `ZRC20.sol` will receive the same freeze surface
under a separate ZEP after careful evaluation against bridge security
constraints (lock-and-mint flow, validator-attested unlocks).

## 2. Motivation

The S15-P2-FINAL audit (closed with architect PASS for ZUSD redeem
hardening) prompted the user to ask: "kya ZRC-20 full advance hai —
mint, freeze, locked?" A full sweep returned ~54/100: solid ERC-20 core,
EIP-2612 permit, batch transfer, mint cap, burn, transfer pause, anti-bot,
vesting/airdrop/staking/lock as separate contracts — but NO native freeze,
NO native lock, NO mint enable/disable, plus two critical deployment bugs.

Without freeze, the standard cannot serve any compliance-required asset
(stablecoins, tokenized securities, geo-restricted utility tokens).
Without native lock, every team-token / vesting / cliff scenario must be
implemented via the separate `ZRC20TokenLocker` escrow vault, which
requires holders to interact with two contracts and forces wallets and
explorers to query both for an honest "spendable balance" display.
Without mint enable/disable, projects that promise a fixed supply have no
on-chain way to back the promise — they can only socially commit not to
mint more.

The two bug fixes are non-negotiable: the factory bug bricks every
factory deployment with non-zero initial supply, and the no-op
`updateLogoURI` silently misleads any caller who thinks they updated the
on-chain logo metadata.

## 3. Specification

### 3.1 Freeze (`IZRC20Freezable`)

```solidity
event Frozen(address indexed account, address indexed by);
event Unfrozen(address indexed account, address indexed by);

function freeze(address account) external;            // onlyOwner
function unfreeze(address account) external;          // onlyOwner
function isFrozen(address account) external view returns (bool);
function frozenBalance(address account) external view returns (uint256);
```

**Semantics**

- `freeze(0)` reverts. `freeze` of an already-frozen account reverts.
- `unfreeze` of a non-frozen account reverts.
- A frozen account cannot **send**, **receive**, be **minted to**, or be
  **burned from**. The `_beforeTransfer` hook checks both `_frozen[from]`
  and `_frozen[to]` for every movement.
- `frozenBalance(a)` returns `balanceOf(a)` if frozen, else `0`. The
  field is informational; it does not change the underlying balance.

### 3.2 Native time-lock (`IZRC20Lockable`)

```solidity
struct LockInfo { uint256 amount; uint64 unlockTime; }

event TokensLocked(address indexed account, uint256 amount, uint64 unlockTime);
event LockExtended(address indexed account, uint256 newAmount, uint64 newUnlockTime);

function lockTokens(address account, uint256 amount, uint64 unlockTime) external;
function extendLock(address account, uint256 newAmount, uint64 newUnlockTime) external;
function lockedBalanceOf(address account) external view returns (uint256);
function transferableBalance(address account) external view returns (uint256);
function lockInfo(address account) external view returns (uint256, uint64);
```

**Semantics**

- Single active lock per account. Once `block.timestamp >= unlockTime`,
  the lock is implicitly expired and a fresh `lockTokens` call may
  replace it freely (smaller amount and shorter time both allowed).
- While a lock is active, only `extendLock` may modify it, and both
  `newAmount` and `newUnlockTime` must be `>=` their current values.
- `lockedBalanceOf` is computed lazily from `(amount, unlockTime)` and
  storage stays cold across the auto-expiry boundary — no garbage-collect
  tx needed.
- `transferableBalance(a) = max(0, balanceOf(a) - lockedBalanceOf(a))`.
  Saturates at zero for defensive reasons (cannot underflow via normal
  flows because the `_beforeTransfer` invariant blocks transfers that
  would).
- The `_beforeTransfer` hook enforces
  `balanceOf(from) - lockedBalanceOf(from) >= value`. Mint (`from = 0`)
  is exempt from the lock check (the hook skips when `from == 0`).
  **Burn IS subject to the lock check** — burning the locked portion
  reverts. Rationale: without lock-on-burn, a holder could circumvent a
  time-lock by burning their locked tokens and asking the issuer to
  re-issue an equivalent amount via airdrop/governance, defeating the
  lock's purpose. Issuer-driven seizure (USDC-style "destroyBlackFunds")
  is a separate primitive not included in this ZEP.
- The `batchTransfer` extension MUST route every individual leg through
  `_transfer` so each leg fires `_beforeTransfer` / `_afterTransfer`.
  The per-leg lock check is correct under serial debits: leg `i` sees
  the running post-debit balance from legs `0..i-1`, so any batch whose
  total would dip into the locked portion reverts atomically on the leg
  that crosses the boundary. (See `ZRC20Base::batchTransfer`, fixed in
  S16-ZRC20-ADV CRIT-1.)

### 3.3 Mint enable/disable

```solidity
bool public mintingPaused;     // toggleable
bool public mintingFinalized;  // one-way; once true, NEVER false

event MintingPausedToggled(bool isPaused, address indexed by);
event MintingFinalizedEvent(address indexed by);

function pauseMinting()    external;  // onlyOwner; reverts if finalized OR already paused
function resumeMinting()   external;  // onlyOwner; reverts if finalized OR not paused
function finalizeMinting() external;  // onlyOwner; reverts if already finalized
```

**Semantics**

- `mint()` requires both flags to be `false`.
- `finalizeMinting` is the trust-minimization primitive: after this
  call, no future mint can ever happen, regardless of owner role,
  minter set, or governance — `addMinter`/`removeMinter` continue to
  function but are moot.
- Pause and finalize are mutually exclusive in a "useful" sense: once
  finalized, neither pause nor resume can be called (both revert).
  Pause-then-finalize is allowed (and locks the chain in the
  permanently-disabled state).

### 3.4 Constructor initial-supply

```solidity
constructor(
    string memory name_, string memory symbol_, uint8 decimals_,
    uint256 initialSupply_,            // NEW
    uint256 mintCap_,
    string memory logoURI_, address owner_
)
```

`initialSupply_` is minted to `owner_` in the constructor, before any
external code can run. Requires `initialSupply_ <= mintCap` (where the
resolved `mintCap` is `mintCap_` or `type(uint256).max` if `mintCap_ == 0`).

The `ZRC20Factory::createToken` flow now passes `initialSupply` to the
constructor and **does not** make any post-deploy `mint()` call. The
previous post-deploy `mint()` always reverted because the factory was
never added to `_minters` on the freshly-deployed token.

### 3.5 `updateLogoURI`

`ZRC20Base` exposes a new `internal virtual` helper:

```solidity
function _setLogoURI(string memory newURI) internal virtual {
    string memory old = _logoURI;
    _logoURI = newURI;
    emit LogoURIUpdated(old, newURI);
}
```

Both `ZRC20.sol` and `ZRC20Token.sol` expose `updateLogoURI` as
`onlyOwner` and delegate to `_setLogoURI`. The new
`event LogoURIUpdated(string oldURI, string newURI)` is added to
`IZRC20.sol` so all wallets/indexers can observe metadata changes.

## 4. Interface ↔ Implementation parity

| Interface | Functions | Events | Both in `ZRC20Token.sol`? |
|---|---|---|---|
| `IZRC20`           | 14 (incl. permit, batch, tokenInfo) | 5 (Transfer, Approval, Permit, BatchTransfer, **LogoURIUpdated** ✱) | ✓ |
| `IZRC20Mintable`   | 5 (mint, mintCap, isMinter, addMinter, removeMinter) | 4 | ✓ |
| `IZRC20Burnable`   | 3 (burn, burnFrom, totalBurned) | 1 | ✓ |
| `IZRC20Freezable`  | 4 (freeze, unfreeze, isFrozen, frozenBalance) | 2 | ✓ NEW |
| `IZRC20Lockable`   | 5 (lockTokens, extendLock, lockedBalanceOf, transferableBalance, lockInfo) | 2 | ✓ NEW |

✱ `LogoURIUpdated` is emitted from `ZRC20Base::_setLogoURI`, inherited by
all subclasses; declared on the base interface.

## 5. Security analysis

### 5.1 Threat model

- **Compliant issuer compromise** — owner key compromise is catastrophic
  (attacker can freeze legitimate users, lock tokens indefinitely, mint
  to attacker). Mitigation: issuer SHOULD use a multisig or governance
  contract as `owner` and call `finalizeMinting` once issuance is
  complete. (Out-of-scope for this ZEP; recommended in the §6 deployment
  checklist.)

- **Malicious lock as denial-of-service** — owner can lock an unwilling
  holder's tokens with arbitrary `unlockTime`. Mitigation: native lock
  is a deliberately privileged primitive (intended for vesting / cliff
  on tokens issuer has agreed to deliver to a beneficiary). Tokens that
  do not want issuer-controlled locks SHOULD `renounceOwnership` after
  setup, which permanently disables `lockTokens`.

- **Active-lock replacement attack** — `lockTokens` reverts on an active
  lock; only `extendLock` can mutate it, and only with growth-only
  semantics. After natural expiry, lock storage is reusable.

- **Underflow via large value** — `transferableBalance` saturates at 0
  via explicit `bal > locked ? bal - locked : 0`. The
  `_beforeTransfer` lock check is rearranged
  (`bal >= locked && bal - locked >= value`) to dodge underflow even if
  the storage somehow drifted (which it cannot via documented flows).

- **Mint-to-frozen as supply-locking** — owner cannot mint to a frozen
  account (compliance correctness). Cannot use freeze + mint to "trap"
  an attacker because the attacker is the recipient, not the source —
  the attacker simply receives nothing and the supply is not increased.

### 5.2 Hook coverage matrix

The `_beforeTransfer` / `_afterTransfer` hooks fire on **every** balance
movement, not just `transfer()`. This is the load-bearing invariant for
freeze, native lock, transfer-pause, and anti-bot enforcement:

| Path | `_beforeTransfer(from, to, value)` fires? |
|---|---|
| `transfer`                       | ✓ via `_transfer` |
| `transferFrom`                   | ✓ via `_transfer` |
| `permit` then `transferFrom`     | ✓ via `_transfer` |
| `batchTransfer` (each leg)       | ✓ via per-leg `_transfer` (S16 fix) |
| `mint` (from = address(0))       | ✓ via `_mint` (S16 fix) |
| `burn` (to = address(0))         | ✓ via `_burn` (S16 fix) |
| `burnFrom` (to = address(0))     | ✓ via `_burn` (S16 fix) |

Pre-S16, `batchTransfer`, `_mint`, and `_burn` wrote `_balances` directly
without invoking the hooks. This bypassed every advanced policy in
`ZRC20Token`. Architect review of S16-ZRC20-ADV caught this as CRIT-1 +
CRIT-2 and the v1.1 base now routes all paths through the hooks.

### 5.3 Storage layout

`ZRC20Token` adds these slots to the layout:

```
+ bool    mintingPaused
+ bool    mintingFinalized
+ mapping(address => bool)        _frozen
+ mapping(address => LockInfo)    _locks
```

Existing slots (`owner`, `_minters`, `mintCap`, `_totalBurned`, `paused`,
`maxTransferAmount`) are unchanged. **This is a breaking change** for any
deployed `ZRC20Token` instance (old factories will deploy old layout;
upgrade requires a new deployment, which is normal for non-upgradeable
tokens).

### 5.4 Gas overhead per transfer

- 2× SLOAD for `_frozen[from]` and `_frozen[to]` (~2 100 cold / 100 warm).
- 1× SLOAD for `_locks[from]` IF `from != address(0)` (~2 100 cold / 100 warm).
- 1× SLOAD for `paused` / `mintingPaused` / `mintingFinalized` flag reads
  (warm after 1st access in tx). The hook is read-only; no SSTORE is
  performed by `_beforeTransfer` itself.
- Total worst case (cold path): ~6 300 gas overhead per transfer.

This is acceptable for a compliance-grade token; ultra-high-throughput
tokens may inherit `ZRC20Base` directly and skip the advanced features.

## 6. Deployment checklist (mainnet)

Before deploying a `ZRC20Token` to mainnet:

- [ ] `owner_` is a multisig or governance contract, not an EOA
- [ ] `mintCap_` is set to the intended max supply (avoid `0 = unlimited`)
- [ ] `initialSupply_` matches the documented genesis distribution
- [ ] If supply is meant to be fixed: call `finalizeMinting()` after
      initial mint completes
- [ ] If team tokens are vesting: call `lockTokens(team, amount, cliff)`
      and document the unlock schedule publicly
- [ ] Verify `logoURI` resolves to a 256×256 PNG over HTTPS or IPFS
- [ ] `pause()` is intentionally accessible — document the runbook for
      when it would be invoked (typically: discovery of an exploit in a
      paired protocol like an AMM)
- [ ] Consider `renounceOwnership` after all setup completes, for
      maximum trustlessness (incompatible with future freeze/lock use)
- [ ] External audit by a reputable firm (mandatory pre-mainnet)

## 7. Rationale and rejected alternatives

- **Why single active lock per account, not multi-lock?** — Multi-lock
  doubles the per-transfer SLOAD cost (must iterate the array) and
  requires garbage-collect logic. Single-lock with growth-only updates
  covers all real-world vesting patterns (cliff, linear, and stepwise
  via `extendLock`) and the separate `ZRC20TokenLocker` escrow contract
  serves the multi-lock case for advanced users.

- **Why freeze blocks both send AND receive?** — USDC's blacklist
  semantics. Allowing receive-while-frozen creates a "trap address"
  attack surface where an attacker can move sanctioned funds to a
  not-yet-frozen address that was previously frozen. Symmetric blocking
  is the conservative choice.

- **Why finalizeMinting one-way?** — A reversible "finalize" provides
  no trust gain over `pauseMinting`. The whole point of finalize is the
  immutability proof: once the on-chain flag is set, no future tx of
  any kind from any caller can re-enable mint. This requires the
  one-way semantics.

- **Why no `Ownable2Step`?** — Out of scope. Tracked for next
  maintenance pass alongside the same migration on `ZRC20.sol` and
  `ZRC20Factory.sol`.

- **Why no ERC-20Votes / Snapshots / Flash mint?** — Future ZEP-007
  (Governance-grade ZRC-20). These are large additions with their own
  test surface; bundling them here would push this ZEP over a healthy
  review-window size.

## 8. Backwards compatibility

- **Source-level**: `ZRC20Token` constructor signature changed from 6
  args to 7 args (`initialSupply_` inserted as 4th arg). Any deployer
  scripts using the old signature MUST update.
- **Bytecode-level**: Storage layout extended (new slots appended).
  Existing deployed instances are NOT upgradeable; new features only
  apply to NEW deployments.
- **Interface-level**: `IZRC20` adds a new event (`LogoURIUpdated`) which
  is purely additive and does not break old consumers. Two new
  interfaces (`IZRC20Freezable`, `IZRC20Lockable`) are opt-in via
  `supportsInterface`. EIP-165 wiring is fully implemented in Session 38:
  `ZRC20Token.supportsInterface` returns `true` for all five interfaces
  (`IZRC20`, `IZRC20Mintable`, `IZRC20Burnable`, `IZRC20Freezable`,
  `IZRC20Lockable`), and `ZRC20.supportsInterface` returns `true` for
  three (`IZRC20`, `IZRC20Mintable`, `IZRC20Burnable`, `IZRC20Freezable`).

## 9. References

- `contracts/ZRC20Base.sol`
- `contracts/ZRC20Token.sol`
- `contracts/ZRC20Factory.sol`
- `contracts/ZRC20.sol`
- `contracts/interfaces/IZRC20.sol`
- `contracts/interfaces/IZRC20Freezable.sol` (NEW)
- `contracts/interfaces/IZRC20Lockable.sol` (NEW)
- `contracts/interfaces/IZRC20Mintable.sol`
- `contracts/interfaces/IZRC20Burnable.sol`
- `contracts/test/ZRC20TokenAdvanced.t.sol` (NEW, 46 tests; 8 hook-coverage adversarial tests added in S16-ZRC20-ADV-CRIT response, plus 1 symmetry test for transferFrom-to-frozen)
- `contracts/ZRC20Standard.md` (updated to v1.1)
- `AUDIT_2026-04-30.md` — S16-ZRC20-ADV block
