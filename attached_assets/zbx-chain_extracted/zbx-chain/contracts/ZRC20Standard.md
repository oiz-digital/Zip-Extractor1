# ZRC-20 Token Standard

**Version**: 1.1 (S16-ZRC20-ADV — adds Freeze, Native Lock, Mint Enable/Disable; see ZEP-006)  
**Chain**: Zebvix Chain (Chain ID 8989 mainnet / 8990 testnet+devnet)  
**Author**: Zebvix Technologies Pvt Ltd  
**Status**: Final  
**Analogous to**: ERC-20 (Ethereum), BEP-20 (BNB Chain), SPL (Solana)

---

## Overview

ZRC-20 is the **official fungible token standard for Zebvix Chain**.  
Any ZRC-20 token is automatically compatible with:
- All ZBX Chain wallets (MetaMask with ZBX network, ZBX Wallet)
- DEX interfaces (ZbxAMM, any Uniswap V2–compatible router)
- Block explorers at zbvix.com/explorer
- The ZBX Bridge (lock-and-mint to other chains)
- Zebvix SDK (zbx-sdk) — `contract.rs` and `abi.rs`

---

## Standard Methods (ERC-20 Compatible)

| Method | Description |
|--------|-------------|
| `name()` | Human-readable token name |
| `symbol()` | Ticker (e.g. "USDT") |
| `decimals()` | Decimal places (18 standard) |
| `totalSupply()` | Total tokens in existence |
| `balanceOf(address)` | Token balance of address |
| `transfer(to, value)` | Send tokens |
| `approve(spender, value)` | Allow spender |
| `allowance(owner, spender)` | Current allowance |
| `transferFrom(from, to, value)` | Spend approved tokens |

---

## ZRC-20 Extensions (Beyond ERC-20)

### 1. Batch Transfer
```solidity
function batchTransfer(address[] calldata to, uint256[] calldata values)
    external returns (bool);
```
Send to up to **512 recipients in one transaction**.  
Saves ~21 000 gas per additional recipient.

### 2. EIP-2612 Permit (Gasless Approvals)
```solidity
function permit(address owner, address spender, uint256 value,
                uint256 deadline, uint8 v, bytes32 r, bytes32 s) external;
function DOMAIN_SEPARATOR() external view returns (bytes32);
function nonces(address owner) external view returns (uint256);
```
Users sign an approval off-chain — no approval transaction needed.  
Enables one-click interactions: sign → swap (single tx).

### 3. On-chain Metadata
```solidity
function logoURI() external view returns (string memory);
function updateLogoURI(string calldata newURI) external; // onlyOwner
function tokenInfo() external view returns (
    string memory name, string memory symbol, uint8 decimals,
    uint256 supply, address owner, string memory logo
);
event LogoURIUpdated(string oldURI, string newURI);
```
All metadata in one call — wallets and explorers can display tokens instantly. `updateLogoURI` persists the new URI on-chain and emits `LogoURIUpdated` (was a silent no-op pre-S16; fixed in ZEP-006).

---

## ZRC-20 v1.1 Advanced Surface (ZEP-006, deployed via `ZRC20Token`)

The base interface (v1.0) is preserved. v1.1 adds three optional extension
interfaces that `ZRC20Token` implements out of the box. Tokens that do
NOT need these can inherit `ZRC20Base` directly and skip them.

### 4. Mint Enable/Disable
```solidity
bool public mintingPaused;     // toggleable
bool public mintingFinalized;  // ONE-WAY — once true, NEVER false

function pauseMinting()    external; // onlyOwner; reverts if finalized OR already paused
function resumeMinting()   external; // onlyOwner; reverts if finalized OR not paused
function finalizeMinting() external; // onlyOwner; reverts if already finalized

event MintingPausedToggled(bool isPaused, address indexed by);
event MintingFinalizedEvent(address indexed by);
```
`mint()` requires both flags to be `false`. `finalizeMinting` is the trust-minimization primitive: after this call, no future mint can ever happen.

### 5. Freeze (Compliance / Sanctions — `IZRC20Freezable`)
```solidity
function freeze(address account)        external; // onlyOwner
function unfreeze(address account)      external; // onlyOwner
function isFrozen(address account)      external view returns (bool);
function frozenBalance(address account) external view returns (uint256);

event Frozen(address indexed account, address indexed by);
event Unfrozen(address indexed account, address indexed by);
```
USDC-style: a frozen account can NEITHER send NOR receive (also blocks mint-to-frozen and burn-from-frozen).

### 6. Native Time-Lock (`IZRC20Lockable`)
```solidity
function lockTokens(address account, uint256 amount, uint64 unlockTime) external; // onlyOwner
function extendLock(address account, uint256 newAmount, uint64 newUnlockTime) external; // onlyOwner; growth-only
function lockedBalanceOf(address account)      external view returns (uint256);
function transferableBalance(address account)  external view returns (uint256);
function lockInfo(address account)             external view returns (uint256 amount, uint64 unlockTime);

event TokensLocked(address indexed account, uint256 amount, uint64 unlockTime);
event LockExtended(address indexed account, uint256 newAmount, uint64 newUnlockTime);
```
Single active lock per account. Auto-expires at `unlockTime` (no on-chain unlock tx required). Locked tokens stay in the holder's wallet but cannot be transferred until unlock — `wallets and explorers SHOULD display `transferableBalance` rather than `balanceOf` for advanced ZRC-20 tokens.

---

## Deploying a ZRC-20 Token

### Option A: Via ZRC20Factory (Recommended)
```solidity
ZRC20Factory factory = ZRC20Factory(0xFactory...);
factory.createToken{value: creationFee}(
    "My Token",           // name
    "MTK",                // symbol
    18,                   // decimals
    1_000_000 * 1e18,     // initial supply
    10_000_000 * 1e18,    // mint cap (0 = unlimited)
    "ipfs://Qm...",       // logo URI
    bytes32(0)            // CREATE2 salt
);
```
**Creation fee**: ~10 ZBX (goes to protocol treasury).

### Option B: Direct Deploy
```solidity
ZRC20Token token = new ZRC20Token(
    "My Token", "MTK", 18,
    1_000_000  * 1e18,  // initial supply (minted to owner in constructor)
    10_000_000 * 1e18,  // mint cap
    "ipfs://Qm...",     // logo
    msg.sender          // owner
);
// initialSupply is minted by the constructor — no separate mint() call needed.
// (Pre-S16 the factory called mint() post-deploy and reverted; ZEP-006 fixes this.)
```

---

## Standard Contract Addresses (Zebvix Mainnet)

| Contract | Address |
|----------|---------|
| ZRC20Factory | TBD (pre-genesis) |
| ZBX (native wrapped) | TBD |
| ZRC20Staking template | TBD |
| ZRC20Airdrop | TBD |
| ZRC20TokenLocker | TBD |

---

## ABI Selectors

| Function | Selector |
|----------|----------|
| `transfer(address,uint256)` | `0xa9059cbb` |
| `approve(address,uint256)` | `0x095ea7b3` |
| `transferFrom(address,address,uint256)` | `0x23b872dd` |
| `balanceOf(address)` | `0x70a08231` |
| `totalSupply()` | `0x18160ddd` |
| `batchTransfer(address[],uint256[])` | `0x88d695b2` |
| `permit(...)` | `0xd505accf` |
| `tokenInfo()` | `0x9d8c5b56` |

---

## Events

| Event | Topic0 |
|-------|--------|
| `Transfer(address,address,uint256)` | `0xddf252ad...` |
| `Approval(address,address,uint256)` | `0x8c5be1e5...` |
| `Permit(...)` | `0x9d8c5b56...` |
| `BatchTransfer(...)` | `0x7fc96d38...` |

---

## Rust Runtime Implementation (Session 38)

ZRC-20 v1.1 is fully implemented on both the Solidity and Rust sides of the chain.

### `crates/zbx-contracts` — `zrc20_token.rs`

Single-token state engine that mirrors `ZRC20Token.sol` exactly. Used by the chain runtime
to enforce ZRC-20 rules natively (without an EVM call) for system-level tokens.

| Type / Function | Description |
|---|---|
| `Zrc20Token` | Full token state struct — balances, allowances, supply, freeze set, lock map, mint flags, pause flag, anti-bot map, 2-step ownership, logo URI |
| `Zrc20Error` | 26 typed error variants matching all Solidity `revert` conditions |
| `LockInfo` | Per-account time-lock data (`amount`, `unlock_time`) |
| `TokenInfo` | Read-only snapshot struct returned by `token_info()` |
| `transfer` / `transfer_from` / `batch_transfer` | Standard + ZRC-20 batch; all legs fire `before_transfer` hook |
| `mint` / `burn` / `burn_from` | Minter-gated; hooks fire; checks `minting_paused` + `minting_finalized` |
| `before_transfer` | Combined check: pause → freeze → native lock → anti-bot |
| `freeze` / `unfreeze` / `is_frozen` / `frozen_balance` | USDC-style per-account blacklist (ZEP-006 §3.1) |
| `lock_tokens` / `extend_lock` / `locked_balance_of` / `transferable_balance` / `lock_info` | Native per-account time-lock (ZEP-006 §3.2) |
| `pause_minting` / `resume_minting` / `finalize_minting` | Mint enable/disable/kill flags (ZEP-006 §3.3) |
| `transfer_ownership` / `accept_ownership` / `renounce_ownership` | 2-step ownership (ZEP-006 §3.4) |
| `update_logo_uri` | Persists URI + returns old value for event emit (ZEP-006 §3.5) |

**Public constants**: `DEFAULT_DECIMALS` (18), `MAX_BATCH_SIZE` (512), `UNLIMITED_CAP` (0)

**Public re-exports** from `zbx-contracts` crate root:
`Zrc20Token`, `Zrc20Error`, `LockInfo`, `TokenInfo`, `DEFAULT_DECIMALS`, `MAX_BATCH_SIZE`, `UNLIMITED_CAP`

**Test coverage**: 42 unit tests in `zrc20_token.rs` — constructor, transfer, approve, batch,
mint cap, burn, freeze (8 paths), lock (10 paths), mint flags, pause, anti-bot, 2-step ownership, logo URI.

### `crates/zbx-pool` — `token_factory.rs` (upgraded)

The multi-token factory registry was upgraded in Session 38 to mirror all ZEP-006 v1.1 fields:

| Addition | Description |
|---|---|
| `TokenRecord.logo_uri` | On-chain logo URI (stored at creation, updatable) |
| `TokenRecord.minting_paused` | Toggleable mint pause |
| `TokenRecord.minting_finalized` | One-way permanent mint kill switch |
| `TokenFactory.frozen_accounts` | `HashMap<(token, account), bool>` — per-token freeze state |
| `TokenFactory.token_locks` | `HashMap<(token, account), LockEntry>` — per-token time-lock |
| `CreateTokenParams.logo_uri` | Logo URI passed through on factory-create |
| 13 new error variants | `AccountFrozen`, `MintingPaused`, `MintingFinalized`, `ActiveLockExists`, etc. |
| 12 new operations | freeze/unfreeze, lock/extend-lock, pause/resume/finalize-mint, update-logo-uri, is-frozen, locked-balance, transferable-balance, lock-info |

**Test coverage**: 18 new unit tests in `token_factory.rs` — all ZEP-006 factory-level operations.

---

## TVL Reporting (ZEP-007)

Tokens deployed under this standard appear in the chain's canonical TVL
aggregate via `ZbxTvlOracle` (see `docs/proposals/ZEP-007-TVL-ORACLE.md`).
The aggregator inspects each ZRC-20's holdings inside the four production
sources (AMM, lending, stability, staking) and converts to USD-18 using
the per-token feed registered with `IZbxAggregatorV3`.

To make a new ZRC-20 TVL-visible:

1. Register a price feed for the token via
   `ZbxTvlOracle.setPriceFeed(token, feed)` (owner only).
2. Confirm the token does NOT appear in `unpricedTokens()` after the
   next `refreshUnpriced()` call.
3. Verify via REST: `GET /v1/tvl/by-source?oracle=0x<oracle>`.
4. Verify via CLI: `zbxctl defi tvl --oracle 0x<oracle>`.

Tokens with no registered feed contribute **zero** to TVL (fail-closed)
and are surfaced via `unpricedTokens()` for operator monitoring — the
aggregate query never reverts on a missing feed.

## Security Checklist

Before deploying a ZRC-20 token:

- [ ] Audit `mint` access control (only trusted minters)
- [ ] Set a reasonable `mintCap` (avoid unlimited minting)
- [ ] Verify `permit` deadline is in the future
- [ ] Test `batchTransfer` with boundary cases (512 recipients)
- [ ] Emit `Transfer(address(0), to, amount)` from `_mint`
- [ ] Consider `renounceOwnership` after setup for decentralization
- [ ] Lock team tokens via `ZRC20Vesting` (separate escrow) or `lockTokens` (native)
- [ ] **v1.1**: If supply is meant to be fixed, call `finalizeMinting()` after initial mint
- [ ] **v1.1**: If issuing a regulated asset, document the freeze runbook publicly
- [ ] **v1.1**: If team tokens use native lock, document unlock schedule on-chain
- [ ] Use a multisig (not EOA) for `owner_` in production deployments