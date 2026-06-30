# Testnet → Mainnet Migration Playbook

**Audience**: Operators who have run a validator on Zebvix testnet (chain_id 8990) and want to graduate to mainnet (chain_id 8989).
**Outcome**: A separate, isolated mainnet validator running alongside (or replacing) your testnet node, with **fresh keys**, **fresh state**, and **fresh stake**.

**The single most important rule**:

> **You do NOT migrate keys, state, or stake from testnet to mainnet. You start over with new everything.**

Testnet tokens have zero economic value and testnet keys are widely visible in logs and screenshots. Reusing either on mainnet would let any past observer of your testnet activity attack you. The migration is a process for transferring *operational experience* and *node infrastructure*, not data.

---

## 0. Why this playbook exists

Operators who succeed on testnet often try one of two failure modes:

1. **"Just point my testnet node at mainnet."** — Changes the chain ID but reuses the BLS key. The key has been in logs/screenshots/CI for weeks. First time you sign mainnet block, an attacker who already has your testnet key files (e.g. from a leaky CI artifact) signs a conflicting block at the same height → you get slashed for equivocation in the first 5 minutes.

2. **"Run mainnet from the testnet snapshot."** — Tries to import state from chain_id 8990 into a chain_id 8989 node. The genesis-hash mismatch check (see `node/src/genesis.rs::bootstrap_into`) refuses, but in a panicked debugging session some operator deletes the check and bricks their data dir.

This playbook avoids both by treating mainnet bring-up as a **clean install on the same VPS**, side-by-side with testnet, sharing the binary but nothing else.

---

## 1. Pre-flight checklist (T-7 days)

Run this 7 days before your target mainnet activation date.

- [ ] Read `docs/runbooks/VALIDATOR-ONBOARDING.md` end-to-end. The keygen-on-air-gapped-machine flow is non-negotiable.
- [ ] Read `docs/MAINNET_LAUNCH_CHECKLIST.md`. Confirm all 4 readiness predicates pass (BLS PoP, precompiles, snapshot binding, trie pruner).
- [ ] Acquire HSM or Ledger device. Order arrives in 3–5 days, schedule accordingly.
- [ ] Acquire 100 ZBX for self-stake. Confirm where it will come from on the day.
- [ ] Confirm chair/founder's PGP-signed bootnode list. Your `mainnet.toml` `[p2p] boot_nodes` must come from this list, not from a forum post.
- [ ] Decide: keep testnet running or shut it down? **Strong recommendation: keep testnet running for at least 30 days after mainnet activation.** You will discover bugs on mainnet that you want to reproduce against a known-good testnet.

---

## 2. VPS sizing (T-5 days)

Mainnet load > testnet load. If your testnet VPS is sized at the minimum, you need a bigger box for mainnet, not the same one.

| Resource | Testnet you ran | Mainnet you need |
|---|---|---|
| RAM | 8 GB | 16 GB minimum, 64 GB recommended |
| Storage | 500 GB SSD | 1 TB NVMe minimum, 2 TB recommended |
| Network | 100 Mbps | 1 Gbps |
| CPU | 4 cores | 8 cores minimum, 16 recommended |

If you upgrade in-place: take a snapshot, resize, boot, verify testnet still works, *then* proceed.

If you provision a new box: see step 4.

---

## 3. Key generation (T-3 days)

Follow `VALIDATOR-ONBOARDING.md` §1 exactly. Output:

- **Public half** (`validator-public.json`): EVM address + BLS pubkey. Will go to the chair signer.
- **Private half**: Wrapped by HSM/Ledger. Never touches the production VPS in raw form.

**Do NOT** reuse your testnet:
- BLS private key (`bls_privkey` field of any testnet keyfile)
- Node private key (`node_privkey` — secp256k1)
- EVM address used for testnet stake

Mainnet starts with a fresh `evm_address` that has zero observable on-chain history.

---

## 4. Provision mainnet service alongside testnet (T-1 day)

`mainnet-deploy.sh` is designed to co-exist with `testnet-deploy.sh` on the same VPS:

| | Testnet | Mainnet |
|---|---|---|
| systemd | `zbx-testnet.service` | `zbx-mainnet.service` |
| Data dir | `/var/lib/zbx-testnet` | `/var/lib/zbx-mainnet` |
| Config | `/etc/zbx/testnet.toml` | `/etc/zbx/mainnet.toml` |
| RPC port | 18545 | 8545 |
| P2P port | 30304 | 30303 |
| Metrics | 9101 | 9100 |
| Binary | `/usr/local/bin/zbx-node` | (same) |

Bring up mainnet as a full-node first (NOT validator yet):

```bash
sudo bash scripts/mainnet-deploy.sh           # build, install, start as full node
sudo bash scripts/mainnet-deploy.sh --status  # verify chain_id 0x231d (8989)
```

Edit `/etc/zbx/mainnet.toml` per `VALIDATOR-ONBOARDING.md` §2.3. Let it sync to the network tip. **Do NOT enable validator mode yet.**

While mainnet syncs, testnet keeps running on port 18545. You can `journalctl -u zbx-testnet -f` and `journalctl -u zbx-mainnet -f` in two windows side-by-side to confirm they are independent.

---

## 5. Self-stake (T-0, morning)

Send 100 ZBX from your new mainnet candidate EVM key to the staking_escrow precompile:

```bash
zbx-node stake-deposit \
  --signer-key /etc/zbx/secrets/candidate-evm.key \
  --amount    100000000000000000000 \
  --rpc-url   http://127.0.0.1:8545
```

Wait for the tx to be included. Verify with `zbx_getValidatorStake`.

---

## 6. Enable validator mode + chair adds you (T-0, ceremony hour)

This is the hand-off from "lurking full node" to "voting validator". Coordinated with the chair via a scheduled call.

**On your mainnet VPS** (5 min before the call):

```bash
# Configure HSM signer (or, dev-only: export VALIDATOR_KEY=...)
sudo systemctl restart zbx-mainnet
sudo journalctl -u zbx-mainnet -n 20 --no-pager | grep -i "validator"
# Expect: "validator mode enabled" — your node is now dormant-but-ready
```

Confirm you can see yourself in the candidate pool but NOT in the active set yet.

**On the chair's machine** (during the call, after they verify your screenshot of stake confirmation):

```bash
export ZBX_NEW_VALIDATOR_KEYFILE=/secure/transfer/<your>-validator-public.json
export ZBX_CHAIR_SIGNER_KEY=/secure/hsm/chair.key

sudo -E bash scripts/mainnet-add-validator.sh --verify-only   # all green?
sudo -E bash scripts/mainnet-add-validator.sh                  # submit
```

The script verifies stake, computes new BFT threshold, confirms enough voters are online to survive the new threshold, then submits `validator-add` and waits for the new set to apply.

**On your VPS** (within 60 seconds):

```bash
sudo journalctl -u zbx-mainnet -n 30 --no-pager | grep -E 'propos|vote'
# Expect: lines like "voted on height N", "proposing block at height M"
```

Congratulations. You are a mainnet validator.

---

## 7. Verify isolation from testnet (T+0)

Quick checks that mainnet and testnet are truly independent:

```bash
# Different chain IDs
curl -s -X POST http://127.0.0.1:8545 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}'
# Expect: "result":"0x231d"   (8989, mainnet)

curl -s -X POST http://127.0.0.1:18545 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}'
# Expect: "result":"0x231e"   (8990, testnet)

# Different data dirs, different genesis hashes
sudo ls /var/lib/zbx-mainnet/db   # populated with mainnet blocks
sudo ls /var/lib/zbx-testnet/db   # populated with testnet blocks

# Genesis hashes differ
sudo journalctl -u zbx-mainnet --no-pager | grep -i "genesis = "
sudo journalctl -u zbx-testnet --no-pager | grep -i "genesis = "
# Two different 32-byte hashes
```

---

## 8. Wind down testnet (T+30 days, optional)

After 30 days of stable mainnet operation:

```bash
# Stop testnet service
sudo systemctl stop zbx-testnet
sudo systemctl disable zbx-testnet

# Archive testnet data (don't delete — useful for bug reports)
sudo tar czf /backup/zbx-testnet-final-$(date +%Y%m%d).tgz /var/lib/zbx-testnet
sudo rm -rf /var/lib/zbx-testnet

# Remove the systemd unit
sudo rm /etc/systemd/system/zbx-testnet.service
sudo systemctl daemon-reload
```

You now have a single, mainnet-only VPS.

---

## 9. Migration fast-fail table

| Symptom | Cause | Fix |
|---|---|---|
| `mainnet-deploy.sh` refuses with "wrong chain_id 0x231e" | You accidentally pointed mainnet at testnet's data dir | Check `[node] data_dir` in `/etc/zbx/mainnet.toml` — must be `/var/lib/zbx-mainnet`, not `/var/lib/zbx-testnet` |
| Node starts but `eth_blockNumber` stays at 0 | No peers — bootnodes wrong or firewall blocks 30303 | `sudo ss -tln \| grep 30303`; check `ufw status` |
| `validator-add` reverts with "stake too low" | Stake tx hasn't been mined yet, or you staked from a different address | Wait for stake tx confirmation; verify with `zbx_getValidatorStake` |
| `validator-add` succeeds but you see "proposer mismatch" in your logs | Two machines holding the same key (the testnet one is also signing!) | Stop the testnet validator IMMEDIATELY. Rotate keys. You may already be slashed — check `zbx_equivocation_alerts_total` |
| Mainnet uses 100% CPU and falls behind tip | VPS undersized | See §2 sizing table — minimum is 8 cores |
| Mainnet and testnet ports collide | You overrode `ZBX_MAINNET_*_PORT` env vars to testnet values | Revert; the defaults are deliberately non-colliding |
| Genesis-hash mismatch on first boot | Operator-supplied genesis file with wrong hash | Unset `chain.genesis_file` in `mainnet.toml` to use the canonical preset |

---

## 10. Why this is conservative

The Zebvix codebase has a `BootstrapPolicy::StrictFailFast` mode that refuses to start if the on-disk genesis hash differs from the configured one. This is what protects you from the worst migration mistakes — but it also means a sloppy migration will leave you with a node that REFUSES to start, blocking your participation in the launch.

Following this playbook avoids the refusal cases entirely. The cost is ~3 hours of duplicated infrastructure work; the benefit is that you reach mainnet activation with confidence that your node is correctly isolated, your keys are fresh, and your stake is exactly where you expect it.
