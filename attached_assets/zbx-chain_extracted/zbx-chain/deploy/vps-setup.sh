#!/usr/bin/env bash
# =============================================================================
# ZBX Chain — Full VPS Deployment Script
# =============================================================================
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/zebvix/zbx-chain/main/deploy/vps-setup.sh | bash
#   OR: chmod +x deploy/vps-setup.sh && ./deploy/vps-setup.sh
#
# What this script does:
#   1. Installs all system dependencies (Rust, Cargo, Clang, etc.)
#   2. Builds the zbx-node binary from source
#   3. Creates system user and directory structure
#   4. Copies configs, genesis, systemd service
#   5. Prompts you to fill in real addresses
#   6. Generates validator keys
#   7. Initializes genesis block
#   8. Starts the node as a systemd service
#   9. Sets up nginx reverse proxy + TLS (optional)
#   10. Enables firewall rules
#
# Requirements:
#   - Ubuntu 22.04 / Debian 12 (recommended)
#   - 4+ vCPU, 8+ GB RAM, 200+ GB SSD
#   - Run as root or with sudo
# =============================================================================

set -euo pipefail

# ── Colors ───────────────────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'

log()  { echo -e "${GREEN}[ZBX]${NC} $*"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }
err()  { echo -e "${RED}[ERR]${NC} $*" >&2; }
step() { echo -e "\n${CYAN}${BOLD}══ $* ══${NC}"; }

# ── Configuration — edit these before running ─────────────────────────────────
CHAIN_ID=8989
NETWORK="mainnet"
ZBX_USER="zbx"
ZBX_HOME="/opt/zbx"
ZBX_DATA="/var/lib/zbx/mainnet"
ZBX_KEYS="/etc/zbx/keys"
ZBX_LOG="/var/log/zbx"
ZBX_BIN="/usr/local/bin/zbx-node"
ZBX_KEYGEN="/usr/local/bin/zbx-keygen"
GENESIS_SRC="$(dirname "$0")/../config/mainnet-genesis.json"
CONFIG_SRC="$(dirname "$0")/../config/mainnet.toml"
SOURCE_DIR="$(dirname "$0")/.."

# ── Root check ────────────────────────────────────────────────────────────────
if [[ $EUID -ne 0 ]]; then
  err "Run as root: sudo bash deploy/vps-setup.sh"
  exit 1
fi

echo -e "${BOLD}"
cat << 'BANNER'
  ╔═══════════════════════════════════════════╗
  ║    ZBX Chain — VPS Deployment Installer   ║
  ║    Zebvix Chain  |  Chain ID 8989         ║
  ╚═══════════════════════════════════════════╝
BANNER
echo -e "${NC}"

# ═════════════════════════════════════════════════════════════════════════════
step "STEP 1 — System Dependencies"
# ═════════════════════════════════════════════════════════════════════════════

log "Updating package lists..."
apt-get update -qq

log "Installing build dependencies..."
apt-get install -y --no-install-recommends \
  build-essential curl git pkg-config libssl-dev \
  clang libclang-dev cmake \
  nginx certbot python3-certbot-nginx \
  ufw jq python3 \
  ca-certificates

# Install Rust if not present
if ! command -v cargo &>/dev/null; then
  log "Installing Rust toolchain..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
  source "$HOME/.cargo/env"
else
  log "Rust already installed: $(rustc --version)"
  source "$HOME/.cargo/env" 2>/dev/null || true
fi

export PATH="$HOME/.cargo/bin:$PATH"

# ═════════════════════════════════════════════════════════════════════════════
step "STEP 2 — Build ZBX Node Binary"
# ═════════════════════════════════════════════════════════════════════════════

log "Building zbx-node (this takes 5-15 minutes)..."
cd "$SOURCE_DIR"

LIBCLANG_PATH=$(find /usr/lib -name "libclang*.so*" 2>/dev/null | head -1 | xargs dirname || echo "/usr/lib/llvm-14/lib")
export LIBCLANG_PATH

cargo build --release -p zbx-node --bin zbx-node 2>&1 | tail -5

if [[ -f "target/release/zbx-node" ]]; then
  log "Binary built successfully"
else
  err "Build failed — check errors above"
  exit 1
fi

# Build keygen tool
cargo build --release -p zbx-cli --bin zbx-keygen 2>/dev/null || \
  cargo build --release --bin zbx-keygen 2>/dev/null || \
  warn "zbx-keygen not built (separate binary) — will use zbx-node key subcommand"

# ═════════════════════════════════════════════════════════════════════════════
step "STEP 3 — Create System User & Directory Structure"
# ═════════════════════════════════════════════════════════════════════════════

if ! id "$ZBX_USER" &>/dev/null; then
  log "Creating system user: $ZBX_USER"
  useradd --system --no-create-home --shell /usr/sbin/nologin "$ZBX_USER"
else
  log "User $ZBX_USER already exists"
fi

log "Creating directories..."
mkdir -p "$ZBX_DATA" "$ZBX_HOME" "$ZBX_KEYS" "$ZBX_LOG" \
         /etc/zbx /var/www/letsencrypt

chown -R "$ZBX_USER:$ZBX_USER" "$ZBX_DATA" "$ZBX_LOG"
chmod 700 "$ZBX_KEYS"

# ═════════════════════════════════════════════════════════════════════════════
step "STEP 4 — Install Binaries & Configs"
# ═════════════════════════════════════════════════════════════════════════════

log "Installing zbx-node binary..."
install -m 755 "$SOURCE_DIR/target/release/zbx-node" "$ZBX_BIN"
[[ -f "$SOURCE_DIR/target/release/zbx-keygen" ]] && \
  install -m 755 "$SOURCE_DIR/target/release/zbx-keygen" "$ZBX_KEYGEN"

log "Copying config files..."
cp "$CONFIG_SRC"  /etc/zbx/mainnet.toml
cp "$GENESIS_SRC" /etc/zbx/mainnet-genesis.json

log "Installed: $(zbx-node --version 2>/dev/null || echo 'zbx-node')"

# ═════════════════════════════════════════════════════════════════════════════
step "STEP 5 — Fill Genesis Placeholders"
# ═════════════════════════════════════════════════════════════════════════════

echo ""
echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}Enter real wallet addresses for genesis allocations.${NC}"
echo -e "Use MetaMask, Gnosis Safe, or hardware wallet addresses."
echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

prompt_addr() {
  local label="$1" var_name="$2"
  local addr=""
  while true; do
    read -rp "  $label (0x...): " addr
    if [[ "$addr" =~ ^0x[0-9a-fA-F]{40}$ ]]; then
      eval "$var_name='$addr'"
      break
    else
      warn "Invalid address format. Must be 0x + 40 hex chars."
    fi
  done
}

prompt_addr "Foundation Multisig address  (9,990,000 ZBX)" FOUNDATION_ADDR
prompt_addr "AMM Pool contract address    (20,000,000 ZBX)" AMM_ADDR
prompt_addr "Treasury/Governance address  (5,000,000 ZBX)"  TREASURY_ADDR
prompt_addr "Team Vesting wallet address  (3,000,000 ZBX)"  TEAM_ADDR
prompt_addr "Ecosystem Grants address     (2,000,000 ZBX)"  ECOSYSTEM_ADDR

echo ""
read -rp "  Launch timestamp (YYYY-MM-DDTHH:MM:SSZ) [default: 2025-06-01T00:00:00Z]: " LAUNCH_TIME
LAUNCH_TIME="${LAUNCH_TIME:-2025-06-01T00:00:00Z}"

log "Patching genesis with real addresses..."
python3 << PYEOF
import json, sys

with open('/etc/zbx/mainnet-genesis.json') as f:
    g = json.load(f)

g['timestamp'] = '$LAUNCH_TIME'

for alloc in g.get('allocations', []):
    c = alloc.get('comment','').lower()
    if 'foundation' in c:
        alloc['address'] = '$FOUNDATION_ADDR'
    elif 'amm' in c or 'liquidity' in c:
        alloc['address'] = '$AMM_ADDR'
    elif 'treasury' in c or 'governance' in c:
        alloc['address'] = '$TREASURY_ADDR'
    elif 'team' in c or 'vesting' in c:
        alloc['address'] = '$TEAM_ADDR'
    elif 'ecosystem' in c or 'grant' in c:
        alloc['address'] = '$ECOSYSTEM_ADDR'

with open('/etc/zbx/mainnet-genesis.json', 'w') as f:
    json.dump(g, f, indent=2)

print('  Genesis allocations updated.')
PYEOF

# ═════════════════════════════════════════════════════════════════════════════
step "STEP 6 — Generate Validator Keys"
# ═════════════════════════════════════════════════════════════════════════════

log "Generating cryptographic keys for this validator node..."
echo ""
echo -e "${YELLOW}⚠  IMPORTANT: Back up all keys in $ZBX_KEYS after this step.${NC}"
echo ""

mkdir -p "$ZBX_KEYS"
chmod 700 "$ZBX_KEYS"

# Generate using zbx-keygen or zbx-node key subcommand
if command -v zbx-keygen &>/dev/null; then
  ZBX_KEYGEN_CMD="zbx-keygen"
else
  ZBX_KEYGEN_CMD="zbx-node key"
fi

echo "  [1/3] Generating node identity key (secp256k1)..."
$ZBX_KEYGEN_CMD generate --type secp256k1 --output "$ZBX_KEYS/node.key" || {
  warn "zbx-keygen subcommand not available — using openssl fallback for node key"
  openssl ecparam -name secp256k1 -genkey -noout -out "$ZBX_KEYS/node.key"
}

echo "  [2/3] Generating validator key (Ed25519)..."
$ZBX_KEYGEN_CMD generate --type ed25519 --output "$ZBX_KEYS/validator.key" || {
  warn "Generating Ed25519 key via openssl fallback..."
  openssl genpkey -algorithm Ed25519 -out "$ZBX_KEYS/validator.key"
}

echo "  [3/3] Generating BLS key (BLS12-381)..."
$ZBX_KEYGEN_CMD generate --type bls --output "$ZBX_KEYS/bls.key" || {
  warn "BLS keygen: requires zbx-keygen binary. Placeholder created."
  echo '{"type":"bls12-381","status":"REPLACE_WITH_REAL_BLS_KEY"}' > "$ZBX_KEYS/bls.key"
}

chmod 600 "$ZBX_KEYS"/*.key 2>/dev/null || true
chown -R "$ZBX_USER:$ZBX_USER" "$ZBX_KEYS"

echo ""
log "Keys generated:"
ls -la "$ZBX_KEYS/"

echo ""
echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD}Add this node's addresses to genesis BEFORE launching:${NC}"
echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "  Validator address (Ethereum-style):"
$ZBX_KEYGEN_CMD show-address --key "$ZBX_KEYS/validator.key" 2>/dev/null || \
  echo "  → Run: zbx-node key show-address --key $ZBX_KEYS/validator.key"
echo ""
echo "  BLS public key:"
$ZBX_KEYGEN_CMD show-pubkey --key "$ZBX_KEYS/bls.key" 2>/dev/null || \
  echo "  → Run: zbx-node key show-pubkey --key $ZBX_KEYS/bls.key"
echo ""

# ═════════════════════════════════════════════════════════════════════════════
step "STEP 7 — Update mainnet.toml"
# ═════════════════════════════════════════════════════════════════════════════

echo ""
read -rp "  Your VPS public IP address: " VPS_IP
read -rp "  Your RPC domain (e.g. rpc.zebvix.com) [leave blank to skip TLS]: " RPC_DOMAIN
read -rp "  Other boot node enodes (comma-separated, or leave blank): " BOOT_NODES

# Update mainnet.toml
python3 << PYEOF
import re

with open('/etc/zbx/mainnet.toml') as f:
    content = f.read()

# Update CORS
if '$RPC_DOMAIN':
    content = re.sub(
        r'cors_allow\s*=.*',
        'cors_allow = "https://$RPC_DOMAIN"',
        content
    )

# Update boot_nodes
if '$BOOT_NODES':
    nodes = ['  "enode://{n}"'.format(n=n.strip()) for n in '$BOOT_NODES'.split(',') if n.strip()]
    node_str = '[\n' + ',\n'.join(nodes) + '\n]'
    content = re.sub(r'boot_nodes\s*=\s*\[.*?\]', f'boot_nodes = {node_str}', content, flags=re.DOTALL)

# Update block_time to 5s
content = re.sub(r'block_time\s*=\s*\d+', 'block_time = 5', content)

# Update data_dir
content = content.replace('/var/lib/zbx/mainnet', '$ZBX_DATA')

with open('/etc/zbx/mainnet.toml', 'w') as f:
    f.write(content)

print('  mainnet.toml updated.')
PYEOF

# ═════════════════════════════════════════════════════════════════════════════
step "STEP 8 — Initialize Genesis"
# ═════════════════════════════════════════════════════════════════════════════

log "Checking genesis for placeholder values..."
if grep -q "PLACEHOLDER\|0xVALIDATOR_\|0xFOUNDATION_" /etc/zbx/mainnet-genesis.json; then
  warn "Genesis still has PLACEHOLDER validator addresses!"
  warn "Add your real validator addresses to /etc/zbx/mainnet-genesis.json"
  warn "then run: zbx-node init --genesis /etc/zbx/mainnet-genesis.json --config /etc/zbx/mainnet.toml"
  echo ""
  echo -e "${YELLOW}Skipping genesis init until validators are filled in.${NC}"
  SKIP_INIT=true
else
  log "Initializing genesis block..."
  sudo -u "$ZBX_USER" "$ZBX_BIN" init \
    --genesis /etc/zbx/mainnet-genesis.json \
    --config  /etc/zbx/mainnet.toml \
    --data-dir "$ZBX_DATA"
  log "Genesis initialized successfully!"
  SKIP_INIT=false
fi

# ═════════════════════════════════════════════════════════════════════════════
step "STEP 9 — Install systemd Service"
# ═════════════════════════════════════════════════════════════════════════════

log "Installing zbx-mainnet.service..."
cat > /etc/systemd/system/zbx-mainnet.service << SERVICE
[Unit]
Description=Zebvix Chain — Mainnet Node (Chain ID 8989)
Documentation=https://docs.zbx.io
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$ZBX_USER
Group=$ZBX_USER
WorkingDirectory=$ZBX_HOME
ExecStart=$ZBX_BIN \\
    --network mainnet \\
    --config /etc/zbx/mainnet.toml \\
    --genesis /etc/zbx/mainnet-genesis.json \\
    --validator-key $ZBX_KEYS/validator.key \\
    --bls-key $ZBX_KEYS/bls.key \\
    --node-key $ZBX_KEYS/node.key \\
    --bind-addr 0.0.0.0
EnvironmentFile=-/etc/zbx/mainnet.env
Restart=on-failure
RestartSec=5
LimitNOFILE=65536
LimitNPROC=4096
StandardOutput=journal
StandardError=journal
SyslogIdentifier=zbx-mainnet

# Hardening
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=$ZBX_DATA $ZBX_LOG
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
RestrictNamespaces=true
RestrictRealtime=true
LockPersonality=true
MemoryDenyWriteExecute=false
SystemCallArchitectures=native

[Install]
WantedBy=multi-user.target
SERVICE

systemctl daemon-reload
log "Service installed: zbx-mainnet.service"

# ═════════════════════════════════════════════════════════════════════════════
step "STEP 10 — Firewall Rules"
# ═════════════════════════════════════════════════════════════════════════════

log "Configuring UFW firewall..."
ufw --force enable
ufw allow 22/tcp     comment "SSH"
ufw allow 30303/tcp  comment "ZBX P2P"
ufw allow 30303/udp  comment "ZBX P2P UDP"
ufw allow 80/tcp     comment "HTTP (certbot)"
ufw allow 443/tcp    comment "HTTPS RPC"

# RPC only accessible via nginx (not directly)
# 8545 and 8546 are NOT opened publicly — nginx proxies them
log "Firewall configured. RPC only via nginx/TLS (port 8545 blocked externally)"

# ═════════════════════════════════════════════════════════════════════════════
step "STEP 11 — nginx + TLS Setup (optional)"
# ═════════════════════════════════════════════════════════════════════════════

if [[ -n "${RPC_DOMAIN:-}" ]]; then
  log "Setting up nginx for $RPC_DOMAIN..."

  cat > /etc/nginx/sites-available/zbx-rpc.conf << NGINX
limit_req_zone \$binary_remote_addr zone=zbx_rpc:10m rate=20r/s;

server {
    listen 80;
    server_name $RPC_DOMAIN;

    location /.well-known/acme-challenge/ {
        root /var/www/letsencrypt;
    }
    location / {
        return 301 https://\$host\$request_uri;
    }
}

server {
    listen 443 ssl http2;
    listen [::]:443 ssl http2;
    server_name $RPC_DOMAIN;

    ssl_certificate     /etc/letsencrypt/live/$RPC_DOMAIN/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/$RPC_DOMAIN/privkey.pem;
    ssl_protocols       TLSv1.2 TLSv1.3;
    ssl_ciphers         ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384;
    ssl_session_cache   shared:SSL:10m;

    add_header Strict-Transport-Security "max-age=31536000; includeSubDomains" always;
    add_header X-Content-Type-Options "nosniff" always;
    add_header X-Frame-Options "DENY" always;

    client_max_body_size 1m;
    limit_req zone=zbx_rpc burst=60 delay=20;
    limit_req_status 429;

    location / {
        proxy_pass         http://127.0.0.1:8545;
        proxy_http_version 1.1;
        proxy_set_header   Host              \$host;
        proxy_set_header   X-Real-IP         \$remote_addr;
        proxy_set_header   X-Forwarded-For   \$proxy_add_x_forwarded_for;
        proxy_set_header   X-Forwarded-Proto \$scheme;
        proxy_read_timeout 30s;
    }

    location = /healthz {
        access_log off;
        proxy_pass http://127.0.0.1:8545/healthz;
    }
}

server {
    listen 443 ssl http2;
    server_name ws.$RPC_DOMAIN;
    ssl_certificate     /etc/letsencrypt/live/$RPC_DOMAIN/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/$RPC_DOMAIN/privkey.pem;
    ssl_protocols       TLSv1.2 TLSv1.3;
    location / {
        proxy_pass         http://127.0.0.1:8546;
        proxy_http_version 1.1;
        proxy_set_header   Upgrade    \$http_upgrade;
        proxy_set_header   Connection "upgrade";
        proxy_read_timeout 3600s;
    }
}
NGINX

  ln -sf /etc/nginx/sites-available/zbx-rpc.conf \
          /etc/nginx/sites-enabled/zbx-rpc.conf
  nginx -t && systemctl reload nginx

  log "Obtaining Let's Encrypt TLS certificate for $RPC_DOMAIN..."
  certbot certonly --webroot \
    -w /var/www/letsencrypt \
    -d "$RPC_DOMAIN" \
    --non-interactive --agree-tos \
    --email "admin@${RPC_DOMAIN#*.}" || \
  warn "certbot failed — run manually: certbot certonly --webroot -d $RPC_DOMAIN"

  systemctl reload nginx || true
  log "nginx configured with TLS"
else
  warn "No domain provided — skipping nginx/TLS. RPC only on http://VPS_IP:8545"
fi

# ═════════════════════════════════════════════════════════════════════════════
step "STEP 12 — Start Node"
# ═════════════════════════════════════════════════════════════════════════════

if [[ "${SKIP_INIT:-false}" == "false" ]]; then
  log "Enabling and starting zbx-mainnet service..."
  systemctl enable zbx-mainnet
  systemctl start  zbx-mainnet
  sleep 3
  systemctl status zbx-mainnet --no-pager
else
  warn "Node NOT started yet. After filling in validator keys in genesis, run:"
  warn "  zbx-node init --genesis /etc/zbx/mainnet-genesis.json --config /etc/zbx/mainnet.toml"
  warn "  systemctl enable --now zbx-mainnet"
fi

# ═════════════════════════════════════════════════════════════════════════════
echo ""
echo -e "${GREEN}${BOLD}══════════════════════════════════════════════════════${NC}"
echo -e "${GREEN}${BOLD}  DEPLOYMENT COMPLETE!${NC}"
echo -e "${GREEN}${BOLD}══════════════════════════════════════════════════════${NC}"
echo ""
echo -e "  ${BOLD}Useful commands:${NC}"
echo "  journalctl -u zbx-mainnet -f          # Live logs"
echo "  systemctl status zbx-mainnet           # Service status"
echo "  systemctl restart zbx-mainnet          # Restart"
echo "  zbx-node admin status                  # Node status"
echo ""
echo -e "  ${BOLD}Key files:${NC}"
echo "  /etc/zbx/mainnet-genesis.json          # Genesis config"
echo "  /etc/zbx/mainnet.toml                  # Node config"
echo "  $ZBX_KEYS/                      # Validator keys (KEEP SECRET)"
echo "  $ZBX_DATA/                      # Chain data"
echo "  /var/log/zbx/                          # Log files"
echo ""
echo -e "  ${BOLD}RPC Endpoints:${NC}"
if [[ -n "${RPC_DOMAIN:-}" ]]; then
echo "  HTTP RPC:   https://$RPC_DOMAIN"
echo "  WebSocket:  wss://ws.$RPC_DOMAIN"
else
echo "  HTTP RPC:   http://${VPS_IP:-YOUR_VPS_IP}:8545"
echo "  WebSocket:  wss://${VPS_IP:-YOUR_VPS_IP}:8546  (via nginx TLS — enable in nginx conf when v0.3 ships)"
echo "  NOTE: Port 8546 is internal-only. Never expose ws:// directly. Always use wss:// via nginx."
fi
echo ""
echo -e "  ${YELLOW}${BOLD}⚠ BACKUP YOUR KEYS:${NC}"
echo "  scp root@VPS_IP:$ZBX_KEYS/* ./zbx-keys-backup/"
echo ""
