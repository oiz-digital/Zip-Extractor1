#!/usr/bin/env bash
# zbx-chain VPS deployment script
# Usage: ./deploy.sh <vps-host> <network>
# Example: ./deploy.sh 93.127.213.192 mainnet
#
# Requires: ssh key access as `root` to the VPS, a previously built
# `target/release/zbx-node` binary in the workspace.

set -euo pipefail

VPS_HOST="${1:-93.127.213.192}"
NETWORK="${2:-mainnet}"
SSH_USER="${SSH_USER:-root}"

case "$NETWORK" in
    mainnet|testnet) ;;
    *) echo "ERROR: network must be 'mainnet' or 'testnet'" >&2; exit 1 ;;
esac

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="$REPO_ROOT/target/release/zbx-node"
KEYGEN_BIN="$REPO_ROOT/target/release/zbx-keygen"

if [[ ! -x "$BIN" ]]; then
    echo "ERROR: $BIN not found. Run: cargo build --release -p zbx-node" >&2
    exit 1
fi

echo "==> Deploying zbx-node ($NETWORK) to ${SSH_USER}@${VPS_HOST}"

# 1. Provision OS user, dirs, firewall
ssh "${SSH_USER}@${VPS_HOST}" bash <<'REMOTE'
set -euo pipefail
id -u zbx >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin zbx
install -d -o zbx -g zbx -m 0750 /var/lib/zbx-mainnet /var/lib/zbx-testnet /var/log/zbx
install -d -m 0755 /etc/zbx /opt/zbx
which ufw >/dev/null 2>&1 && {
    ufw allow 22/tcp || true
    ufw allow 80/tcp || true
    ufw allow 443/tcp || true
    ufw allow 30303/tcp || true   # mainnet p2p
    ufw allow 30304/tcp || true   # testnet p2p
}
REMOTE

# 2. Copy binary + configs
scp "$BIN"                                  "${SSH_USER}@${VPS_HOST}:/usr/local/bin/zbx-node.new"
[[ -x "$KEYGEN_BIN" ]] && scp "$KEYGEN_BIN" "${SSH_USER}@${VPS_HOST}:/usr/local/bin/zbx-keygen.new"
scp "$REPO_ROOT/zbx-chain/node/configs/${NETWORK}.toml" \
    "${SSH_USER}@${VPS_HOST}:/etc/zbx/${NETWORK}.toml"
scp "$REPO_ROOT/zbx-chain/deploy/systemd/zbx-${NETWORK}.service" \
    "${SSH_USER}@${VPS_HOST}:/etc/systemd/system/zbx-${NETWORK}.service"

# 3. Atomic swap + restart
ssh "${SSH_USER}@${VPS_HOST}" bash <<REMOTE
set -euo pipefail
chmod +x /usr/local/bin/zbx-node.new
mv /usr/local/bin/zbx-node.new /usr/local/bin/zbx-node
[[ -f /usr/local/bin/zbx-keygen.new ]] && {
    chmod +x /usr/local/bin/zbx-keygen.new
    mv /usr/local/bin/zbx-keygen.new /usr/local/bin/zbx-keygen
}
chown zbx:zbx /etc/zbx/${NETWORK}.toml
chmod 0640    /etc/zbx/${NETWORK}.toml
systemctl daemon-reload
systemctl enable zbx-${NETWORK}.service
systemctl restart zbx-${NETWORK}.service
sleep 3
systemctl --no-pager status zbx-${NETWORK}.service | head -20
REMOTE

echo "==> Deployment complete."
echo "    Tail logs : ssh ${SSH_USER}@${VPS_HOST} journalctl -u zbx-${NETWORK} -f"
echo "    Health    : curl -s http://${VPS_HOST}:$([[ $NETWORK == mainnet ]] && echo 8545 || echo 18545)/healthz"
