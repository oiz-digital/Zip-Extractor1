# ZBX Chain — VPS Deployment Guide
**Chain ID: 8989 | Zebvix Chain | PoS / HotStuff-BFT**

---

## Quick Start (Automated)

```bash
# On your VPS (Ubuntu 22.04 recommended):
git clone https://github.com/zebvix/zbx-chain
cd zbx-chain
sudo bash deploy/vps-setup.sh
```

That's it. The script handles everything automatically.

---

## Manual Step-by-Step

### 1. VPS Requirements

| Component | Minimum | Recommended |
|-----------|---------|-------------|
| CPU       | 4 vCPU  | 8 vCPU      |
| RAM       | 8 GB    | 16 GB       |
| Disk      | 200 GB SSD | 500 GB NVMe |
| OS        | Ubuntu 22.04 | Ubuntu 22.04 |
| Network   | 100 Mbps | 1 Gbps     |

### 2. Build Binary

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Build
export LIBCLANG_PATH=/usr/lib/llvm-14/lib
cargo build --release -p zbx-node
```

### 3. Generate Validator Keys

Run this on EACH validator VPS:

```bash
bash scripts/keygen.sh --network mainnet --output-dir /etc/zbx/keys
```

This generates:
- `/etc/zbx/keys/node.key`       — P2P identity (secp256k1)
- `/etc/zbx/keys/validator.key`  — Consensus signing (Ed25519)
- `/etc/zbx/keys/bls.key`        — BLS aggregate signatures
- `/etc/zbx/keys/keystore.json`  — Encrypted backup

**Get your addresses** (needed for genesis):
```bash
zbx-node key show-address --key /etc/zbx/keys/validator.key
# Output: 0xYOUR_VALIDATOR_ETH_ADDRESS

zbx-node key show-pubkey --key /etc/zbx/keys/bls.key
# Output: 0xYOUR_BLS_PUBLIC_KEY
```

### 4. Fill Genesis File

**Option A — Interactive wizard (recommended):**
```bash
bash deploy/genesis-fill.sh
```

**Option B — Manual:**
Edit `config/mainnet-genesis.json` and replace:

| Placeholder | Replace with |
|-------------|-------------|
| `FILL_VALIDATOR_N_ETH_ADDRESS` | Ethereum address from `key show-address` |
| `FILL_VALIDATOR_N_BLS_PUBKEY`  | BLS pubkey from `key show-pubkey` |
| `FILL_FOUNDATION_MULTISIG`     | Gnosis Safe multisig address |
| `FILL_AMM_POOL_CONTRACT`       | ZbxAMM contract address (deploy first) |
| `FILL_TREASURY_GOVERNANCE`     | Governance/DAO address |
| `FILL_TEAM_VESTING_CONTRACT`   | VestingWallet contract address |
| `FILL_ECOSYSTEM_GRANTS`        | Ecosystem committee multisig |
| `FILL_LAUNCH_TIMESTAMP_UTC`    | e.g. `2025-06-01T00:00:00Z` |

**Balance amounts (in wei = ZBX × 10^18):**
```
9,990,000 ZBX   = "9990000000000000000000000"
20,000,000 ZBX  = "20000000000000000000000000"
5,000,000 ZBX   = "5000000000000000000000000"
3,000,000 ZBX   = "3000000000000000000000000"
2,000,000 ZBX   = "2000000000000000000000000"
100 ZBX         = "100000000000000000000"        ← validator min self-stake
10 ZBX          = "10000000000000000000"         ← delegator minimum per delegation
```

### 5. Deploy Smart Contracts (Optional Pre-genesis)

Some allocation addresses need contracts deployed first:

```bash
# Deploy AMM + governance + vesting (on testnet first to verify):
bash scripts/deploy-contracts.sh testnet --verify
# Then:
bash scripts/deploy-contracts.sh mainnet --verify --ledger
```

Take the deployed addresses and put them in the genesis file.

### 6. Initialize Genesis (All Validators)

Every validator node runs this:

```bash
zbx-node init \
  --genesis config/mainnet-genesis.json \
  --config  config/mainnet.toml \
  --data-dir /var/lib/zbx/mainnet
```

### 7. Verify Genesis Hash (Critical)

All validators MUST agree on the genesis hash before starting:

```bash
zbx-node genesis-hash --genesis config/mainnet-genesis.json
# Everyone must get the same hash!
```

Share this hash in your validator coordination channel.

### 8. Start Node

```bash
# systemd (recommended):
sudo systemctl enable --now zbx-mainnet

# Or manually:
zbx-node start \
  --config config/mainnet.toml \
  --validator-key /etc/zbx/keys/validator.key \
  --bls-key /etc/zbx/keys/bls.key \
  --node-key /etc/zbx/keys/node.key
```

### 9. Verify Node is Running

```bash
# Check sync status
curl -X POST http://localhost:8545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"eth_syncing","params":[]}'

# Check block number
curl -X POST http://localhost:8545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"eth_blockNumber","params":[]}'

# Check peer count
curl -X POST http://localhost:8545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"net_peerCount","params":[]}'

# Run full launch checklist
bash scripts/mainnet-launch.sh
```

---

## Firewall Rules

```bash
ufw allow 22/tcp     # SSH
ufw allow 30303/tcp  # P2P
ufw allow 30303/udp  # P2P UDP
ufw allow 80/tcp     # HTTP (certbot)
ufw allow 443/tcp    # HTTPS RPC (via nginx)
# DO NOT open 8545 or 8546 directly — nginx proxies them
```

## TLS Setup (nginx)

```bash
# Install certbot
apt install certbot python3-certbot-nginx

# Get certificate
certbot certonly --webroot \
  -w /var/www/letsencrypt \
  -d rpc.yourdomain.com

# Configure nginx
cp deploy/nginx/zbx-rpc.conf /etc/nginx/sites-available/
# Edit server_name and cert paths
ln -s /etc/nginx/sites-available/zbx-rpc.conf /etc/nginx/sites-enabled/
nginx -t && systemctl reload nginx
```

---

## Key Files

| File | Purpose |
|------|---------|
| `/etc/zbx/mainnet.toml`           | Node configuration |
| `/etc/zbx/mainnet-genesis.json`   | Genesis block |
| `/etc/zbx/keys/validator.key`     | Consensus signing key (SECRET) |
| `/etc/zbx/keys/bls.key`           | BLS key (SECRET) |
| `/etc/zbx/keys/node.key`          | P2P identity (SECRET) |
| `/var/lib/zbx/mainnet/`           | Chain data |
| `/var/log/zbx/`                   | Logs |

## Useful Commands

```bash
journalctl -u zbx-mainnet -f              # Live logs
systemctl status zbx-mainnet              # Service status
systemctl restart zbx-mainnet             # Restart
zbx-node admin status                     # Node status
zbx-node admin peers                      # Peer list
zbx-node admin validator-status           # Validator status
bash scripts/mainnet-launch.sh            # Full health check
```

---

## Validator Economics

| Parameter | Value |
|-----------|-------|
| Min validator self-stake | 100 ZBX |
| Min delegator stake | 10 ZBX |
| Block reward (Year 1) | 3.0 ZBX |
| Block time | 5 seconds |
| Halving interval | Every 25,000,000 blocks (~3.97 years) |
| Epoch length | 43,200 blocks (~2.5 days) |
| Commission range | 0–20% |
| Unbonding period | 7 days |
| Slash: double sign | -5% stake |
| Slash: downtime | -1% stake |
| Annual emission cap | 50,000,000 ZBX (5% of supply) |
