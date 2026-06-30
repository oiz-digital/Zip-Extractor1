# Zebvix Chain — Testnet → Mainnet Build Guide

**Date:** 2026-06-29  
**Author:** Code-verified audit (all file paths and guards confirmed from source)  
**Audience:** Core team / chain operators who have successfully run testnet and want to produce the first mainnet binary

---

## The Core Principle

> **Testnet aur Mainnet ka code 100% same hai — sirf 4 compile-time constants + 3 runtime files + 2 genesis addresses different hain.**

Mainnet binary banaana ek clean, ordered sequence hai. Is guide mein har step ke saath exact file path aur code reference diya gaya hai jahan se woh guard aaya hai.

---

## Code-Verified Mainnet Blockers (5 Hard Panics + 2 Soft Errors)

Ye sab directly code se verified hain — agar ye nahi kiye toh node boot nahi hoga ya silently broken hoga.

| # | Guard | File | Type | Testnet bypass | Mainnet behaviour |
|---|---|---|---|---|---|
| **M-1** | Genesis hash sentinel | `crates/zbx-types/src/pinned_genesis.rs:57` | Hard | Allowed (SENTINEL_HASH) | `PinError::Sentinel` — node refuses to start |
| **M-2** | KZG ceremony file | `crates/zbx-da/src/commitment.rs:173` | Hard panic | `ZBX_KZG_ALLOW_DEVNET_TAU=1` | `panic!("SECURITY: KZG τ·G2 ceremony point not found")` |
| **M-3** | AI model weights | `crates/zbx-ai-precompile/src/weights.rs:147` | Hard panic | `ZBX_AI_ALLOW_STUBS=1` | `panic!("SECURITY: model weight file missing/invalid on mainnet")` |
| **M-4** | Placeholder addresses | `node/src/genesis.rs:239` | Hard error | Skip (chain_id≠8989) | `"OPERATOR-05: mainnet genesis validator has placeholder address"` |
| **M-5** | PLONK SRS sentinel | `crates/zbx-zk/src/plonk.rs:518` | Soft error | Sentinel allowed | `Err(VerifyError::PlonkSrsNotInitialized)` — all ZK proofs fail |
| **M-6** | FeeRegistry zero address | `crates/zbx-pool/src/registry.rs:169` | Soft error | Allowed (fails loudly) | All governance/fee ops rejected — DEX non-functional |
| **M-7** | External Solidity audit | `contracts/` (133 files) | Process | Internal tests only | 3rd-party audit required before mainnet |

---

## Step-by-Step Mainnet Build Sequence

### Prerequisites — Testnet pe ye confirm karo pehle

```bash
# Testnet successfully running at chain_id 8990
curl -s http://localhost:18545 \
  -d '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}'
# Expected: "result":"0x231e"

# Minimum 30 days stable testnet operation recommended
# All validators consistently proposing blocks
# Zero equivocation alerts
curl -s http://localhost:9101/metrics | grep zbx_equivocation_alerts_total
# Expected: 0
```

---

## STEP 1 — KZG Ceremony File (M-2)

**Code guard:** `crates/zbx-da/src/commitment.rs:154-186`

```rust
// Exact panic in the code:
if is_production && !allow_devnet {
    panic!(
        "SECURITY: KZG τ·G2 ceremony point not found at '{}' \
         and ZBX_CHAIN_ENV={chain_env:?}. \
         Booting mainnet with τ=1 allows KZG proof forgery.",
        path,
    );
}
```

**Kya karna hai:**

Ethereum ke EIP-4844 ceremony ka file use karo — ZBX chain EIP-4844 compatible hai.
Do NOT generate your own τ — the ceremony must be public multi-party.

```bash
# Option A (Recommended): Ethereum EIP-4844 ceremony adopt karo
# File: 96-byte compressed BLS12-381 G₂[τ] point

# Official Ethereum ceremony se download karo:
wget https://raw.githubusercontent.com/ethereum/c-kzg-4844/main/src/trusted_setup.txt

# ZBX tool se convert karo:
cargo run --release -p zbx-da -- \
    kzg-import-ceremony \
    --input trusted_setup.txt \
    --output /etc/zbx/kzg_g2_tau.bin

# Verify: file exactly 96 bytes honi chahiye
ls -la /etc/zbx/kzg_g2_tau.bin
# Expected: 96 bytes

# Path override karna ho toh (optional):
export ZBX_KZG_G2_TAU_PATH=/your/custom/path/kzg_g2_tau.bin
```

**Option B:** `KzgSettings::load_from_ceremony_json()` se directly JSON import karo:

```bash
# trusted_setup.json (Ethereum format) se:
cargo run --release -p zbx-da -- \
    kzg-import-json \
    --input /path/to/trusted_setup.json \
    --output /etc/zbx/kzg_g2_tau.bin
```

**Verify:**

```bash
# Node should log this on startup:
# "KZG: loaded real τ·G2 ceremony point from /etc/zbx/kzg_g2_tau.bin"
# NOT: "KZG: τ·G2 ceremony point not found — using DEVNET placeholder"
```

---

## STEP 2 — AI Model Weights (M-3)

**Code guard:** `crates/zbx-ai-precompile/src/weights.rs:129-151`

```rust
// Exact panic in the code:
if !allow_stubs {
    panic!(
        "SECURITY: model '{}' weight file missing/invalid on mainnet: {}. \
         Place the .zbxw file in {:?} or set ZBX_MODEL_DIR.",
        meta.name, e, self.model_dir
    );
}
```

**Format spec** (from `weights.rs:1-30`):

```
File: /etc/zbx/models/<model_name>.zbxw

Layout (little-endian):
  [magic:      4 bytes]  = 0x5A425857 ("ZBXW")
  [version:    1 byte]   = 0x01
  [model_id:   1 byte]   = 0x00–0x0B (models 0–11)
  [in_size:    2 bytes LE u16]
  [hidden:     2 bytes LE u16]
  [out_size:   2 bytes LE u16]
  [da_hash:   32 bytes]  = SHA3-256(layer1_weights || layer2_weights)
  [layer1_weights: in_size * hidden bytes i8, row-major]
  [layer1_biases:  hidden * 4 bytes i32 LE]
  [layer2_weights: hidden * out_size bytes i8, row-major]
  [layer2_biases:  out_size * 4 bytes i32 LE]
```

**Kya karna hai:**

```bash
# 12 models train karo (ZEP-009 spec ke according)
# Training pipeline: zbx-model-export CLI (separate training repo)

# Ek baar trained and exported:
mkdir -p /etc/zbx/models/
cp model_0_txclass.zbxw    /etc/zbx/models/
cp model_1_anomaly.zbxw    /etc/zbx/models/
cp model_2_spam.zbxw       /etc/zbx/models/
# ... sab 12 models

# Custom dir use karna ho:
export ZBX_MODEL_DIR=/your/path/to/models

# Verify: sab 12 files present hain
ls /etc/zbx/models/*.zbxw | wc -l
# Expected: 12

# Har model ka SHA3-256 hash DA layer pe publish karo
# (ZEP-009 requires on-chain hash commitment)
zbx-node da-publish-model-hashes \
    --model-dir /etc/zbx/models/ \
    --rpc-url http://localhost:8545
```

---

## STEP 3 — Real Validator Keys Generate Karo (M-4)

**Code guard:** `node/src/genesis.rs:239-277`

```rust
// Sirf chain_id == 8989 pe enforce hota hai:
pub fn validate_no_placeholders(&self) -> Result<(), String> {
    if self.chain_id != zbx_types::CHAIN_ID { return Ok(()); }  // testnet skip
    
    let is_placeholder = |bytes: &[u8; 20]| {
        bytes.iter().all(|&b| b == 0)          // zero address
        || bytes[..18].iter().all(|&b| b == 0) // sequential stub (0x000...2001)
    };
    // Hard error if any validator or alloc address is placeholder
}
```

**Testnet keys KABHI USE MAT KARO** — testnet BLS keys logs/CI mein visible hote hain.

```bash
# Air-gapped machine pe chalao (internet se disconnect):

# Step 3a: New validator key generate karo
cargo run --release -p zbx-keygen -- generate \
    --output /secure/mainnet-validator.json

# Output format:
# {
#   "evm_address": "0xABC...full entropy 20 bytes...",
#   "bls_pubkey":  "0x...",
#   "bls_privkey": "ENCRYPTED — stored in HSM"
# }

# Step 3b: Node identity key
cargo run --release -p zbx-keygen -- node-key \
    --output /secure/mainnet-node.key

# Step 3c: HSM mein wrap karo (Ledger / YubiHSM)
# BLS private key kabhi raw VPS pe mat rakhho
zbx-node keystore wrap \
    --input /secure/mainnet-validator.json \
    --hsm ledger \
    --output /etc/zbx/secrets/validator-wrapped.json
```

---

## STEP 4 — Mainnet Genesis File Update Karo (M-1 + M-4)

**Config file:** `config/mainnet-genesis.json` (exists — placeholder addresses hain abhi)

```bash
# Step 4a: Placeholder addresses replace karo with real keygen output

# Current state (placeholder — rejected on mainnet):
# validators: ["0x0000000000000000000000000000000000002001", ...]

# Required state (real secp256k1 addresses):
# validators: ["0xABC123...real entropy...", ...]

# Edit config/mainnet-genesis.json:
# 1. validators[] — sab validator addresses real hone chahiye
# 2. alloc[] — treasury/fund addresses real wallet addresses
# 3. timestamp — mainnet launch datetime (Unix seconds)
# 4. chain_id — 8989 (already set in mainnet.toml)
```

**Example `mainnet-genesis.json` diff:**

```json
{
  "chain_id": 8989,
  "timestamp": 1782000000,
  "validators": [
    "0xRealValidator1Address20BytesFull",
    "0xRealValidator2Address20BytesFull",
    "0xRealValidator3Address20BytesFull",
    "0xRealValidator4Address20BytesFull"
  ],
  "alloc": [
    {
      "address": "0xRealTreasuryMultisig20Bytes",
      "balance": 100000000000000000000000000
    }
  ]
}
```

```bash
# Step 4b: Genesis block hash compute karo
cargo run --release -p zbx-genesis -- \
    build config/mainnet-genesis.json \
    > /tmp/mainnet-genesis-build.log

# Hash extract karo:
MAINNET_HASH=$(grep "genesis_block_hash:" /tmp/mainnet-genesis-build.log \
    | awk '{print $2}')
echo "MAINNET_GENESIS_HASH = $MAINNET_HASH"
# Example: 0x1a2b3c4d...64 hex chars...

# Hash ko hex bytes mein convert karo [u8; 32] format ke liye:
python3 -c "
h = '$MAINNET_HASH'.lstrip('0x')
assert len(h) == 64, 'Must be 32 bytes'
arr = [int(h[i:i+2], 16) for i in range(0, 64, 2)]
print('pub const MAINNET_GENESIS_HASH: H256 = H256([')
print('   ', ', '.join(str(b) for b in arr))
print(']);')
"
```

---

## STEP 5 — Genesis Hash Pin Karo (M-1)

**File to edit:** `crates/zbx-types/src/pinned_genesis.rs:57`

**Current state (sentinel — boot blocked):**

```rust
pub const MAINNET_GENESIS_HASH: H256 = SENTINEL_HASH;  // [0xFF; 32]
```

**Replace with real hash:**

```rust
pub const MAINNET_GENESIS_HASH: H256 = H256([
    0x1a, 0x2b, 0x3c, 0x4d,  // ← real bytes from Step 4b
    // ... 32 bytes total
]);
```

```bash
# Sed se replace karo (ya manually edit karo):
# crates/zbx-types/src/pinned_genesis.rs line 57 update karo

# Verify karo ki sentinel nahi raha:
grep "MAINNET_GENESIS_HASH" crates/zbx-types/src/pinned_genesis.rs
# Should NOT contain "SENTINEL_HASH" for MAINNET_GENESIS_HASH line
```

---

## STEP 6 — FeeRegistry Governance + Treasury Address Set Karo (M-6)

**Code guard:** `crates/zbx-pool/src/registry.rs:146-169`

```rust
// Default zero address — intentional sentinel, fails loudly:
pub governance: [u8; 20],  // Must be mainnet governance multisig
pub treasury:   [u8; 20],  // Must be mainnet treasury multisig
```

**Genesis config mein set karo:**

```bash
# config/mainnet-genesis.json mein add karo:
# "fee_registry": {
#   "governance": "0xYourGovernanceMultisig3of5",
#   "treasury":   "0xYourTreasuryMultisig4of7"
# }

# Recommended: Gnosis Safe multisig use karo
# Governance: 3-of-5 (core team keys)
# Treasury: 4-of-7 (includes community representatives)
```

---

## STEP 7 — PLONK SRS Supply Karo (M-5)

**Code guard:** `crates/zbx-zk/src/plonk.rs:514-518`

```rust
if is_srs_sentinel(srs) {
    return Err(VerifyError::PlonkSrsNotInitialized);
}
```

PLONK sirf ZK proof verification ke liye use hota hai (optional ZK features).
Agar launch pe ZK features disable karna chahte ho toh ye defer kar sakte ho.

```bash
# Option A: KZG ceremony se PLONK SRS derive karo
cargo run --release -p zbx-zk -- \
    derive-plonk-srs \
    --kzg-ceremony /etc/zbx/kzg_g2_tau.bin \
    --output /etc/zbx/plonk_srs.bin

# Node config mein path set karo (config/mainnet.toml):
# [zk]
# plonk_srs_path = "/etc/zbx/plonk_srs.bin"

# Option B: ZK features launch ke baad enable karo
# config/mainnet.toml mein:
# [zk]
# enabled = false  # Mainnet v1 pe disable, v1.1 pe enable
```

---

## STEP 8 — Mainnet Binary Build Karo

**Sab previous steps complete hone ke baad:**

```bash
# Clean build
cargo clean

# Production build
export ZBX_CHAIN_ENV=mainnet
cargo build --release --features zvm

# Binary location:
ls -la target/release/zbx
# Expected: ~50-80 MB ELF binary

# Verify mainnet chain_id compile hua:
strings target/release/zbx | grep "8989"
# Expected: chain_id 8989 string present

# Verify NO devnet placeholder strings:
strings target/release/zbx | grep -i "devnet.*tau\|allow_stubs" || echo "CLEAN — no devnet strings"
```

---

## STEP 9 — External Solidity Audit (M-7)

**Scope:** 133 Solidity files, 40 interfaces, 17 Foundry tests

```
Critical contracts (highest priority):
  contracts/ZbxBridge.sol         — funds lock/unlock, cross-chain
  contracts/ZUSD.sol              — stablecoin minting/burning
  contracts/ZusdVault.sol         — collateral management
  contracts/ZbxGovernor.sol       — on-chain upgrades
  contracts/ZbxPerpetuals.sol     — leveraged trading (1-200x)

High priority:
  contracts/ZbxAMM.sol            — DEX core
  contracts/ZbxRouter.sol         — swap routing
  contracts/ZbxLending.sol        — borrow/supply

Recommended firms:
  - Trail of Bits      (4-6 weeks, ~$150k)
  - OpenZeppelin       (4-8 weeks, ~$120k)
  - Halborn            (3-5 weeks, ~$80k)
  - Code4rena contest  (2 weeks, variable cost — competitive audit)

Run internally first:
  cd contracts && forge test -vvv
  # Expected: 17 tests, all pass

  slither . --config-file scripts/slither.config.json
```

**Timeline:** Audit ke bina mainnet launch mat karo. Recommended: audit parallel mein karo jabki testnet stable chal raha hai.

---

## STEP 10 — Mainnet Config File Verify Karo

**File:** `config/mainnet.toml` (already exists)

```toml
# Current mainnet.toml se verified values:
chain_id    = 8989                          # ✅ Correct
cors_allow  = "https://app.zebvix.com"      # ✅ Restricted (not *)
rate_limit  = 100                           # ✅ Conservative

# Ye values add/verify karo:
[node]
data_dir    = "/var/lib/zbx-mainnet"        # Separate from testnet
log_level   = "info"                        # NOT "debug" on mainnet

[rpc]
port        = 8545
ws_port     = 8546

[p2p]
port        = 30303
boot_nodes  = [
    # Chair/founder ke PGP-signed bootnode list se lo
    # Forum posts se mat lo
]

[kzg]
ceremony_path = "/etc/zbx/kzg_g2_tau.bin"  # Step 1 ka output

[ai]
model_dir   = "/etc/zbx/models"             # Step 2 ka output
allow_stubs = false                         # PRODUCTION LOCK

[zk]
plonk_srs_path = "/etc/zbx/plonk_srs.bin"  # Step 7 ka output

[monitoring]
prometheus_port = 9100
grafana_enabled = true
```

---

## STEP 11 — Pre-flight Checklist Run Karo

**Script:** `scripts/mainnet-launch.sh` (already exists in codebase)

```bash
# Script 7 categories check karta hai:
bash scripts/mainnet-launch.sh

# Expected output:
# === ZBX Chain Mainnet Launch Checklist ===
#
# ── 1. Node binary ──────────────────────────
#   ✓ zbx binary exists
#   ✓ zbx version matches
#
# ── 2. Configuration ────────────────────────
#   ✓ mainnet.toml exists
#   ✓ chain_id = 8989
#   ✓ genesis.json exists
#   ✓ validators in genesis
#
# ── 3. Cryptographic material ───────────────
#   ✓ node key exists
#   ✓ validator key exists
#   ✓ BLS key exists
#
# ── 4. Network ──────────────────────────────
#   ✓ port 30303 open
#   ✓ port 8545 open
#   ✓ minimum peers connected
#
# ── 5. Smart contracts ──────────────────────
#   ✓ deployment file exists
#   ✓ ZbxOracle deployed
#   ✓ ZbxVerifier deployed
#
# ── 6. Security ─────────────────────────────
#   ✓ SSL certs present
#   ✓ no debug flags in binary
#
# ── 7. Monitoring ───────────────────────────
#   ✓ Prometheus accessible
#   ✓ Grafana accessible
#
# ═══════════════════════════════════════════
# Results: 17 passed, 0 failed
# ✓ ALL CHECKS PASSED — ready to produce genesis block
```

---

## STEP 12 — Mainnet Node Start Karo

```bash
# Systemd service install karo:
sudo bash scripts/mainnet-deploy.sh

# Service status verify karo:
sudo systemctl status zbx-mainnet

# Chain ID confirm karo:
curl -s http://localhost:8545 \
  -d '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}'
# Expected: "result":"0x231d"   (8989 decimal = 0x231D hex)

# Genesis hash confirm karo:
sudo journalctl -u zbx-mainnet --no-pager | grep -i "genesis"
# Expected log line with real hash (NOT 0xffff...ffff sentinel)

# Testnet alag chal raha hai:
curl -s http://localhost:18545 \
  -d '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}'
# Expected: "result":"0x231e"   (8990 = testnet, still running)
```

---

## Full Sequence Summary

```
TESTNET VPS (stable, 30+ days)
        │
        ▼
[Step 1]  KZG ceremony file download → /etc/zbx/kzg_g2_tau.bin
        │
        ▼
[Step 2]  AI models train + export → /etc/zbx/models/*.zbxw (12 files)
        │
        ▼                              ← Parallel karo Steps 2+7+Audit
[Step 7]  PLONK SRS derive → /etc/zbx/plonk_srs.bin
        │
        ▼
[Step 9]  External Solidity audit ── (4-8 weeks, START EARLY)
        │
        ▼
[Step 3]  zbx-keygen generate (air-gapped machine)
        │
        ▼
[Step 4]  mainnet-genesis.json update (real addresses, real timestamp)
        │
        ▼
[Step 4b] genesis block hash compute → MAINNET_GENESIS_HASH bytes
        │
        ▼
[Step 5]  crates/zbx-types/src/pinned_genesis.rs line 57 update
        │
        ▼
[Step 6]  FeeRegistry governance + treasury set in genesis
        │
        ▼
[Step 8]  cargo build --release --features zvm
        │
        ▼
[Step 10] config/mainnet.toml final review
        │
        ▼
[Step 11] bash scripts/mainnet-launch.sh  ← ALL 17 CHECKS PASS?
        │
        ▼
[Step 12] sudo bash scripts/mainnet-deploy.sh
        │
        ▼
        🟢 MAINNET LIVE — chain_id 8989
```

---

## Realistic Timeline

| Step | Task | Duration | Parallel? |
|---|---|---|---|
| M-7 | External Solidity audit | 4–8 weeks | ✅ Start Day 1 |
| M-3 | AI model training (12 models) | 2–4 weeks | ✅ Start Day 1 |
| M-1 | KZG ceremony download + convert | 1–2 days | ✅ Start Day 1 |
| M-5 | PLONK SRS derive | 1 day | After KZG |
| M-4 | Key generation (air-gapped) | 1 day | Any time |
| — | Genesis file + hash computation | 1 day | After keys |
| — | Binary build + testing | 2–3 days | After pin |
| — | Pre-flight + validator coordination | 3–7 days | After binary |
| **Total** | | **~6–10 weeks** | |

**Bottleneck:** External Solidity audit. Baaki sab parallel ho sakta hai.

---

## Common Mistakes to Avoid

| Mistake | Consequence | Prevention |
|---|---|---|
| Testnet BLS key reuse on mainnet | Equivocation slash in first 5 min | Always `zbx-keygen generate` fresh |
| Forgetting `ZBX_CHAIN_ENV=mainnet` | AI stub used silently | Build script mein set karo |
| KZG ceremony file wrong format | Pairing check fails for all blobs | Verify 96 bytes exact |
| Sentinel hash not replaced | Node refuses to connect to peers | `grep SENTINEL` before build |
| Genesis timestamp in past | Node stuck at height 0 | Set timestamp to future launch time |
| Same data dir as testnet | Genesis hash mismatch panic | `/var/lib/zbx-mainnet` — separate |
| Two machines with same validator key | Equivocation slash | Only one live signer per key |
| Debug build on mainnet | Slow + memory leaks | Always `--release` |

---

## Verification Commands (Post-Launch)

```bash
# 1. Chain ID correct:
curl -s localhost:8545 \
  -d '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}'
# → 0x231d (8989)

# 2. Node is producing blocks:
zbx-node admin status
# → latest_block: N (increasing)

# 3. KZG real (not placeholder):
sudo journalctl -u zbx-mainnet | grep "KZG:"
# → "KZG: loaded real τ·G2 ceremony point"

# 4. AI weights real (not stubs):
sudo journalctl -u zbx-mainnet | grep "AI:"
# → "AI: loaded real INT8 weights from disk" (12 times)

# 5. Genesis hash pinned (not sentinel):
sudo journalctl -u zbx-mainnet | grep "genesis"
# → real 32-byte hash, NOT 0xffffffffffffffff...

# 6. Equivocation zero:
curl -s localhost:9100/metrics | grep zbx_equivocation_alerts_total
# → 0

# 7. Peers connected (minimum 3):
curl -s localhost:8545 \
  -d '{"jsonrpc":"2.0","method":"net_peerCount","params":[],"id":1}'
# → >=3

# 8. Testnet still independent:
curl -s localhost:18545 \
  -d '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}'
# → 0x231e (8990) — completely separate
```

---

*Guide code-verified on 2026-06-29. All file paths and panic messages confirmed from direct source reading. No document inference used.*
