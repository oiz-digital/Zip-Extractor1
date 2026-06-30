# Zebvix Chain — Production-Readiness Phase Plan

**Document ID:** PHASE-PLAN-2026-05-01
**Author:** Audit (Session 12)
**Date:** 2026-05-01
**Status:** Draft for node-team review
**Source data:** `AUDIT_2026-04-30.md` (Sessions 1–11), `docs/proposals/S7-ARCH1-vm-consolidation.md`, `docs/proposals/S7-EVM3-call-family-implementation.md`, ZEPs 000–004, plus structural sweep of all 66 crates and `node/zbx-node`.

---

## 1. Purpose

This document is the single roadmap for getting Zebvix Chain from its current state to a production-ready PoS L1 capable of being trusted with user funds. It consolidates:

- Every deferred finding from Sessions 1–11 of the rolling audit
- Every "real but unwired" crate identified by Session 12 structural sweep
- Every duplicate/triplicate implementation that needs consolidation
- Every operational gap (CI, fuzz, hardfork plumbing, slashing monitor)

It is sequenced into **five phases (P0–P4)** with per-task effort estimates, dependencies, files affected, and acceptance criteria. Phases are designed to be parallelizable across 2–4 engineers.

**Total estimated effort:** **~92–125 developer-days** (33 tasks) = ~18–25 calendar weeks for a single engineer, **or ~7–10 calendar weeks with 3–4 engineers** working in parallel. *(Revised post-architect-review: prior figures of 70–95 / 82–114 were under-estimates; see §9 for the rebaselined breakdown.)*

---

## 2. How to read this document

Each task uses the format:

```
P{phase}-T{##}  [SEVERITY]  ({effort-days})
  Title
  Depends-on:  P{x}-T{##}, …
  Files:       …
  Acceptance:  …
  Why:         …
```

Severity levels:

| Severity | Meaning |
|----------|---------|
| **MAINNET-BLOCKER** | Cannot launch mainnet with this open. Loss-of-funds or chain-halt risk. |
| **CRITICAL** | Loss-of-funds risk OR consensus risk. Fix before any production rollout. |
| **HIGH** | User-impact bug, fixable by upgrade but should not ship. |
| **MEDIUM** | Operational risk, defense-in-depth gap, or feature incompleteness. |
| **LOW** | Hygiene, gas, code clarity. |

Effort days assume one experienced Rust + Solidity engineer, including code, tests, review, and merge.

---

## 3. Executive summary — the real picture

After the Session 12 structural sweep, the picture is clearer than earlier sessions implied:

### Good news

1. **HotStuff BFT consensus is REAL and WIRED.** `zbx-consensus` (2,303 LOC) implements full 3-phase HotStuff with VRF-weighted proposer selection, pacemaker (liveness), safety rules, vote/QC machinery, and a round manager. It is one of the 13 crates the production node binary depends on.
2. **Most "unwired" infrastructure crates are REAL implementations**, not stubs. `zbx-fee` is a real EIP-1559 module. `zbx-pruner` is a real state pruner. `zbx-rewards` has a real block-reward + halving engine. `zbx-state-rent` implements ZEP-008. `zbx-bundler` is a real ERC-4337 bundler with the canonical entry-point address. `zbx-oracle` is a real Chainlink-style decentralized oracle. The gap is **integration, not implementation.**
3. The production EVM (`zbx-evm`) had its CRITICAL silent-NOP InvalidOpcode bug closed in Session 10. Bridge contracts have BLS+FROST 2-of-3 multisig with pause guards (post Session 6 work).
4. Strong hygiene markers: `--allow-chain-mismatch` escape hatch, fail-fast genesis check, `block_producer` reorg/equivocation hooks, `tracing` everywhere.

### Bad news

1. **Production node binary `node/zbx-node` directly depends on only 13 of 66 crates (20%).** The 53 unwired crates include critical infrastructure (state-sync, pruner, fee market, rewards distribution, oracle, MEV protection, ERC-4337 bundler, light-client server, indexer, state-rent).
2. **Three competing networking implementations** (`zbx-network` 911 LOC wired, `zbx-p2p` 975 LOC orphan, `zbx-net` 1316 LOC orphan, `zbx-gossip` 461 LOC orphan = 3,663 LOC of dead networking code). The wired one is the simplest; the orphans have discv5, NAT traversal, and GossipSub respectively.
3. **`zbx-finality` is a confusing duplicate.** It's a Casper-FFG-style finality gadget (104 LOC, 4 files) that overlaps with HotStuff's built-in QC finality from `zbx-consensus`. Likely leftover from a design pivot. Must be **deleted or merged**, not built up.
4. **Three production-blocker findings unfixed** from Sessions 7 and 11:
   - **S7-PROD1** [MAINNET-BLOCKER] — production block-producer writes `tx_root = [0u8; 32]`. Consensus-incompatible against any external verifier.
   - **S7-EVM3** [MAINNET-BLOCKER] — CALL/CREATE/DELEGATECALL/STATICCALL/CALLCODE/CREATE2/SELFDESTRUCT/REVERT all missing from BOTH `zbx-evm` and `zbx-zvm` byte-dispatch tables. Plan-doc exists (`S7-EVM3-call-family-implementation.md`, 11 workstreams, ~3,500 LOC, ~16 days).
   - **S11-BRIDGE-SOL-OUT1** [CRITICAL] — production BSC `ZbxBridge.bridgeOut` nonce-collision can cause silent fund loss.
5. **`zbx-bridge` Rust crate is orphaned AND has 6 conditional CRITICAL/HIGH bugs** (Session 11). Currently dead code, but anyone reading it as "the Rust bridge" absorbs the bugs as live.
6. **No CI integration test harness verified** for VMs. Sandbox blocks rocksdb tests (SIGKILL via mmap limit), so all integration work happens in node-team's local environment.

### What this means

The chain is **closer to launch-ready than the 53-of-66 orphan number suggests**, because most of the "missing" code already exists and just needs wiring. But the three MAINNET-BLOCKER items (P0 below) are non-negotiable, and the wiring effort (P1) is real work that cannot be skipped.

---

## 4. Phase 0 — Mainnet-blocker fixes (~24–32 dev-days)

**Definition:** Anything in this phase, if left open, makes mainnet launch impossible OR creates user-fund-loss risk on day one. **All P0 tasks must be closed before any mainnet rollout.**

### P0-T01  [MAINNET-BLOCKER]  (3 days)
**S7-PROD1 — production block-producer writes `tx_root = [0u8; 32]`**
- Depends-on: none
- Files: `node/src/block_producer.rs`, `crates/zbx-execution/src/lib.rs`, `crates/zbx-trie/src/lib.rs`
- Acceptance:
  1. Block producer computes `tx_root` as keccak256-Merkle-root over RLP-encoded txs (Ethereum-compatible) OR as the chain's chosen alternative, documented in a ZEP.
  2. Same value written into `Block.header.tx_root` and recomputable from `Block.body.txs`.
  3. CI test: `tx_root_matches_recompute` over a fixture block.
  4. RPC `eth_getBlockByNumber` returns a non-zero `transactionsRoot`.
- Why: An all-zero `tx_root` means any external Ethereum-style verifier (block explorers, light clients, bridge relayers) rejects every Zebvix block. Cannot interop. Cannot launch.

### P0-T02  [CRITICAL]  (8–10 days)
**S11-BRIDGE-SOL-OUT1/OUT2/MS1 — production BSC bridge contract fixes**
- Depends-on: none (independent of node work)
- Files: `contracts/ZbxBridge.sol`, `contracts/BridgeMultisig.sol`, plus migration scripts
- Sub-tasks:
  - **a)** OUT1 nonce-collision: replace single-bucket nonce with per-sender counter `mapping(address => uint64) bridgeOutCount` + include `(sender, count)` in nonce derivation.
  - **b)** OUT2 source-chain binding: add `uint256 immutable sourceChainId` (set in constructor) and include it in the digest signed by relayers, mirroring the fix already applied to `BridgeMultisig.sol` (S6-BM1).
  - **c)** MS1 tally-griefing: change tally key from `seq` alone to `keccak256(seq, to, amount)` so a griefing relayer cannot lock a real withdrawal by tallying a garbage `(to, amount)` first.
  - **d)** Storage-layout migration plan: changing `bridgeOutCount` mapping requires careful slot allocation if upgradeable proxy. Document the slot. If non-upgradeable, plan the pause-deploy-migrate dance.
  - **e)** Independent audit-firm review of the diffs (the bridge holds bridged BNB/BSC funds; do not deploy without a second pair of eyes).
- Acceptance:
  1. `forge test` passes including new tests for: nonce-collision regression, cross-chain replay regression, tally-griefing regression.
  2. Deployment runbook checked into repo with multisig signer order, pause window, and rollback plan.
  3. External audit sign-off attached as a PDF in the repo.
- Why: OUT1 alone can silently lose user funds on collision; MS1 lets a single compromised relayer key permanently brick any user withdrawal; OUT2 enables cross-chain signature replay if the bridge ever extends to a third chain.

### P0-T03  [MAINNET-BLOCKER]  (14–18 days)
**S7-EVM3 — CALL family across both VMs**
- Depends-on: none
- Files: see `docs/proposals/S7-EVM3-call-family-implementation.md` workstreams W1–W11
- Plan: already specified in detail. 11 workstreams: dispatch table, gas, memory, address arithmetic, sub-call frame, return-data buffer, CREATE/CREATE2, DELEGATECALL/STATICCALL semantics, REVERT propagation, SELFDESTRUCT (Cancun-style), tests.
- Acceptance: as per the existing plan-doc.
- Why: Without these opcodes, no real Solidity contract executes. ERC-20 transfers, ERC-721, every DeFi primitive, the bundler, every payable contract — all dead. This is the single largest production-blocking code gap.

### P0-T04  [CRITICAL]  (3–5 days)
**Resolve the duplicate-finality confusion**
- Depends-on: none
- Files: `crates/zbx-finality/`, `crates/zbx-consensus/src/hotstuff.rs`, `crates/zbx-consensus/src/vote.rs`
- Decision needed: HotStuff in `zbx-consensus` already provides QC-based 3-chain finality. The orphan `zbx-finality` crate adds a Casper-FFG-style 2f+1 vote tracker on top. **Pick one:**
  - **Option A (recommended):** Delete `zbx-finality`. Document that finality = HotStuff committed-QC. Update `node/main.rs` doc-comment to remove the misleading "single-slot finality gadget" reference (it does not exist).
  - **Option B:** Wire `zbx-finality` as an explicit checkpoint layer ABOVE HotStuff for additional safety (similar to Ethereum's beacon chain "finalized" vs "justified"). Requires designing checkpoint-vote propagation, signature verification (currently missing), equivocation detection, and conflict resolution. ~15 dev-days.
- Acceptance:
  1. Single source of truth for "what does finalized mean on Zebvix" written into `docs/architecture/finality.md`.
  2. RPC `eth_getBlockByNumber('finalized')` returns the right block consistently.
- Why: Two competing finality definitions in the codebase guarantees confusion for anyone integrating (bridge relayers, exchanges). Pick one, document it.

### P0-T05  [HIGH]  (2 days)
**Decide: keep-and-fix OR delete `zbx-bridge` Rust crate**
- Depends-on: none
- Files: `crates/zbx-bridge/`
- Decision: the production bridge runs on Solidity. `zbx-bridge` Rust crate is orphan AND has 6 conditional CRITICAL/HIGH bugs (S11-BRIDGE-RUST-PROOF1, RELAY1, RELAY2, RELAY3, RELAY4, RELAY5).
  - **Option A (recommended):** Delete the crate. Production has no need. Anyone reading it absorbs the bugs as live code.
  - **Option B:** Fix the 6 bugs and wire it as an in-process relayer alternative to the off-chain JS relayer (avoids one trust boundary). ~10 dev-days.
- Acceptance:
  1. Either crate is gone OR all 6 conditional CRITs are fixed AND it appears in `node/Cargo.toml`.
- Why: Attractive nuisance with documented critical bugs. Cannot leave as-is.

### P0-T06  [DEVNET-BLOCKER]  (Session 14 — code DONE, VPS verify pending)
**S13-CHAIN-ID-DRIFT — chain ID consolidation closure**
- Depends-on: none
- Status: **CODE LANDED in Session 14**; awaiting VPS build/test verification before flipping to CLOSED.
- Files (this task only tracks closure verification — see `docs/proposals/S13-CHAIN-ID-DRIFT-fix.md` and `AUDIT_2026-04-30.md` §S13.2 for the full diff inventory): `crates/zbx-types/src/lib.rs`, `crates/zbx-{tx,config,wallet,bundler,sdk,zvm,vm}/`, `tests/integration/{evm,da_test,zvm_test,bundler_test}.rs`, `tests/unit/payid.rs`, `sdk/{zebvix-js,ethers-zbx}/`, 4× `contracts/*.sol` NatSpec, `monitoring/prometheus.yml`, `k8s/da-node.yaml`, `scripts/{snapshot,testnet-add-validator,da-submit}.sh`, 10× docs, `scripts/check-chain-id.sh` (CI guard).
- Locked values: `CHAIN_ID_MAINNET=8989`, `CHAIN_ID_TESTNET=8990` (devnet shares testnet), `BIP44_COIN_TYPE_ZBX=7878` (SLIP-44, **independent of chain ID**).
- Acceptance:
  1. On build VPS (srv1266996 or equivalent): `bash scripts/ci-check.sh` (full gate, no `--quick`) passes. The script exercises **3 stages**: `cargo check --features $FEATURES`, `cargo clippy --features $FEATURES -- -D warnings -A clippy::all`, and `cargo test --lib --features $FEATURES`. Note: `--lib` covers crate unit tests only; integration tests in `tests/integration/` are NOT run by `ci-check.sh` and require a separate `cargo test --features $FEATURES` invocation on the VPS (which the dev sandbox cannot complete because of the RocksDB linker-OOM).
  2. On build VPS: `bash scripts/check-chain-id.sh` exits 0 (164 allowlisted hits, 0 violations in dev sandbox at S13.2 close).
  3. `target/.last-ci-pass` marker touched by `ci-check.sh`, then a fresh tarball cut and deployed to a clean devnet validator. Devnet `eth_chainId` RPC returns `0x231e` (= 8990).
  4. Cross-replay smoke test: a tx signed for chain 8989 is rejected by a devnet node (and vice-versa) at the EIP-155 verification layer. (Note: this is the PRE-P0-T07 smoke; full devnet ↔ testnet domain-separation is P0-T07's job.)
  5. After VPS verification, flip P0-T06 to CLOSED in this file and append a one-line VPS-verify-stamp to `AUDIT_2026-04-30.md` §S13.2.
- Why: Without consolidation the 5-value drift (7878/7879/7880/8989/8990) silently re-emerges; devnet bring-up cannot proceed and any signed-tx verification path that hashes the wrong chain-id is a consensus footgun.

### P0-T07  [HIGH]  (3–5 days)
**S13-LEGACY-CHAINID-REPLAY — devnet ↔ testnet cross-replay risk (shared chain ID 8990)**
- Depends-on: P0-T06 (consolidation must land first)
- Files: `crates/zbx-tx/src/{legacy,eip1559,eip2930}.rs` (signing-domain salt), `crates/zbx-execution/src/lib.rs` (verification path), new test fixtures in `tests/integration/`.
- Background: by design, devnet and testnet now share `CHAIN_ID = 8990` to avoid introducing a 4th value. Without an additional signing-domain separator, a tx that is valid on one network is bit-for-bit valid on the other. This is acceptable for early devnet but must be closed before public testnet exposure.
- Decision needed (pick one before implementation):
  - **Option A (recommended):** Add a per-network 32-byte `signing_domain_salt` mixed into the EIP-712 / EIP-155 digest at signing AND verification time. Devnet and testnet use distinct salts. Mainnet salt is `0x00…00` for backward compatibility.
  - **Option B:** Allocate a 4th chain ID for devnet (e.g. 8991), reverting one of the deliberate decisions in S13-CHAIN-ID-DRIFT. Reopens the value-sprawl problem this session just closed; not recommended.
- Acceptance:
  1. ZEP authored under `docs/proposals/ZEP-002-CHAINID-DOMAIN-SEPARATION.md` recording the decision and digest format.
  2. Tx signed with devnet salt is **rejected** by testnet `verify_tx_signature` (and vice-versa). New regression test `cross_network_replay_rejected` in `tests/integration/`.
  3. `scripts/check-chain-id.sh` updated if any new constants need allowlisting.
  4. SDKs (`sdk/zebvix-js`, `sdk/ethers-zbx`) updated to set the right salt automatically based on `network: 'mainnet' | 'testnet' | 'devnet'`. Major version bump documented as breaking.
- Why: Public testnet without domain separation means anyone replaying a devnet tx onto testnet (or vice-versa) gets a free, signature-valid duplicate. Even without economic value on devnet today, this is a credibility issue at launch and a real attack path the moment any contract on either network has economic state worth replaying.

---

## 5. Phase 1 — Production wiring (~26–36 dev-days)

**Definition:** Crates that are real implementations of features the chain claims to have, but are not currently wired into `node/zbx-node`. Each task is "wire X into the node service tree."

The Phase 1 standard pattern for each crate:
1. Add to `node/Cargo.toml` deps with `path = "../crates/zbx-X"`.
2. Add a `tokio::spawn` or `select!` arm in `node/src/node.rs` for the service loop, gated by a config flag.
3. Wire the crate's public types into the relevant existing pipeline (mempool / executor / RPC / block-producer).
4. Add a `#[cfg(test)]` integration test for the wire-up under a `tests/integration_X.rs` file.
5. Update `replit.md` and the audit doc to record closure.

### P1A — Critical infrastructure wiring (~18–24 dev-days)

#### P1A-T01  [HIGH]  (3 days)  Wire `zbx-fee` (EIP-1559)
- Depends-on: none
- Files: `node/Cargo.toml`, `node/src/block_producer.rs`, `crates/zbx-mempool/src/lib.rs`, `crates/zbx-rpc/src/eth.rs`
- Acceptance: mempool tx admission uses base-fee + tip, block producer sets `header.base_fee_per_gas`, `eth_gasPrice` and `eth_feeHistory` RPCs return real values from `GasPriceOracle`.
- Why: Without dynamic gas, block congestion either fails-open (DoS) or fails-closed (no txs include).

#### P1A-T02  [HIGH]  (3 days)  Wire `zbx-rewards`
- Depends-on: none
- Files: `node/Cargo.toml`, `node/src/block_producer.rs`, `crates/zbx-execution/src/lib.rs`
- Acceptance: every produced block credits the proposer with `BlockReward::current(block_number)` (respecting halving), fee distribution per `fee_distribution.rs` policy.
- Why: Validators currently get nothing. Mainnet launch with no rewards = no validators = no security.

#### P1A-T03  [HIGH]  (2 days)  Wire `zbx-pruner`
- Depends-on: none
- Files: `node/Cargo.toml`, `node/src/node.rs`, `node/src/config.rs`
- Acceptance: background `tokio::spawn` runs `StatePruner` with config-driven `PruneMode` (Archive / Full / Light), default Full. RocksDB column-family stats logged hourly.
- Why: Without pruning, full nodes' disks fill in weeks of mainnet usage.

#### P1A-T04  [HIGH]  (5 days)  Wire `zbx-sync` + `zbx-snapshot`
- Depends-on: P1A-T03 (pruning interacts with snapshot retention) **AND P1B-T01 (network target must be decided first — sync wires into the chosen networking crate's `sync.rs`)**
- Files: `node/Cargo.toml`, `node/src/node.rs`, `crates/<chosen-network-crate>/src/sync.rs` (set by P1B-T01), `crates/zbx-snapshot/src/lib.rs`
- Acceptance: new node `zbx-node --network mainnet --data-dir /fresh` reaches tip without replaying genesis (snap-sync). Snapshot generated per epoch (~once per N blocks) and served via P2P.
- Why: New validators must be able to join in hours, not days. No snap-sync = no scalable network.
- *Revised post-architect-review: bumped 3→5 days; added explicit P1B-T01 dependency.*

#### P1A-T05  [MEDIUM]  (4 days)  Wire `zbx-oracle`
- Depends-on: none
- Files: `node/Cargo.toml`, `node/src/node.rs`, `contracts/ZbxOracle.sol` (verify exists), `contracts/ZusdPricePeg.sol` (closes S11-PEG-ORACLE), `crates/zbx-evm/src/precompiles.rs` for ZBXPRICE opcode
- Acceptance: oracle reporter nodes push prices (median aggregate) to on-chain `ZbxOracle.sol` every N blocks. ZBXPRICE precompile reads from on-chain oracle. ZUSD peg adjusts based on oracle price (closes S11-PEG-STUB).
- Why: Without oracle: ZUSD peg is blind, ZBXPRICE returns garbage, lending crate has no price feed, S11 PEG findings stay open forever.

#### P1A-T06  [MEDIUM]  (3 days)  Wire `zbx-state-rent` (ZEP-008)
- Depends-on: P1A-T01 (rent is paid in base-fee currency)
- Files: `node/Cargo.toml`, `crates/zbx-execution/src/lib.rs`, `crates/zbx-state/src/account.rs`
- Acceptance: SSTORE charges rent prepayment per ZEP-008. Touched-accounts list captured per block. Hibernation transition tested.
- Why: State growth is unbounded today. By block 1M, full nodes need 100+ GB disk per ZEP-008 design. Either wire rent OR document explicit "no state rent, use archival pricing" decision.

#### P1A-T07  [LOW]  (2 days)  Wire `zbx-indexer`
- Depends-on: none
- Files: `node/Cargo.toml`, `crates/zbx-rpc/src/eth.rs`, `crates/zbx-indexer/src/`
- Acceptance: `eth_getLogs` with large block-range filter returns results in <1 s (currently O(blocks-scanned)). Custom indexer schemas exposed via `zbx_*` namespace.
- Why: Without indexer, RPC `getLogs` walks the entire chain — exchanges and explorers will not work at scale.

### P1B — Triplicate cleanup (~8–12 dev-days)

#### P1B-T01  [HIGH]  (5 days)  Networking consolidation (4 crates → 1)
- Depends-on: none. **Sequencing note: this is a hard prerequisite of P1A-T04 (sync wiring) — schedule P1B-T01 to land BEFORE P1A-T04 begins so the chosen networking crate's `sync.rs` is the integration target. The two are NOT parallelizable across engineers despite both being in Phase 1.**
- Files: `crates/zbx-network/`, `crates/zbx-p2p/`, `crates/zbx-net/`, `crates/zbx-gossip/`, `node/Cargo.toml`
- Decision matrix:
  | Crate | LOC | Has discv5 | Has GossipSub | Has NAT | Wired |
  |-------|----:|:---------:|:------------:|:-------:|:-----:|
  | `zbx-network` (TCP+noise) | 911 | no | no | no | YES |
  | `zbx-p2p` (devp2p/RLPx) | 975 | no | no | no | no |
  | `zbx-net` (full discv5+RLPx+GossipSub+NAT) | 1316 | YES | YES | YES | no |
  | `zbx-gossip` (GossipSub only) | 461 | no | YES | no | no |
- **Recommendation:** Promote `zbx-net` to the wired networking layer (most complete, has the discovery/NAT/gossip features needed for mainnet). Delete `zbx-network`, `zbx-p2p`, `zbx-gossip`.
- Acceptance:
  1. `node/Cargo.toml` references `zbx-net` only.
  2. The other three crates are deleted from `crates/`.
  3. `zbx-net` integration test boots two nodes and confirms peer discovery + tx propagation.
- Why: 3,663 LOC of dead networking code is maintenance debt. Mainnet needs discv5 and NAT traversal which only `zbx-net` has.

#### P1B-T02  [MEDIUM]  (2 days)  RPC consolidation (2 crates → 1)
- Depends-on: none
- Files: `crates/zbx-rpc/`, `crates/zbx-jsonrpc/`
- Decision: `zbx-rpc` (1865 LOC, wired) vs `zbx-jsonrpc` (531 LOC, has WebSocket transport).
- **Recommendation:** Port `zbx-jsonrpc`'s WebSocket transport into `zbx-rpc`. Delete `zbx-jsonrpc`.
- Acceptance:
  1. `eth_subscribe` over WebSocket works against `zbx-rpc`.
  2. `zbx-jsonrpc/` deleted.
- Why: Two RPC crates = double the surface, double the bugs. WebSocket subscriptions are a mainnet must-have for dApp UX.

#### P1B-T03  [LOW]  (1 day)  VM family cleanup (per S7-ARCH1 Option C decision)
- Depends-on: none
- Files: `crates/zbx-vm/`
- Decision: Per S7-ARCH1, user chose **Option C** (keep both `zbx-evm` and `zbx-zvm` with full functionality). The wrapper crate `zbx-vm` (1339 LOC) has unclear role.
- Acceptance: either `zbx-vm` is wired into `zbx-execution` as the dispatch shim, OR it is deleted.
- Why: Three VM-related crates with overlapping types is a recurring confusion source.

---

## 6. Phase 2 — Audit closures (~10–14 dev-days)

**Definition:** Remaining findings from Sessions 1–11 that are not P0/P1 but still need code-level fixes.

### P2-T01  [LOW–MEDIUM]  (2 days)  Session 11 Solidity small fixes
- Files: `contracts/ZbxBridge.sol`, `contracts/BridgeMultisig.sol`
- Sub-tasks:
  - S11-BRIDGE-SOL-PAUSE1 [MED]: add `whenNotPaused` modifier to `bridgeOut` and `bridgeIn`.
  - S11-BRIDGE-SOL-CAP1 [MED]: per-window cap (e.g. `maxBridgeOutPerHour`) on `bridgeOut` and `bridgeIn`.
  - S11-BRIDGE-SOL-MS2 [MED]: add an explicit `vaultSetterDeadline` and revert if `setVault` called after it.
  - S11-BRIDGE-SOL-V155 [LOW]: in `_recoverWithLowS`, normalize `v` from `{0,1}` to `{27,28}` before `ecrecover`.
  - S11-BRIDGE-SOL-MS3 [LOW]: `delete tallies[zebvixSeq]` after successful executeMint.
- Acceptance: forge tests cover each fix. Gas-snapshot diff documented.

### P2-T02  [HIGH]  (2 days)  PEG fixes
- **Depends-on: P1A-T05 (the oracle must be wired before the PEG can read from it; S11-PEG-ORACLE acceptance is gated by it).**
- Files: `contracts/ZusdPricePeg.sol`
- Sub-tasks:
  - S11-PEG-STUB: replace `adjustPeg()` TODO no-op with real implementation that calls `vault.setStabilityFee(...)` based on price deviation. Bound the adjustment magnitude per call.
  - S11-PEG-ORACLE: read price from on-chain oracle (post P1A-T05) with staleness check (revert if `block.timestamp - oracle.lastUpdate > MAX_STALENESS`).
- Acceptance: forge tests for adjust-up, adjust-down, stale-revert.

### P2-T03  [LOW]  (2 days)  zbx-bridge Rust orphan-fix subset (only if P0-T05 chose Option B)
- Files: `crates/zbx-bridge/src/{lib,relayer,multisig,proofs,error}.rs`
- Sub-tasks: fix all 10 Session 11 Rust findings (PROOF1, RELAY1–5, MS1–2 conditional + DEAD1, DEAD2 unconditional).
- Acceptance: `cargo test -p zbx-bridge` passes; `node/Cargo.toml` includes the crate.
- Skip if P0-T05 chose Option A (delete).

### P2-T04  [MEDIUM]  (2 days)  S7-EVM2-TESTS
- Files: `crates/zbx-evm/tests/`
- Sub-tasks: add the 3 architect-prescribed CI integration tests for precompile dispatch (sandbox-blocked but node-team can run).
- Acceptance: `cargo test -p zbx-evm` passes including the 3 new tests.

### P2-T05  [LOW]  (2 days)  Re-verify Sessions 1–10 closures still hold
- Files: walk every closure noted in `AUDIT_2026-04-30.md`
- Acceptance: produce a "Session 12 re-verification" addendum confirming no regressions, OR opening any newly-discovered regression as a fresh finding.

### P2-T06  [LOW]  (2 days)  Audit doc rebase
- Files: `AUDIT_2026-04-30.md`
- Sub-tasks: collapse closed findings into a "CLOSED archive" section, keep only OPEN findings in the main body. Doc has grown to 2,996 lines and is unwieldy.
- Acceptance: doc < 2,000 lines, all OPEN findings clearly visible at top.

### P2-T07  [MEDIUM]  (2 days)  Session 6 lending pool caps (S6-A2)
- *Added post-architect-review: was deferred from Session 6 closure log (`AUDIT_2026-04-30.md:1362-1369`) but missing from initial plan draft.*
- Files: `contracts/ZbxLendingPool.sol`, related forge tests
- Sub-tasks: add `supplyCap` and `borrowCap` per reserve; revert deposits/borrows that would exceed cap; admin-settable via Governable.
- Acceptance: forge tests cover at-cap-revert and after-admin-bump-allowed flows. Gas-snapshot diff documented.

---

## 7. Phase 3 — Feature completion (~16–23 dev-days)

**Definition:** Real features whose code exists but require non-trivial integration work, or product features that are claimed but not yet usable end-to-end.

### P3-T01  [MEDIUM]  (5–7 days)  MEV protection rollout
- Depends-on: P1B-T01 (networking consolidation)
- Files: `crates/zbx-mev/`, `crates/zbx-mempool/src/lib.rs`, `crates/zbx-rpc/src/eth.rs`, `node/src/block_producer.rs`
- Sub-tasks:
  - Layer 1: encrypted mempool — wire `zbx_sendPrivateTransaction` RPC.
  - Layer 2: commit-reveal (deferred — flag as "Phase 4" if too ambitious).
  - Layer 3: PBS — out of scope for v1 mainnet (defer).
  - Layer 4: MEV redistribution — implement basic burn-and-credit per design.
- Acceptance: `zbx_sendPrivateTransaction` works; encrypted txs not visible in `eth_getMempool`.

### P3-T02  [MEDIUM]  (4–5 days)  ERC-4337 bundler service
- Depends-on: P0-T03 (CALL family — bundler needs CALL to execute UserOps)
- Files: `crates/zbx-bundler/`, deployment of `EntryPoint` contract at `0x5FF1...d2789`
- Sub-tasks: stand up bundler as a separate binary `zbx-bundler-svc` listening on its own port. Wire `eth_sendUserOperation` / `eth_estimateUserOperationGas` / `eth_getUserOperationByHash` / `eth_getUserOperationReceipt` / `eth_supportedEntryPoints`.
- Acceptance: a sample UserOp from a smart-account wallet (e.g. Stackup, Alchemy AA SDK) lands on chain.

### P3-T03  [MEDIUM]  (3 days)  PayID registry + opcode wiring
- Depends-on: P0-T03
- Files: `crates/zbx-payid/`, `crates/zbx-evm/src/opcodes.rs` (PAYID, PAYIDSET), `contracts/PayIDRegistry.sol`
- Acceptance: end-to-end: user registers `ali@zbx → 0x742d...`, `eth_call` to PAYID opcode resolves, `eth_call` to PAYIDSET writes (with auth).

### P3-T04  [MEDIUM]  (2–3 days)  Light-client server
- Depends-on: P1A-T04 (sync infra)
- Files: `crates/zbx-light/`, `node/src/node.rs`
- Acceptance: a CLI light-client (`zbx-cli light-sync`) catches up to tip in <30 s and serves SPV proofs for arbitrary tx inclusion.

### P3-T05  [LOW]  (2–3 days)  ZbxAMM flash-callback hook (S6-AMM2)
- *Added post-architect-review: was deferred from Session 6 closure log (`AUDIT_2026-04-30.md:1381-1388`) but missing from initial plan draft. Feature-parity gap with Uniswap V2, NOT a vulnerability.*
- Depends-on: none
- Files: `contracts/ZbxAMM.sol`, `contracts/interfaces/IZbxAMMCallee.sol` (new), forge tests
- Sub-tasks: add Uniswap-V2-style `data.length > 0` callback hook in `swap()` so flash-swap consumers can post-pay. Re-entrancy guard must wrap the callback safely.
- Acceptance: forge test demonstrating a flash-swap that borrows + repays + verifies invariant, plus a re-entrancy attack test that reverts.
- *Note: Build only when an actual flash-swap consumer requires it; can be deferred indefinitely if no demand.*

---

## 8. Phase 4 — Operational hardening (~16–20 dev-days)

**Definition:** Things that make the chain operable, observable, and upgradeable in production. None of these block initial launch but all of them block scaled mainnet operation.

### P4-T01  [HIGH]  (5 days)  CI integration test harness with rocksdb
- **EARLY-GATE: per architect-review, this lands as a P0/P1 PRE-REQUISITE rather than waiting until P4. Recommended schedule: have a minimal CI green by end of P0 week 1 (even if it only runs `cargo check --workspace` initially, expand coverage as P1 lands). Risk register §11 mandates CI on every PR due to sandbox limits — fulfilling that mandate requires CI to exist BEFORE the high-risk P0/P1 work merges.**
- Files: `.github/workflows/ci.yml`, `tests/integration/`, `Cargo.toml` workspace test config
- Acceptance: every PR triggers `cargo test --workspace` against a real rocksdb. Sandbox limitation is irrelevant in CI (Linux runners have generous mmap limits).
- Sequencing: minimal CI (`cargo check --workspace` only) ships in P0 week 1. Full integration test harness expands incrementally with each P1 task. Listed under P4 only because it's an "ops" deliverable; the CALENDAR start is P0.

### P4-T02  [MEDIUM]  (3 days)  Fuzz harness for VMs
- Files: `fuzz/`, `crates/zbx-evm/fuzz_targets/`, `crates/zbx-zvm/fuzz_targets/`
- Acceptance: `cargo fuzz run evm_dispatch` runs cleanly for 1 hour without panic. Same for zvm. Corpus seeded from EthereumStateTests.

### P4-T03  [HIGH]  (3 days)  Hardfork plumbing
- Depends-on: none
- Files: NEW `crates/zbx-hardfork/`, `crates/zbx-execution/src/lib.rs`, `node/src/genesis.rs`
- Sub-tasks: define `enum Hardfork { Genesis, Shanghai, Cancun, ... }`, per-block `hardfork_at(block) -> Hardfork` table, gate opcodes/precompiles by hardfork. Currently chain has no hardfork concept which means upgrades require a hard recompile-everyone fork.
- Acceptance: can ship `zbx-node` v0.3 with new opcode active at block N, while the same binary correctly rejects that opcode for blocks <N.

### P4-T04  [HIGH]  (5 days)  Slashing module + monitor
*Revised post-architect-review: bumped 3→5 days. New crate + equivocation detection + liveness slashing + downtime tracking + slash-tx submission is not a 3-day job; this is a self-contained microservice with HotStuff integration.*
- Depends-on: none
- Files: NEW `crates/zbx-slashing/` (extracted from `zbx-staking`), `node/src/node.rs`
- Sub-tasks: wire double-sign detection (HotStuff equivocation), liveness slashing, downtime tracking, slash-tx submission.
- Acceptance: simulated double-sign by a test validator results in stake reduction within N blocks.

### P4-T05  [MEDIUM]  (2 days)  Monitoring/alerting
- Files: `monitoring/`, `k8s/`, `crates/zbx-metrics/src/lib.rs`
- Sub-tasks: confirm Prometheus exporter exposes block-rate, peer-count, mempool-depth, finalized-lag. Wire Grafana dashboards. Wire alertmanager for: chain-halt, peer-loss, fork-detected.
- Acceptance: dashboards render at staging URL; alert fires when test node killed.

### P4-T06  [MEDIUM]  (2 days)  Genesis tooling
- Files: `crates/zbx-genesis/`, `scripts/`
- Sub-tasks: deterministic genesis builder, validator-set bootstrap, accountancy of pre-mine. Currently `zbx-genesis` is orphan.
- Acceptance: `zbx-cli genesis build --validators=N --balances=...` produces a reproducible `genesis.json` whose hash can be embedded in `node/src/genesis.rs`.

---

## 9. Effort summary

*Rebaselined post-architect-review. Original figures (82–114) were under-estimates. The table below incorporates: P1A-T04 +2 days, P1B-T01 +2 days, P4-T04 +2 days, P2-T07 +2 days (new), P3-T05 +2–3 days (new).*

| Phase | Title | Tasks | Days (low) | Days (high) |
|-------|-------|------:|-----------:|------------:|
| **P0** | Mainnet blockers | 5 | 24 | 32 |
| **P1A** | Critical infra wiring | 7 | 18 | 24 |
| **P1B** | Triplicate cleanup | 3 | 8 | 12 |
| **P2** | Audit closures | 7 | 10 | 14 |
| **P3** | Feature completion | 5 | 16 | 23 |
| **P4** | Operational hardening | 6 | 16 | 20 |
| **Total** | | **33** | **92** | **125** |

**Single engineer:** ~18–25 weeks (~4.5–6.5 months).
**3 engineers in parallel:** ~7–10 weeks calendar time (assuming 70% parallelization efficiency).
**4 engineers in parallel:** ~6–8 weeks calendar time (diminishing returns).

*Calendar add-ons NOT in dev-day count: external bridge audit lead time (4–8 weeks wall-clock for P0-T02 sign-off), testnet bake periods between phases (1–2 weeks each), and security retainer review of P0 changes.*

---

## 10. Sequencing rationale

### Critical path

```
P4-T01 (CI gate, minimal)  ───► (must land week 1, gates everything below)
                                  │
P0-T01 (tx_root)            ───►┐ │
P0-T03 (CALL family)        ───►┤ │
P0-T04 (finality dedup)     ───►├─┼──► P1B-T01 (network) ──► P1A-T04 (sync) ──► other P1 ──► P3 ──► P4 rest ──► MAINNET
P0-T02 (bridge sol + audit) ───►┤ │
P0-T05 (zbx-bridge)         ───►┘ │
                                  │
                                  ├──► P2 closures (mostly independent — except P2-T02 which depends on P1A-T05)
                                  └──► P1B-T02, P1B-T03 (independent of T01, run in parallel)
```

*Revised post-architect-review:*
1. *P4-T01 (CI) hoisted to a pre-P0 gate per §11 risk-register requirement.*
2. *P1B-T01 (networking choice) is now an explicit prerequisite of P1A-T04 (sync wiring) — they cannot run in parallel.*
3. *P2-T02 (PEG) is now an explicit dependent of P1A-T05 (oracle wiring).*

### Parallelization plan (3 engineers, ~7–10 weeks)

| Engineer | Weeks 1–2 | Weeks 3–4 | Weeks 5–7 | Weeks 8–10 |
|----------|-----------|-----------|-----------|------------|
| **A** (Rust core) | P4-T01 minimal CI + P0-T01 | P0-T03 (CALL family) | P0-T03 cont. + P1B-T01 (network) | P1A-T04 (sync; needs P1B-T01) + P1A-T01..T03 + P3-T01 |
| **B** (Solidity) | P0-T02 (kick off external audit) | P0-T02 cont. + P2-T01 + P2-T07 | P2-T02 (gated by P1A-T05) + P3-T02 + P3-T03 + P3-T05 | P4-T04 (slashing monitor) |
| **C** (Infra/ops) | P0-T04 + P0-T05 | P4-T01 expansion + P1A-T05..T07 + P1B-T02 + P1B-T03 | P3-T04 (light-client) + P4-T03 (hardfork) | P4-T02 + P4-T05 + P4-T06 |

*External-audit lead time for P0-T02 (4–8 weeks wall-clock) runs concurrently with engineer-B work; if the audit blocks bridge re-deploy, engineer B pivots to P3 work earlier.*

### Why this sequence

1. **CI gate first.** P4-T01 (minimal: `cargo check --workspace`) lands in P0 week 1 so every subsequent merge is automatically validated. Without this, sandbox-blocked verification in P0/P1 becomes a major regression risk.
2. **P0 next.** No mainnet without these. P0-T03 (CALL family) is the longest single task — start it day 1 of week 2 and parallelize everything else around it.
3. **P1B-T01 before P1A-T04.** Decide the networking target BEFORE wiring sync into it; otherwise you wire sync into a doomed crate and re-do the work.
4. **P1A before P1B-T02/T03.** Wire the real implementations first; then cleanup the remaining duplicates (RPC, VM family) after you know which interface stays.
5. **P2 interleaved (with P1A-T05 dependency).** P2-T02 explicitly waits on P1A-T05 (oracle). Other P2 closures are independent and slot into Solidity engineer's schedule between P0-T02 and P3 features.
6. **P3 after P0-T03.** Bundler and PayID both depend on CALL working.
7. **P4 last (except T01).** Hardfork plumbing and slashing monitor are essential for sustained mainnet but not for genesis launch. P4-T01 (CI) is the exception — it's first.

---

## 11. Risk register

| Risk | Likelihood | Impact | Mitigation |
|------|:---------:|:------:|------------|
| P0-T03 (CALL family) takes longer than 18 days | High | Schedule slip 2–4 weeks | Architect-review the existing plan-doc first; add 1 dedicated engineer. **CREATE2 descope is TESTNET-ONLY** — chain CANNOT launch mainnet with CREATE2 missing because deterministic-address pre-deploy patterns (used by canonical EntryPoint at `0x5FF1...d2789`, factory contracts, deployer bootstrappers) silently break and any consumer expecting EVM-equivalence will fail. If schedule slips, push mainnet date, do not ship without CREATE2. |
| Bridge migration corrupts user funds during P0-T02 deploy | Low | Catastrophic | External audit before deploy; pause-deploy-migrate runbook with multisig dry-run; invariant tests in forge. |
| P1B-T01 networking consolidation breaks peer mesh | Medium | Testnet downtime | Roll out to testnet first; keep `zbx-network` deletable for 1 release as fallback. |
| Real BFT under HotStuff is buggier than realized (currently no equivocation detection wired) | Medium | Liveness or safety incident | P4-T04 slashing must land with mainnet, not after. |
| State-rent (P1A-T06) breaks existing contract assumptions | Medium | Devx complaint storm | Long testnet bake; clear migration warning in release notes; hibernation grace period configurable. |
| Sandbox limitation prevents full pre-merge verification | Certain | Unknown bugs reach node-team | Mandatory CI run on every PR (P4-T01 is high priority). |

---

## 12. Out-of-scope (explicitly deferred beyond this plan)

These are real concerns but not in P0–P4. Document so they are not forgotten:

- **Layer-2 / rollup support** — `zbx-da`, `zbx-prover`, `zbx-zk`, `zbx-verkle`, `zbx-wasm`, `zbx-oracle-zk`, `zbx-oracle-optimistic` exist as crates. Defer to v0.4 / v1.0 plan.
- **AI precompile** — `zbx-ai-precompile` (315 LOC, 9 stubs) — speculative feature, defer.
- **DEX/lending** — `zbx-launchpad`, `zbx-lending`, `zbx-nft` — application crates, not chain-core; defer.
- **Sequencer crate** — `zbx-sequencer` (839 LOC) likely for L2 mode. Defer.
- **Threshold signatures admin** — `zbx-threshold` (730 LOC) — already used in bridge; standalone usage TBD.
- **Light light-client** — `zbx-light` is server-side; CLI-side light-client not built.

---

## 13. Acceptance for this plan-doc

This document is considered "good enough" when:

1. Node-team lead reads it once and understands what to do for the next 6 weeks without further questions.
2. Founder/PM can use the effort table for resourcing decisions.
3. Auditor (next session) can trace every Session 1–11 finding to a P0/P1/P2 task in this plan.
4. Each P-task can be picked up as an independent GitHub issue with this doc as the only context.

---

## 14. References

- `AUDIT_2026-04-30.md` — rolling audit (Sessions 1–11, 2,996 lines)
- `docs/proposals/S7-ARCH1-vm-consolidation.md` — VM Option C decision
- `docs/proposals/S7-EVM3-call-family-implementation.md` — CALL family detailed plan (W1–W11)
- `docs/proposals/ZEP-001-PAYID.md` — PayID design
- `docs/proposals/ZEP-002-ZUSD.md` — ZUSD stablecoin design
- `docs/proposals/ZEP-003-DA-LAYER.md` — Data availability design
- `docs/proposals/ZEP-004-ZVM.md` — ZVM design
- `replit.md` — project memory and conventions

---

**End of PHASE-PLAN-2026-05-01.**
