# ZBX Chain — Security Audit Summary

**Last updated:** 2026-06-27 (code audit via direct `.rs` file reads)  
**External audit:** Pending — targeting Q3 2026  
**Full consolidated audit:** [`AUDIT_2026-06-27.md`](../AUDIT_2026-06-27.md)

---

## Scope

### Rust crates (critical path — all audited in code)

| Crate | Audit Method | Status |
|---|---|---|
| `zbx-consensus` | Direct `.rs` read | ✅ HotStuff-2 real BLS; 1 medium open (zero TC fallback) |
| `zbx-evm` | Direct `.rs` read | ✅ All opcodes + precompiles verified real |
| `zbx-crypto` | Direct `.rs` read | ✅ BLS12-381, Ed25519, RFC 9381 ECVRF real |
| `zbx-da` | Direct `.rs` read | ⚠️ Pairing real; commitment MSM + ceremony missing |
| `zbx-staking` | Direct `.rs` read | ⚠️ Core real; bonds in-memory; unbonding TODO |
| `zbx-oracle` | Direct `.rs` read | ✅ Real HTTP to 7 CEXs + VWAP |
| `zbx-threshold` | Direct `.rs` read | ✅ Real Feldman VSS + FROST |
| `zbx-prover` | Grep-verified | ✅ Real Groth16/Bn254; PLONK fail-closed by design |
| `zbx-ai-precompile` | Direct `.rs` read | ❌ Stub model weights (12 models) |
| `zbx-bridge` | Partially verified | ⚠️ Rust wired; Solidity needs separate audit |
| `zbx-rpc` | Direct `.rs` read | ✅ Governance RocksDB-backed |
| `zbx-light` | Grep-verified | ⚠️ Genesis checkpoint deprecated/all-zero |
| `zbx-vm` | Direct `.rs` read | ✅ Precompiles 0x01–0x0a fully implemented (0x05–0x0a fixed 2026-06-27, 19 tests) |

### Solidity contracts (separate audit required)

- `ZbxBridge.sol` — **known issue:** BSC nonce collision (S11-BRIDGE-SOL-OUT1)
- `BridgeMultisig.sol` — **known issue:** single-relayer key griefing
- `ZbxStaking.sol`, `ZbxGovernor.sol`, `ZUSD.sol`, `ZusdVault.sol` — not yet audited
- `ZbxEntryPoint.sol` (ERC-4337), `ZbxPaymaster.sol`, `ZbxSmartWallet.sol` — not yet audited

---

## Findings Summary

### CRITICAL (all closed)

| ID | Description | Fixed in | Code Evidence |
|---|---|---|---|
| C-01 | BLS `sign_block` returned zero bytes — any attacker could forge blocks | Session 12 | `bls/signing.rs:3` comment confirms fix |
| C-02 | `vrf_verify` rubber-stamped any proof | Audit-2026-05-01 S7-CR4 | `vrf.rs:43-48` audit comment |
| C-03 | Groth16 prover returned 12 zero bytes | Session 16 | `prover.rs` — real `Groth16::<Bn254>::prove` |
| C-04 | KZG verifier returned `false` unconditionally | Session 43 | `commitment.rs:7-10` comment confirms fix |
| C-05 | Zero BLS votes could propagate | ZBX-C-05 | `hotstuff2.rs:551` comment confirms fix |

### HIGH — Open Mainnet Blockers

| ID | Description | File | Status |
|---|---|---|---|
| MB-1 | KZG τ=1 — ceremony file missing | `zbx-da/src/commitment.rs` | ❌ OPEN — requires external Powers of Tau ceremony |
| MB-2 | `blob_to_kzg_commitment` G1 MSM | `zbx-da/src/commitment.rs:410` | ✅ FIXED (2026-06-27) — real BLS12-381 G1 MSM |
| MB-3 | AI precompile stub weights | `zbx-ai-precompile/src/model.rs:168` | ❌ OPEN — all 12 models use `stub_network()` |
| MB-4 | `vrf_verify()` always returns Err | `zbx-crypto/src/vrf.rs:50` | ✅ FIXED (2026-06-27) — secp256k1 ECDSA-backed, 7 tests |

### MEDIUM — Fixed / Open

| ID | Description | File | Status |
|---|---|---|---|
| MB-5 | Whistleblower bonds in-memory | `zbx-staking/src/pipeline.rs` | ✅ FIXED (2026-06-27) — RocksDB-backed via SlashingBonds CF |
| MB-6 | `build_tc` zero BLS fallback | `zbx-consensus/src/hotstuff2.rs:315` | ✅ FIXED (2026-06-27) — returns `Option`, no zero TC |
| M-1 | Staking unbonding chunk tracking | `zbx-contracts/src/staking_escrow.rs:222` | ⚠️ OPEN — before mainnet |
| M-2 | Light client genesis checkpoint all-zero | `zbx-light/src/header_chain.rs:64` | ⚠️ OPEN — before mainnet |
| M-3 | zbx-vm precompiles 0x05–0x0a stubbed | `zbx-vm/src/precompiles.rs` | ✅ FIXED (2026-06-27) — 6 precompiles + 19 tests added |

### LOW — Technical Debt

| ID | Description | File |
|---|---|---|
| L-1 | `zbx-executor/src/batch.rs` simplified stub | `zbx-executor/src/batch.rs:108` |
| L-2 | XCL state not initialized at genesis | `zbx-rpc/src/zbx_api.rs:379` (documented) |
| L-3 | zbx-bundler chain_id placeholder | `zbx-bundler/src/service.rs:66` |

---

## What the Audit CONFIRMED is Real (Previously Misreported)

Several prior documents incorrectly marked these as stubs. Direct code reading confirms all are real:

| Component | What Documents Said | What Code Shows |
|---|---|---|
| Oracle price feeds | "hardcoded stubs" | Real HTTP to Binance/Coinbase/Kraken/Gate/Bybit/KuCoin/CoinGecko + VWAP |
| DKG key shares | "uses `new_stub()`" | Real Feldman VSS with k256 polynomial, `from_dkg_parts()` |
| EVM PREVRANDAO | "returns block_number × constant" | `self.ctx.randao_mix` from consensus layer |
| EVM ORIGIN | "returns msg.sender" | `self.ctx.tx_origin` (real EOA origin) |
| EVM GASPRICE | "returns base fee only" | `effective_gas_price = base + priority_tip` |
| VRF precompile 0x0E | "always returns None" | Real RFC 9381 ECVRF-EDWARDS25519-SHA512 |
| Governance RPC | "returns null" | RocksDB write + in-memory cache + DB fallback |
| 21+ crates "orphaned" | "unwired" | All imported and spawned in `node/src/node.rs` |

---

## Test Coverage Gaps

Integration tests that pass CI but test nothing (assert!(true)):

| Test File | Stub Count | What Is Not Tested |
|---|---|---|
| `tests/integration/staking_test.rs` | 5 | Stake, epoch rewards, slashing, unbonding |
| `tests/integration/sync_test.rs` | 4 | Fast sync, snap sync, chain healing |
| `tests/integration/bridge_test.rs` | 3 | Lock-mint, burn-mint, replay protection |
| `tests/integration/prover_test.rs` | multiple | State proofs, fraud proofs |

**Recommendation:** Replace `assert!(true)` stubs with real scenario tests before external audit.

---

## External Audit Status

**Planned:** Q3 2026 (post-mainnet-blocker resolution)  
**Scope for external audit:**
1. Full Rust crates audit (focus: consensus safety, crypto correctness)
2. Solidity contracts full audit (bridge, staking, governance, AA)
3. ZK circuit audit (Groth16 circuit constraints for state proofs)
4. Post-quantum integration audit (Dilithium-3 + Kyber-768 usage patterns)
