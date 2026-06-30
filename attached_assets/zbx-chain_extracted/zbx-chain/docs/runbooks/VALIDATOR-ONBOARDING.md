# Validator Onboarding Runbook — Zebvix Chain Mainnet

**Audience**: A new operator joining the Zebvix mainnet validator set (chain_id 8989).
**Time required**: ~3–4 hours over 2 days (most of it is hardware prep + secure key ceremony).
**Prerequisites**: Root-capable VPS, 100 ZBX self-stake, secure offline machine for key generation, and (strongly recommended) an HSM or Ledger device.

This document is intentionally step-by-step. Skipping steps will either:
- Burn your stake on a misconfigured tx, or
- Leak your validator key (irrecoverable — the address can be slashed even if you stop the node).

---

## 0. Hardware shopping list

| Item | Why it matters | Minimum |
|---|---|---|
| VPS (production node) | Hosts the live `zbx-node` process. | 16 GB RAM, 8 vCPU, 1 TB NVMe SSD, 1 Gbps, monthly uptime ≥99.9% |
| Offline workstation (key gen) | Generates BLS + secp256k1 keys without ever touching the internet. | Any laptop that can boot Tails OS / a fresh Ubuntu Live USB |
| HSM **or** Ledger Nano S/X | Custody of the BLS private key. | Required for production. A flat file with mode 0600 is **dev-only**. |
| 2× FIDO2 security keys | TOTP cannot replace these for SSH + chair-signer access. | YubiKey 5 / Solokey |
| 2× encrypted USB sticks | Air-gap transport of the public keyfile, plus offline backup of the encrypted private key. | hardware-encrypted (e.g. Apricorn) |

If you do not have an HSM/Ledger yet, **stop here and acquire one**. Validator keys protected only by file permissions on a network-attached host are out of scope for mainnet onboarding.

---

## 1. Generate your validator keys (OFFLINE, ~30 min)

**Do this on the offline workstation. The network cable must be physically unplugged. Wi-Fi disabled.**

### 1.1 Boot a clean OS

Tails OS Live USB or an Ubuntu 24.04 Live USB is recommended. Do NOT use your daily-driver laptop's installed OS — keylogger malware on the host would steal the private key the moment `zbx-keygen` prints it.

### 1.2 Copy the `zbx-keygen` binary onto the offline machine

From your build VPS or local dev machine:

```bash
# Build the binary (one-time)
cd zbx-chain
LIBCLANG_PATH=/usr/lib/llvm-15/lib cargo build --release --bin zbx-keygen
sha256sum target/release/zbx-keygen
```

Write `target/release/zbx-keygen` to a USB stick. Note the SHA256.

On the offline machine, verify the binary BEFORE running it:

```bash
sha256sum /media/usb/zbx-keygen   # must match the value you noted above
chmod +x /media/usb/zbx-keygen
```

### 1.3 Generate a single validator keyset

```bash
/media/usb/zbx-keygen --count 1 --output json > /media/usb/validator-keyset.json
chmod 600 /media/usb/validator-keyset.json
```

The JSON file contains:

```json
[{
  "evm_address":  "0x<20-byte hex>",
  "bls_pubkey":   "0x<48-byte hex>",
  "bls_privkey":  "0x<32-byte hex>",   // ⚠ SECRET
  "node_privkey": "0x<32-byte hex>"    // ⚠ SECRET
}]
```

### 1.4 Split the file into PUBLIC and PRIVATE halves

```bash
# Public half — safe to share with the chair-signer, kept on the production VPS
jq '[.[] | {evm_address, bls_pubkey}]' \
   /media/usb/validator-keyset.json \
   > /media/usb/validator-public.json

# Private half — NEVER leaves the offline machine until it's wrapped by HSM
jq '[.[] | {bls_privkey, node_privkey}]' \
   /media/usb/validator-keyset.json \
   > /media/usb/validator-private.json
```

### 1.5 Wrap the private keys with HSM / Ledger

How you do this depends on your device. Two acceptable patterns:

**Pattern A (Ledger Nano)**: Use the Ledger's hidden passphrase BIP-39 feature. Derive the BLS private key from a passphrase-protected seed phrase. The seed phrase lives only on the Ledger; the production VPS calls a thin signer agent over USB.

**Pattern B (YubiHSM 2 / AWS CloudHSM)**: Import the raw 32-byte BLS private key as a non-extractable key object. The production VPS authenticates to the HSM at boot and asks the HSM to sign each consensus message.

In both patterns the **production VPS never sees the raw private key bytes**. The `VALIDATOR_KEY` env var pattern in `mainnet-deploy.sh` is a DEV-ONLY fallback — flag it as such in your runbook.

### 1.6 Sanitize the offline machine

Once the private half is safely wrapped:

```bash
# Overwrite the keyset files with random bytes
shred -uvz /media/usb/validator-keyset.json /media/usb/validator-private.json
```

Carry off:
- `validator-public.json` — to the production VPS and chair-signer's machine
- HSM device or Ledger — to wherever it lives in your secure cabinet
- 2× sealed-envelope copies of the Ledger seed phrase (if you used Pattern A) — to two separate safety-deposit boxes

---

## 2. Provision the production VPS (~45 min)

### 2.1 Build & install `zbx-node`

```bash
git clone https://github.com/zebvix-org/zbx-chain
cd zbx-chain
sudo bash scripts/mainnet-deploy.sh --build-only
```

This compiles `zbx-node` + `zbx-keygen` in release mode and installs them to `/usr/local/bin/`. Verifies SHA256 of installed binary.

### 2.2 Install config (full node first, validator later)

```bash
sudo bash scripts/mainnet-deploy.sh --service-only
```

Verify:

```bash
sudo bash scripts/mainnet-deploy.sh --status
# Expect chain_id 0x231d (8989), block number > 0 within 30 seconds
```

### 2.3 Edit `/etc/zbx/mainnet.toml`

Required edits:
- `[p2p] boot_nodes` — at least 3 actual mainnet bootnodes
- `[rpc] cors_allow` — your dApp origin (NOT `*`)
- `[chain] genesis_file` — leave **unset** unless you intentionally want operator-supplied genesis. The hardcoded `GenesisConfig::mainnet()` is the canonical default.

Reload:

```bash
sudo systemctl restart zbx-mainnet
sudo journalctl -u zbx-mainnet -f
```

### 2.4 Firewall (CRITICAL for validators)

```bash
# Block RPC from public
sudo ufw deny 8545/tcp
sudo ufw deny 8546/tcp
# Allow only P2P
sudo ufw allow 30303/tcp
# Allow metrics scrape ONLY from your Prometheus IP
sudo ufw allow from <PROMETHEUS_IP> to any port 9100
sudo ufw enable
```

If you skip this step, attackers will hit your RPC and (a) drain your account-cache resources, (b) potentially exploit any future RPC bug to extract validator metadata.

### 2.5 Wait for full sync

```bash
# Watch tip catch up
watch -n 5 'curl -s -X POST http://127.0.0.1:8545 \
  -H "Content-Type: application/json" \
  -d "{\"jsonrpc\":\"2.0\",\"method\":\"eth_blockNumber\",\"params\":[],\"id\":1}"'
```

Do NOT proceed to validator mode until tip matches the public block explorer.

---

## 3. Self-stake 100 ZBX (~10 min)

The `validator-add` tx will revert if the candidate's `staking_escrow` balance is below `MIN_STAKE` (100 ZBX, 18 decimals). Stake first.

```bash
# Send 100 ZBX from your candidate EVM key to the staking_escrow precompile
zbx-node stake-deposit \
  --signer-key /etc/zbx/secrets/candidate-evm.key \
  --amount    100000000000000000000 \
  --rpc-url   http://127.0.0.1:8545
```

Verify:

```bash
curl -s -X POST http://127.0.0.1:8545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"zbx_getValidatorStake",
       "params":["0x<your_evm_address>"],"id":1}'
# Expect {"result":{"stake":"100000000000000000000", ...}}
```

---

## 4. Enable validator mode (~15 min)

The node has been running as a full-node-only so far. Now wire in the validator key via your HSM signer:

```bash
# If using HSM: configure the HSM-signer sidecar first, then:
sudo systemctl edit zbx-mainnet
# Add to the [Service] block:
#   Environment="VALIDATOR_HSM_URL=unix:///var/run/zbx-hsm.sock"
#   Environment="VALIDATOR_HSM_KEY_ID=zbx-validator-1"
sudo systemctl restart zbx-mainnet

# OR (dev-only, NOT recommended for production):
export VALIDATOR_KEY=0x<bls_privkey_hex>
sudo -E bash scripts/mainnet-deploy.sh --service-only --validator
```

The node is now able to produce consensus signatures, but **it is still dormant** — its `evm_address` is NOT in the on-chain validator set, so `who_proposes(...) != me` on every tick and the producer task skips its turn. This is by design — it lets you verify everything before you commit on-chain.

Verify your node sees itself as a validator candidate:

```bash
sudo journalctl -u zbx-mainnet -n 50 --no-pager | grep -i validator
# Expect: "validator mode enabled" or similar
# Should NOT see: "proposing block ..." until step 5 completes
```

---

## 5. Submit `validator-add` (chair signer side, ~5 min)

This step is performed by the existing chair/founder, NOT by the new operator. The chair runs:

```bash
# From the chair's machine, with the chair's HSM-protected signer
export ZBX_NEW_VALIDATOR_KEYFILE=/secure/transfer/validator-public.json
export ZBX_CHAIR_SIGNER_KEY=/secure/hsm/chair.key
export ZBX_MAINNET_RPC_URL=http://chair-node.zebvix.com:8545
export ZBX_NEW_VALIDATOR_POWER=1

# Verify everything first
sudo -E bash scripts/mainnet-add-validator.sh --verify-only

# Then submit
sudo -E bash scripts/mainnet-add-validator.sh
```

The script:
1. Verifies the candidate's stake ≥ MIN_STAKE
2. Computes new BFT threshold and refuses if liveness would be at risk
3. Submits the `validator-add` tx
4. Waits for the tx to be mined and the set to update
5. Verifies the new validator produces at least one vote in the next round

---

## 6. Post-add verification (~10 min)

On the new validator VPS:

```bash
# Should now show "proposing block ..." periodically
sudo journalctl -u zbx-mainnet -f | grep -E 'propos|vote|commit'

# Prometheus metrics — these MUST be near zero
curl -s http://127.0.0.1:9100/metrics | grep -E 'zbx_signing_misses_total|zbx_equivocation_alerts_total|zbx_view_changes_total'
```

Add alerts (see `monitoring/alerts/chain.yml` and `monitoring/alerts/validator.yml`) for:
- `zbx_signing_misses_total{address="0x<your_addr>"}` > 5 over 1h → page yourself
- `zbx_equivocation_alerts_total{address="0x<your_addr>"}` > 0 → page yourself IMMEDIATELY (slashing imminent)
- `zbx_peer_count` < 3 → warning

---

## 7. Ongoing operations (cheat sheet)

| Situation | Action |
|---|---|
| Node restarted, signing misses spike briefly | Normal during catch-up. Should reset within 10 min. |
| `zbx_equivocation_alerts_total` > 0 | **Stop the node immediately**: `sudo systemctl stop zbx-mainnet`. Investigate before restarting — usually a key-on-two-machines incident. |
| HSM rotation needed | (a) Generate new keyset offline. (b) `validator-remove` old key. (c) Wait one epoch. (d) `validator-add` new key. Do NOT skip the wait. |
| Mainnet hard-fork upgrade | Stop, pull new binary, verify SHA256 against the release announcement, run `mainnet-deploy.sh --build-only --service-only`. |
| Lost the HSM | Slashing is NOT triggered by loss alone, but you cannot sign. Issue `validator-remove` from the chair, then go through onboarding from scratch with new keys. |
| Want to add more self-stake | `zbx-node stake-deposit --amount <wei>` — increases your voting power weight. Effective next epoch. |
| Want to delegate from another wallet | The delegator (not you) runs `zbx-node delegate --validator 0x<your_addr> --amount <wei>`. |

---

## 8. References

- `scripts/mainnet-deploy.sh` — single-VPS production install (hardened systemd unit, validator + full-node modes)
- `scripts/mainnet-add-validator.sh` — chair-side N→N+1 quorum growth
- `docs/runbooks/TESTNET-TO-MAINNET-MIGRATION.md` — graduation playbook for operators who participated in testnet
- `docs/runbooks/INCIDENT-RESPONSE-RUNBOOK.md` — chain-halt, slashing, double-sign, p2p eclipse
- `docs/MAINNET_LAUNCH_CHECKLIST.md` — pre-mainnet go/no-go criteria
- `monitoring/prometheus.yml` + `monitoring/alerts/` — bring your own Prometheus + Alertmanager
- `monitoring/grafana/zbx_dashboard.json` — pre-built dashboard

---

**Security reminder**: The BLS private key signs consensus messages. Two nodes signing the same height with the same key = automatic slashing of the entire stake. NEVER copy the key to a second machine, even as a "warm spare". Hot-spare validators must have their own key + their own `validator-add`.
