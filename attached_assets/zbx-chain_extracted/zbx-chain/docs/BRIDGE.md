# Zebvix Bridge

> **⚠ Session 13 caveat — read before any bridge use:**
> - Production chain_id is **8989** (mainnet) / **8990** (devnet) — earlier docs that
>   say `7878` are stale.
> - **`S11-BRIDGE-SOL-OUT1` (CRITICAL, CLOSED S17)** — nonce-collision vulnerability
>   fixed: `submit()` checks content-hash ID in both `pending` and `completed` maps.
>   Bridge contracts must still be deployed to **BSC TESTNET only** until a full
>   external audit is completed. See `docs/proposals/DEVNET-LAUNCH-PLAN-2026-05-01.md`.
> - **Session 21**: Multi-token support added — bridge now supports ZBX, ZUSD, ZBXBTC,
>   ZBXETH, ZBXUSDC with per-token daily limits and whitelist validation.

Cross-chain bridge connecting Zebvix Chain (ZBX, Chain ID 8989 mainnet / 8990 devnet) to:

---

## Supported Networks

| Network | Chain ID | Status | Required Confirmations |
|---------|----------|--------|------------------------|
| **Ethereum Mainnet** | 1 | Production (testnet phase) | **12 blocks** (~2.5 min) |
| **BNB Chain (BSC)** | 56 | Production (testnet phase) | **20 blocks** (~60s) |
| **Polygon** | 137 | Production (testnet phase) | **128 blocks** (~4 min) |

> **Not yet supported**: Solana, Arbitrum, Optimism — these are in the v0.5 roadmap.
> The required confirmations per chain are enforced in `ChainId::required_confirmations()`.

---

## Supported Tokens

ZBX Chain has **two native tokens** — both fully supported on the bridge:

| Token | Type | Max per Transaction | Daily Limit | Bridge Model |
|-------|------|---------------------|-------------|--------------|
| **ZBX** | Native protocol token | 1,000,000 ZBX | 10,000,000 ZBX/day | Lock-and-Mint |
| **ZUSD** | USD stablecoin (native) | 5,000,000 ZUSD | 50,000,000 ZUSD/day | Lock-and-Mint |

Both use **Lock-and-Mint**: tokens are locked in escrow on ZBX Chain
and wrapped equivalents are minted on the target chain (Ethereum / BSC / Polygon).

> Both tokens can also pay ZBX Chain gas fees (`gas_token` field in transaction).

### Bridge Models Explained

**Lock-and-Mint** (used for native ZBX):
```
Source chain: user's ZBX locked in BridgeVault escrow
Dest chain:   wrapped WZBX ERC-20 minted 1:1 to recipient
Reverse:      burn WZBX → unlock native ZBX from escrow
```

**Burn-and-Mint** (used for ZUSD, ZBXBTC, ZBXETH, ZBXUSDC):
```
Source chain: token burned (permanently destroyed)
Dest chain:   equivalent token minted fresh
Reverse:      burn on dest → mint on source
Total supply stays constant across the bridge
```

---

## Architecture

```
ZBX Chain                          External Chain
─────────────────────────────      ────────────────────
BridgeVault.sol                    BridgeVault.sol
  ↑ lock / burn token                ↑ unlock / mint token
        ↑                                    ↑
  zbx-bridge/relayer.rs  ←────────  bridge-relayer (off-chain)
        ↑                                    ↑
  TokenWhitelist                     3-of-5 Multisig
  DailyLimitTracker                  BridgeMultisig.sol
```

### Core Rust Modules

```
crates/zbx-bridge/src/
  ├─ lib.rs        — ChainId enum, constants, TryFrom<u64>
  ├─ token.rs      — BridgeToken, TokenWhitelist, DailyLimitTracker
  ├─ relayer.rs    — BridgeRequest (multi-token), BridgeRelayer, BridgeAction
  ├─ multisig.rs   — 3-of-5 strict multisig (InvalidSignature vs InsufficientConfirmations)
  ├─ proofs.rs     — Merkle receipt proof verification
  └─ error.rs      — BridgeError enum
```

---

## Flow: ZBX Chain → Ethereum (Deposit)

1. User calls `BridgeVault.bridgeOut(token, amount, destChain=1, destAddr)`
2. Token locked (ZBX) or burned (ZUSD/ZBXBTC/etc) in vault.
3. `DepositEvent` emitted with token address + amount.
4. Relayers observe event, independently sign `keccak(id ‖ token ‖ from ‖ to ‖ amount)`.
5. Each relayer calls `BridgeRelayer::confirm(id, signer, sig, timestamp)`.
6. After 3-of-5 confirmations: `confirm()` returns `Ok(true)`.
7. Relayer attaches Merkle receipt proof via `set_proof()`.
8. `BridgeRelayer::execute(id, receipts_root, timestamp)` runs final checks:
   - Not expired (24h TTL)
   - 3-of-5 multisig threshold verified
   - Merkle receipt proof valid
9. Returns `BridgeAction::MintOnTarget { target_chain, token, recipient, amount }`.
10. Execution layer mints wrapped token on Ethereum for `destAddr`.

**Confirmation time**: ~5 ZBX blocks (25s) + 12 Ethereum blocks (~2.5 min)

---

## Flow: Ethereum → ZBX Chain (Withdrawal)

1. User burns wrapped token on Ethereum.
2. `BurnEvent` emitted.
3. Relayers observe, sign, confirm 3-of-5.
4. `execute()` returns `BridgeAction::UnlockOnZbx { token, recipient, amount }`.
5. Execution layer unlocks (ZBX) or mints (ZUSD/ZBXBTC/etc) on ZBX Chain.

---

## Bridge Fees

| Route | Fee | Destination |
|-------|-----|-------------|
| Any token → Any chain | **0.1%** (10 bps) | Protocol treasury |

Fee is deducted from `amount` before computing `net_amount`. The `BridgeRequest.fee`
field records the exact fee for auditability.

---

## Security Model

| Protection | How it works |
|---|---|
| **3-of-5 strict multisig** | Any invalid/unknown/duplicate sig rejects entire batch (`InvalidSignature`). Valid-but-insufficient sigs return `InsufficientConfirmations` (wait for more). |
| **Merkle receipt proof** | `execute()` requires proof attached via `set_proof()` — no proof = no execution. |
| **Per-token daily limits** | `DailyLimitTracker` — caps total outflow per token per UTC day. Resets at midnight. |
| **Per-tx max amounts** | `TokenWhitelist.max_per_tx` — rejects single oversized transactions. |
| **Token whitelist** | Only whitelisted+enabled tokens can be bridged. Admin can `disable()` a token instantly. |
| **Request TTL (24h)** | `is_expired()` checked in `confirm()` and `execute()`. `expire_stale()` clears stuck requests. |
| **Replay protection** | Request ID = `keccak256(token ‖ from ‖ to ‖ amount ‖ block ‖ ts ‖ chain ‖ type)` — computationally infeasible to collide. Checked in both `pending` and `completed` maps. |
| **Duplicate rejection** | `submit()` rejects if same ID already in `pending` or `completed`. |
| **Emergency pause** | `set_paused(true)` stops all new `submit()` calls globally. |
| **Chain ID validation** | `ChainId::try_from(u64)` rejects unknown chain IDs with `UnsupportedChain`. |

---

## Error Reference

| Error | When | Action |
|-------|------|--------|
| `TokenNotWhitelisted` | Token not in whitelist | Use a supported token |
| `TokenDisabled` | Token temporarily paused | Wait for re-enable |
| `ExceedsMaxPerTx` | Amount > per-tx limit | Split into smaller transfers |
| `DailyLimitExceeded` | Daily cap hit | Wait until UTC midnight reset |
| `InvalidSignature` | Bad/unknown/duplicate sig | Investigate relayer — reject whole batch |
| `InsufficientConfirmations` | Valid sigs, need more | Wait for more relayer signatures |
| `Expired` | Request > 24h old | Re-submit the request |
| `ProofInvalid` | No proof or bad Merkle proof | Attach valid receipt proof |
| `DuplicateRequest` | Same content-hash already pending | Already submitted |
| `Paused` | Bridge globally paused | Wait for unpause |
| `UnsupportedChain` | Unknown chain ID | Use ETH(1), BSC(56), or Polygon(137) |

---

## Adding a New Token (Bridge Admin)

```rust
relayer.register_token(BridgeToken {
    address:     [0xAB; 20],          // token contract on ZBX Chain
    symbol:      "MYTOKEN".into(),
    decimals:    18,
    max_per_tx:  100_000 * 10u128.pow(18),
    daily_limit: 1_000_000 * 10u128.pow(18),
    is_native:   false,               // Burn-and-Mint model
    enabled:     true,
});
```

---

## Running a Bridge Relayer

```bash
zbx bridge-relayer \
  --zbx-rpc    https://rpc.zbvix.com \
  --eth-rpc    https://mainnet.infura.io/v3/KEY \
  --bsc-rpc    https://bsc-dataseed.binance.org \
  --poly-rpc   https://polygon-rpc.com \
  --private-key /path/to/relayer.key \
  --chains     ethereum,bsc,polygon
```

---

## Contract Addresses (Testnet)

| Contract | Chain | Address |
|----------|-------|---------|
| BridgeVault | ZBX Testnet (8990) | TBD |
| BridgeVault | Ethereum Sepolia | TBD |
| BridgeVault | BSC Testnet | TBD |
| BridgeVault | Polygon Mumbai | TBD |
| BridgeMultisig | ZBX Testnet | TBD |
| WZBX (ERC-20) | Ethereum | TBD |
| WZBX (BEP-20) | BSC | TBD |
| WZBX (ERC-20) | Polygon | TBD |

---

## Roadmap

| Version | Feature |
|---------|---------|
| v0.1 (now) | ZBX, ZUSD, ZBXBTC, ZBXETH, ZBXUSDC — ETH/BSC/Polygon |
| v0.2 | External audit + BSC mainnet launch |
| v0.3 | Fraud proof challenge window (7 days) |
| v0.4 | Bridge sunset — XCL replaces bridge for trustless transfers |
| v0.5 | Solana, Arbitrum, Optimism support |
