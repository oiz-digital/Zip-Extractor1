# Security Policy — Zebvix Chain (ZBX)

## Supported Versions

| Version | Supported |
|---|---|
| `main` (devnet) | ✅ Active |
| Testnet (8990) | ✅ Active |
| Mainnet (8989) | ⏳ Pre-launch |

---

## Reporting a Vulnerability

**Do not open public GitHub issues for security vulnerabilities.**

Report to: **security@zebvix.io**

Include:
- Affected component(s) and crate(s)
- Reproduction steps
- Potential impact
- Suggested fix (optional)

Response within 48 hours. We will coordinate a fix + disclosure timeline before any public release.

---

## Current Security Status (2026-06-27)

Verified by direct `.rs` source code reads. Full details: [AUDIT_2026-06-27.md](./AUDIT_2026-06-27.md)

### ✅ Fixed (code-verified)

| ID | Finding | Component |
|---|---|---|
| C53-01 | BLS sign stub returned zero bytes | `zbx-consensus/src/bls/signing.rs` |
| S7-CR4 | VRF verify rubber-stamped any proof | `zbx-crypto/src/vrf.rs` (now explicit Err) |
| MB-1p | BLOCKHASH returned all zeros | `zbx-evm/src/interpreter.rs` |
| MB-2p | PREVRANDAO used block_number × constant | `zbx-evm/src/interpreter.rs:589` |
| MB-3p | ORIGIN returned msg.sender not tx.origin | `zbx-evm/src/interpreter.rs:431` |
| MB-4p | GASPRICE returned base fee only | `zbx-evm/src/interpreter.rs:491` |
| MB-5p | RIPEMD-160 precompile used keccak256 | `zbx-evm/src/precompiles.rs:342` |
| MB-6p | Groth16 prover returned zero bytes | `zbx-prover/src/prover.rs` |
| MB-7p | Sequencer `mock_execute()` returned `[0xAA; 32]` | `zbx-sequencer/src/sealer.rs` |
| MB-8p | Governance RPC returned null/error | `zbx-rpc/src/zbx_api.rs` |
| ZBX-C-05 | Zero BLS vote propagation | `zbx-consensus/src/hotstuff2.rs:551` |
| S7-CR5 | VRF score used non-deterministic f64.powf | `zbx-crypto/src/vrf.rs:73` |

### ❌ Open Blockers

| ID | Finding | File | Severity |
|---|---|---|---|
| MB-1 | KZG τ=1 placeholder — proofs forgeable | `zbx-da/src/commitment.rs` | CRITICAL (panics on mainnet) |
| MB-2 | `blob_to_kzg_commitment` SHA-256 not G1 MSM | `zbx-da/src/commitment.rs:410` | HIGH |
| MB-3 | AI precompile stub weights — not trained | `zbx-ai-precompile/src/model.rs:168` | HIGH |
| MB-4 | Consensus `vrf_verify()` always Err | `zbx-crypto/src/vrf.rs:50` | HIGH |
| MB-5 | Whistleblower bonds in-memory | `zbx-staking/src/pipeline.rs` | MEDIUM |
| MB-6 | `build_tc` zero BLS fallback | `zbx-consensus/src/hotstuff2.rs:315` | MEDIUM |

---

## Security Architecture

### Cryptographic Primitives

| Primitive | Usage | Crate |
|---|---|---|
| BLS12-381 | Consensus aggregation + Verkle | `blst` |
| Ed25519 | Transaction signing | `ed25519-dalek` |
| secp256k1 | EVM-compatible signing | `k256` |
| Keccak-256 | Address derivation + state trie | `sha3` |
| Dilithium-3 | Post-quantum signing (FIPS 204) | `pqcrypto-dilithium` |
| Kyber-768 | Post-quantum KEM (FIPS 203) | `pqcrypto-kyber` |
| Groth16/Bn254 | ZK proofs (state, bridge, payment) | `arkworks` |
| RFC 9381 ECVRF | VRF block proposer selection | `curve25519-dalek` |

### Network Security

- **P2P transport:** Noise XX (25519 + ChaChaPoly + SHA256)
- **mDNS discovery:** off by default (opt-in only)
- **RPC limits:** 256 inflight requests, 256 KiB body, 600 RPM (mainnet)
- **Mempool:** fee floor enforced (`MIN_TX_FEE_WEI`)

### Consensus Security

- BLS Proof-of-Possession at validator registration
- 2/3+ quorum for block commit (BFT safety)
- 24-hour evidence replay window (17,280 blocks)
- Chain ID enforced in tx signing (EIP-155)

---

## Audit History

| Date | Sessions | Findings |
|---|---|---|
| 2026-04-30 | 1–12 | 21 CRITICAL crypto stubs closed |
| 2026-05-09 | 13–19 | All HIGH precompile + BLS stubs closed |
| 2026-06-27 | Code re-audit | 6 mainnet blockers; documents corrected |

Full report: [AUDIT_2026-06-27.md](./AUDIT_2026-06-27.md)
