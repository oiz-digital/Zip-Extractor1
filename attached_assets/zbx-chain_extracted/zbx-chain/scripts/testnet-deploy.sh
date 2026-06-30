#!/usr/bin/env bash
# Zebvix Testnet — VPS bootstrap and service management script.
#
# Brings up a zbx-node testnet instance (chain_id 8990) on the same VPS as
# mainnet (chain_id 8989). The two processes are fully isolated:
#   different data dirs, RPC ports, P2P ports, and systemd service units.
#
# Surface allocation (avoids ALL collisions with mainnet on the same VPS):
#   Component        | Mainnet                        | Testnet
#   -----------------+--------------------------------+---------------------------
#   Binary           | /usr/local/bin/zbx-node        | /usr/local/bin/zbx-node  (shared)
#   Data dir         | /var/lib/zbx-mainnet           | /var/lib/zbx-testnet
#   Config           | /etc/zbx/mainnet.toml          | /etc/zbx/testnet.toml
#   Genesis          | /etc/zbx/genesis.mainnet.json  | /etc/zbx/genesis.testnet.json
#   Systemd service  | zbx-mainnet.service            | zbx-testnet.service
#   RPC listen       | 127.0.0.1:8545                 | 127.0.0.1:18545
#   P2P listen       | 0.0.0.0:30303                  | 0.0.0.0:30304
#   Metrics          | localhost:9000                  | localhost:9001
#
# Usage:
#   sudo bash scripts/testnet-deploy.sh               # full deploy (build + install + service)
#   sudo bash scripts/testnet-deploy.sh --build-only  # build + install binary, skip systemd
#   sudo bash scripts/testnet-deploy.sh --service-only# write systemd unit + restart, skip build
#   sudo bash scripts/testnet-deploy.sh --status      # show service status + tip
#
# Environment overrides:
#   ZBX_TESTNET_RPC_PORT   default 18545
#   ZBX_TESTNET_P2P_PORT   default 30304
#   ZBX_TESTNET_DATA_DIR   default /var/lib/zbx-testnet
#   ZBX_TESTNET_CONFIG     default /etc/zbx/testnet.toml
#   ZBX_TESTNET_GENESIS    default /etc/zbx/genesis.testnet.json
#   ZBX_BIN               default /usr/local/bin/zbx-node
#   VALIDATOR_KEY          BLS private key hex (32 bytes) — required for validator mode
#
# Exit codes:
#   0 — success
#   1 — build / install / systemd failure
#   2 — usage / environment error

set -euo pipefail

# ── 0. Preflight ─────────────────────────────────────────────────────────────
if [[ "${EUID}" -ne 0 ]]; then
    echo "ERROR: this script must be run as root (sudo)" >&2
    exit 2
fi

# cargo PATH discovery (sudo strips PATH — find cargo in known locations)
if ! command -v cargo >/dev/null 2>&1; then
    _cargo_candidates=()
    [[ -n "${CARGO:-}"     ]] && _cargo_candidates+=("$(dirname "$CARGO")")
    [[ -n "${SUDO_USER:-}" ]] && _cargo_candidates+=("/home/${SUDO_USER}/.cargo/bin")
    [[ -n "${HOME:-}"      ]] && _cargo_candidates+=("${HOME}/.cargo/bin")
    _cargo_candidates+=( "/root/.cargo/bin" "/usr/local/cargo/bin" "/usr/local/bin" )
    for _d in "${_cargo_candidates[@]}"; do
        if [[ -x "${_d}/cargo" ]]; then
            export PATH="${_d}:${PATH}"
            echo "  (cargo found at ${_d}/cargo)"
            break
        fi
    done
    unset _cargo_candidates _d
fi
if ! command -v cargo >/dev/null 2>&1; then
    echo "ERROR: cargo not found. Install Rust (https://rustup.rs) and re-run." >&2
    exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SOURCE_DIR="$(dirname "$SCRIPT_DIR")"
if [[ ! -f "${SOURCE_DIR}/Cargo.toml" ]]; then
    echo "ERROR: ${SOURCE_DIR}/Cargo.toml not found — run from inside zbx-chain checkout" >&2
    exit 2
fi

# ── Config ───────────────────────────────────────────────────────────────────
RPC_PORT="${ZBX_TESTNET_RPC_PORT:-18545}"
P2P_PORT="${ZBX_TESTNET_P2P_PORT:-30304}"
DATA_DIR="${ZBX_TESTNET_DATA_DIR:-/var/lib/zbx-testnet}"
CONFIG_FILE="${ZBX_TESTNET_CONFIG:-/etc/zbx/testnet.toml}"
GENESIS_FILE="${ZBX_TESTNET_GENESIS:-/etc/zbx/genesis.testnet.json}"
BIN_PATH="${ZBX_BIN:-/usr/local/bin/zbx-node}"
KEYGEN_BIN_PATH="${ZBX_BIN:-/usr/local/bin}/zbx-keygen"
SERVICE="zbx-testnet"
SERVICE_FILE="/etc/systemd/system/${SERVICE}.service"

mode="full"
case "${1:-}" in
    --build-only)   mode="build"   ;;
    --service-only) mode="service" ;;
    --status)       mode="status"  ;;
    "")             mode="full"    ;;
    *)
        echo "ERROR: unknown flag: $1" >&2
        echo "  valid: --build-only | --service-only | --status | (no flag = full)" >&2
        exit 2
        ;;
esac

echo "═══════════════════════════════════════════════════════════"
echo "  Zebvix Testnet Deploy — zbx-node v0.2.0"
echo "  source  : ${SOURCE_DIR}"
echo "  binary  : ${BIN_PATH}"
echo "  data    : ${DATA_DIR}"
echo "  config  : ${CONFIG_FILE}"
echo "  genesis : ${GENESIS_FILE}"
echo "  rpc     : 127.0.0.1:${RPC_PORT}  p2p: 0.0.0.0:${P2P_PORT}"
echo "  mode    : ${mode}"
echo "═══════════════════════════════════════════════════════════"

# ── --status fast path ────────────────────────────────────────────────────────
if [[ "$mode" == "status" ]]; then
    echo ""
    systemctl status "$SERVICE" --no-pager | head -12 || true
    echo ""
    echo "── recent journal (last 30s) ──"
    journalctl -u "$SERVICE" --since "30 seconds ago" --no-pager | tail -20 || true
    echo ""
    echo "── chain tip via RPC ──"
    curl -fsS -X POST "http://127.0.0.1:${RPC_PORT}" \
         -H 'Content-Type: application/json' \
         -d '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}' \
         2>/dev/null | head -1 || echo "(RPC not responding)"
    echo "  (expect 0x231e = chain_id 8990)"
    exit 0
fi

# ── 1. Build (mode=build|full) ────────────────────────────────────────────────
if [[ "$mode" == "build" || "$mode" == "full" ]]; then
    echo ""
    echo "► building zbx-node + zbx-keygen (cargo build --release)..."
    cd "$SOURCE_DIR"
    LIBCLANG_PATH="${LIBCLANG_PATH:-$(find /usr/lib -name 'libclang*.so*' 2>/dev/null | head -1 | xargs dirname || echo '')}"
    LIBCLANG_PATH="$LIBCLANG_PATH" cargo build --release --bin zbx-node --bin zbx-keygen

    SRC_BIN="${SOURCE_DIR}/target/release/zbx-node"
    SRC_KEYGEN="${SOURCE_DIR}/target/release/zbx-keygen"
    if [[ ! -x "$SRC_BIN" ]]; then
        echo "ERROR: build succeeded but ${SRC_BIN} not found" >&2
        exit 1
    fi

    echo ""
    echo "► installing binaries..."
    install -m 755 "$SRC_BIN"    "$BIN_PATH"
    install -m 755 "$SRC_KEYGEN" "$KEYGEN_BIN_PATH"
    echo "  zbx-node   sha256: $(sha256sum "$BIN_PATH"    | awk '{print $1}')"
    echo "  zbx-keygen sha256: $(sha256sum "$KEYGEN_BIN_PATH" | awk '{print $1}')"
fi

# ── 2. Install config + genesis if not already present ───────────────────────
if [[ "$mode" == "build" || "$mode" == "full" ]]; then
    echo ""
    echo "► installing config + genesis files..."
    mkdir -p /etc/zbx
    if [[ ! -f "$CONFIG_FILE" ]]; then
        cp "${SOURCE_DIR}/node/configs/testnet.toml" "$CONFIG_FILE"
        echo "  installed: $CONFIG_FILE"
        echo "  IMPORTANT: edit $CONFIG_FILE and set:"
        echo "    [chain] is_validator = true  (for validator nodes)"
        echo "    [chain] extra_validators — other validators' BLS pubkeys"
        echo "    [network] bootnodes — actual bootnode addresses"
    else
        echo "  SKIP: $CONFIG_FILE already exists (not overwriting)"
    fi
    if [[ ! -f "$GENESIS_FILE" ]]; then
        cp "${SOURCE_DIR}/config/testnet-genesis.json" "$GENESIS_FILE"
        echo "  installed: $GENESIS_FILE"
        echo "  IMPORTANT: update $GENESIS_FILE with real validator addresses"
        echo "  (use zbx-keygen --count N --output text to generate validator keypairs)"
    else
        echo "  SKIP: $GENESIS_FILE already exists (not overwriting)"
    fi
fi

# ── 3. Systemd unit (mode=service|full) ──────────────────────────────────────
if [[ "$mode" == "service" || "$mode" == "full" ]]; then
    echo ""
    echo "► writing systemd unit ${SERVICE_FILE}..."
    mkdir -p "$DATA_DIR"

    # Determine validator flag
    VALIDATOR_FLAG=""
    if [[ -n "${VALIDATOR_KEY:-}" ]]; then
        VALIDATOR_FLAG=" --validator"
        echo "  VALIDATOR_KEY set — enabling validator mode"
    else
        echo "  NOTE: VALIDATOR_KEY not set — starting as full node (no block production)"
        echo "        To enable validator: set VALIDATOR_KEY env var in systemd unit"
    fi

    cat > "$SERVICE_FILE" <<UNIT
[Unit]
Description=Zebvix Chain Node — TESTNET (chain_id 8990)
Documentation=https://github.com/zebvix-org/zbx-chain
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=${BIN_PATH} \\
  --network testnet \\
  --config ${CONFIG_FILE} \\
  --data-dir ${DATA_DIR} \\
  --rpc-port ${RPC_PORT} \\
  --p2p-port ${P2P_PORT} \\
  --log-level info${VALIDATOR_FLAG}
Restart=always
RestartSec=5
LimitNOFILE=65536
SyslogIdentifier=${SERVICE}
StandardOutput=journal
StandardError=journal

# VALIDATOR_KEY — BLS private key (32 bytes hex, 0x prefix optional)
# Set this for validator mode: generate with `zbx-keygen --count 1 --output text`
Environment="VALIDATOR_KEY=${VALIDATOR_KEY:-}"

# Produce empty blocks on testnet so block time stays predictable
Environment="ZBX_PRODUCE_EMPTY_BLOCKS=1"

[Install]
WantedBy=multi-user.target
UNIT

    systemctl daemon-reload
    systemctl enable "$SERVICE" >/dev/null 2>&1 || true
    echo ""
    echo "► starting / restarting ${SERVICE}..."
    systemctl restart "$SERVICE"
    sleep 3

    echo ""
    if systemctl is-active --quiet "$SERVICE"; then
        echo "  service is running"
    else
        echo "  WARNING: service not active — check: journalctl -u ${SERVICE} -e"
    fi
    systemctl status "$SERVICE" --no-pager | head -8 || true

    echo ""
    echo "► verifying chain_id=8990 in RPC..."
    sleep 2
    RPC_RESP=$(curl -fsS -X POST "http://127.0.0.1:${RPC_PORT}" \
        -H 'Content-Type: application/json' \
        -d '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}' \
        2>/dev/null || echo "")
    if echo "${RPC_RESP,,}" | grep -q "0x231e"; then
        echo "  chain_id confirmed: 0x231e (8990)"
    else
        echo "  WARNING: RPC not responding or unexpected chain_id — check logs"
        echo "  Response: ${RPC_RESP:-<empty>}"
    fi
fi

echo ""
echo "═══════════════════════════════════════════════════════════"
echo "  Testnet deploy complete"
echo ""
echo "  Verify health:   sudo bash ${SCRIPT_DIR}/testnet-deploy.sh --status"
echo "  Tail logs:       sudo journalctl -u ${SERVICE} -f"
echo "  Block number:    curl -X POST http://127.0.0.1:${RPC_PORT} \\"
echo "                     -H 'Content-Type: application/json' \\"
echo "                     -d '{\"jsonrpc\":\"2.0\",\"method\":\"eth_blockNumber\",\"params\":[],\"id\":1}'"
echo "  Validator set:   curl -X POST http://127.0.0.1:${RPC_PORT} \\"
echo "                     -H 'Content-Type: application/json' \\"
echo "                     -d '{\"jsonrpc\":\"2.0\",\"method\":\"zbx_getValidatorSet\",\"params\":[],\"id\":1}'"
echo ""
echo "  TESTNET TOKENS HAVE ZERO ECONOMIC VALUE."
echo "  Mainnet (port 8545, chain_id=8989) is UNTOUCHED."
echo "═══════════════════════════════════════════════════════════"
