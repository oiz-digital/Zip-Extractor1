# ZBX Chain — Mainnet Launch Checklist

**Go/no-go criteria for ZBX Chain (chain ID 8989) mainnet launch.**  
**Last updated:** 2026-06-29 (re-verified against source code — all prior fixes confirmed)  
**Full testnet audit:** [`docs/TESTNET_AUDIT_2026-06-29.md`](TESTNET_AUDIT_2026-06-29.md)  
**Code gaps:** [`docs/CODE_GAPS.md`](CODE_GAPS.md)

**Blocker progress (code-verified 2026-06-29):**  
✅ MB-2 FIXED — blob_to_kzg_commitment real G1 MSM (g1_srs Vec<G1Affine> added)  
✅ MB-4 FIXED — consensus VRF verify (secp256k1 ECDSA-backed, 7 tests)  
✅ MB-5 FIXED — whistleblower bonds now RocksDB-backed (SlashingBonds CF)  
✅ MB-6 FIXED — build_tc returns Option, no zeroed TC can propagate  
✅ zbx-vm precompiles FIXED — 0x05–0x0a (modexp, bn128_add/mul/pairing, blake2f, kzg_point_eval) implemented with 19 tests  
✅ XCL NOT_INITIALIZED FIXED — genesis defaults registered in zbx_api.rs  
✅ Partial undelegate FIXED — UnbondingChunk pushed in staking_escrow.rs  
❌ MB-1 OPEN — KZG Powers of Tau ceremony file (external, cannot code-fix)  
❌ MB-3 OPEN — AI precompile stub weights (12 models, needs real training)  
❌ M-3 OPEN — External Solidity security audit (3rd-party firm required)

---

## Critical Blockers — Must Resolve Before Mainnet

### ❌ MB-1: KZG Trusted Setup Ceremony File

**File:** `zbx-da/src/commitment.rs:113-146`

Without `/etc/zbx/kzg_g2_tau.bin` (or `ZBX_KZG_G2_TAU_PATH`), the node panics on `ZBX_CHAIN_ENV=mainnet`. This guard is correct — but the ceremony file itself must be generated externally.

**What to do:**
- Run an EIP-4844 compatible Powers of Tau ceremony
- Output: 96-byte compressed G₂[τ] point in `kzg_g2_tau.bin`
- Alternatively use the Ethereum KZG ceremony output (if blob field sizes match)

**Verification:** `zbx-node --network mainnet` boots without panic

**Status:** ❌ OPEN

---

### ✅ MB-2: blob_to_kzg_commitment Real G1 MSM — FIXED (2026-06-27)

**File:** `zbx-da/src/commitment.rs`

`blob_to_kzg_commitment` now performs `C = Σᵢ aᵢ · g1_srs[i]` (real BLS12-381 G1 MSM). `KzgSettings` extended with `g1_srs: Vec<G1Affine>` (4096 G1 points). Devnet placeholder (τ=1): all 4096 entries = `G1Affine::generator()` — commitments are mathematically valid and consistent with the τ=1 verify path. `load_from_ceremony_json` now also parses `g1_monomial` (or `g1_lagrange`) to load real ceremony G1 points. `with_g1_srs()` builder added for programmatic setup.

**Tests (4 new):**
- `kzg_commitment_zero_blob_is_identity` — all-zero blob → G1 identity ✅
- `kzg_commitment_constant_poly_with_dev_srs` — blob=[42,0,…] → 42·G₁ ✅
- `kzg_commitment_wrong_size_returns_identity` — bad-size blob → identity ✅
- `kzg_self_consistency_dev_setup` — real commit+proof+verify roundtrip ✅

**Verification:** `cargo test -p zbx-da` — all pass

**Status:** ✅ FIXED

---

### ❌ MB-3: AI Precompile Stub Model Weights

**Files:** `zbx-ai-precompile/src/model.rs:168`, `engine.rs:132`

All 12 AI inference models use `ModelRegistry::with_stubs()` — weights deterministically derived from `model_id` byte, not real trained models.

```rust
pub fn with_stubs() -> Self {
    for id in 0..12u8 { r.register(ModelMeta::stub(id)); }
}
```

**What to do:**
- Train or source real model weights
- Load from DA layer blob commitments (per ZEP-009 design)
- Or gate AIINFER opcode behind a feature flag that panics on mainnet

**Verification:** `AIINFER` returns meaningful, non-stub inference output for known inputs

**Status:** ❌ OPEN

---

### ✅ MB-4: Consensus VRF Verify — FIXED (2026-06-27)

**File:** `zbx-crypto/src/vrf.rs`

`vrf_prove` now computes `output = keccak256(privkey ‖ input)` and signs `keccak256("zbx-vrf-v1\x00" ‖ input ‖ output)` with secp256k1 ECDSA, storing r‖s (64 bytes) in `VrfProof.proof` — no struct change needed. `vrf_verify` recomputes the signed hash, tries both recovery ids (0 and 1), and accepts iff the recovered public key matches the expected `pub_key_bytes`. Unforgeable without the private key.

**Tests:** `cargo test -p zbx-crypto consensus_vrf_tests` — 7 tests:
prove→verify roundtrip ✅ | wrong pubkey rejected ✅ | wrong input rejected ✅ | tampered output rejected ✅ | tampered proof bytes rejected ✅ | output determinism ✅ | distinct inputs → distinct outputs ✅

**Status:** ✅ FIXED

---

### ✅ MB-5: Whistleblower Bond Escrow — FIXED (2026-06-27)

**Files:** `zbx-staking/src/slashing_v2.rs`, `zbx-staking/src/persistence.rs`

`SlashingRegistryV2` no longer has a `pending_bonds` field. Bond state is fully durable via `EvidenceStore::put_bond` / `get_bond` / `delete_bond` / `list_bonds_for_record`, backed by the `SlashingBonds` RocksDB column family. Keys are `(record_id ‖ reporter)`; values are `bincode`-serialized `BondEntry` structs. Module doc in `pipeline.rs` updated.

**Verification:** Post bond → restart node → `EvidenceStore::get_bond(record_id, reporter)` returns `Ok(Some(entry))`

**Status:** ✅ FIXED

---

### ✅ MB-6: BLS Zero Signature Fallback in build_tc — FIXED (2026-06-27)

**File:** `zbx-consensus/src/hotstuff2.rs`

`build_tc` now returns `Option<TimeoutCertificate>`. On BLS aggregation failure it logs `tracing::error!` and returns `None` via `.map_err(...).ok()?` — a zeroed `BlsSignature([0u8; 96])` is never constructed or propagated. `add_share` passes the result directly to its caller. `on_timeout_share` already used `if let Some(tc) = self.tc_accum.add_share(...)` — no call-site changes were needed.

**Verification:** BLS agg failure in `build_tc` → `None` returned → `on_timeout_share` returns `Ok(vec![])` → view-change waits for fresh shares

**Status:** ✅ FIXED

---

## ✅ Passed Go/No-Go Items

### ✅ 1. BLS Proof-of-Possession at Validator Registration

`zbx-staking::ValidatorSet::register_with_pop` calls `BlsPubKey::verify_pop(pop, address)` and rejects invalid PoP with `StakingError::InvalidEvidence`.

**Code:** `zbx-staking/src/validator.rs:153-173`  
**Test:** `cargo test -p zbx-node readiness::tests::bls_pop_check_rejects_zero_pop`

---

### ✅ 2. All 9 Standard EVM Precompiles Implemented

Real implementations confirmed in `zbx-evm/src/precompiles.rs`:
- `0x01` ECRECOVER, `0x02` SHA-256, `0x03` RIPEMD-160 (real, not keccak256)
- `0x04` IDENTITY, `0x05` MODEXP (EIP-2565 gas), `0x06-0x08` BN128 ops
- `0x09` BLAKE2F

---

### ✅ 3. Custom Precompiles 0x0A–0x0F Implemented

- `0x0A` PayID, `0x0B` KZG verify, `0x0C` Price oracle
- `0x0D` Ed25519, `0x0E` RFC 9381 ECVRF, `0x0F` ZUSD vault

---

### ✅ 4. EVM Opcodes ORIGIN / GASPRICE / PREVRANDAO Correct

- `0x32 ORIGIN` → `self.ctx.tx_origin` (real tx signer, not msg.sender)
- `0x3a GASPRICE` → `self.ctx.gas_price` (effective = base + priority tip)
- `0x44 PREVRANDAO` → `self.ctx.randao_mix` (from consensus randao mix)

---

### ✅ 5. Snapshot Manifest Cryptographically Bound

`SnapshotManifest` BLS-signed with chain_id domain separation. Stale-checkpoint rejection and import-boundary enforcement wired.

**Code:** `zbx-state/src/snapshot.rs`

---

### ✅ 6. Trie Pruner Wired into Node Startup

`zbx-storage::pruner::prune_once` runs bounded mark-and-sweep every `storage.pruner.interval_secs` (default 300s) as supervised auto-restart task.

**Code:** `node/src/node.rs:416`

---

### ✅ 7. KZG Trusted Setup Panics on Mainnet Without File

`zbx-da/src/commitment.rs:113-146` panics when `ZBX_CHAIN_ENV=mainnet` and no ceremony file found. This is a correct defense; the file itself is MB-1.

---

### ✅ 8. Governance RPC RocksDB-Backed

`zbx_proposeGovernance` writes to RocksDB. `zbx_getGovernanceProposal` reads from in-memory cache + DB fallback. Rehydrated on startup.

---

### ✅ 9. Oracle Price Feeds Real HTTP

`zbx-oracle/src/fetcher.rs` makes real HTTP calls to Binance, Coinbase, Kraken, Gate, Bybit, KuCoin, CoinGecko with VWAP aggregation.

---

### ✅ 10. Groth16 ZK Prover Real

`zbx-prover/src/prover.rs` uses real `arkworks Groth16::<Bn254>::prove`. Not stub.

---

## Medium Issues (Fix Before Mainnet, Non-Blocking for Testnet)

| ID | Issue | File | Notes |
|---|---|---|---|
| M-1 | Unbonding chunk tracking TODO | `zbx-contracts/src/staking_escrow.rs:222` | Delegator partial withdrawal may break |
| M-2 | Light client genesis checkpoint all-zero | `zbx-light/src/header_chain.rs:64` | Warn-only; fix before production light clients |
| M-3 | Integration tests all `assert!(true)` | `tests/integration/*` | ~20 stub assertions; CI green but nothing tested |
| M-4 | Bridge Solidity nonce collision | `contracts/ZbxBridge.sol` | Needs separate Solidity audit |
| M-5 | XCL state not initialized at genesis | `zbx-rpc/src/zbx_api.rs:379` | Returns empty list (documented, not a bug) |

---

## Operational Checklist (Infrastructure)

Before mainnet launch, also verify:

- [ ] Ceremony file at `ZBX_KZG_G2_TAU_PATH` on all validator nodes
- [ ] Mainnet genesis JSON signed by ≥3/5 validators
- [ ] All validators running BLS PoP registration
- [ ] Prometheus + Grafana dashboards live
- [ ] CORS restricted to production domains
- [ ] Rate limiting configured (600 RPM)
- [ ] DDoS protection in front of public RPC
- [ ] Validator keys in HSM or encrypted store (not plaintext disk)
- [ ] External security audit completed
- [ ] Bug bounty program active

---

*Updated: 2026-06-27 | Source: direct `.rs` code audit*
