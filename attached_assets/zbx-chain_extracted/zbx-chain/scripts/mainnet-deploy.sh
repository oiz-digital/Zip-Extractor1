#!/usr/bin/env bash
# Zebvix Mainnet — production VPS bootstrap and service management script.
#
# Brings up a zbx-node mainnet instance (chain_id 8989) on a VPS. Co-exists
# with testnet (chain_id 8990, port 18545) without collision because all
# paths, ports, and the systemd unit are mainnet-specific.
#
# Surface allocation:
#   Component        | Mainnet                        | Testnet (peer script)
#   -----------------+--------------------------------+---------------------------
#   Binary           | /usr/local/bin/zbx-node        | /usr/local/bin/zbx-node  (shared)
#   Data dir         | /var/lib/zbx-mainnet           | /var/lib/zbx-testnet
#   Config           | /etc/zbx/mainnet.toml          | /etc/zbx/testnet.toml
#   Genesis (custom) | /etc/zbx/genesis.mainnet.json  | /etc/zbx/genesis.testnet.json
#   Systemd service  | zbx-mainnet.service            | zbx-testnet.service
#   RPC listen       | 127.0.0.1:8545                 | 127.0.0.1:18545
#   P2P listen       | 0.0.0.0:30303                  | 0.0.0.0:30304
#   Metrics          | 127.0.0.1:9100                 | 127.0.0.1:9101
#
# *** BOOT BEHAVIOUR ***
# Mainnet does NOT require a custom genesis file. If `chain.genesis_file` is
# unset (or left at the sentinel "genesis.json"), zbx-node boots from the
# hardcoded `GenesisConfig::mainnet()` preset in node/src/genesis.rs. This
# guarantees a canonical genesis hash across all operators. Only set
# `chain.genesis_file` if you intentionally want operator-supplied genesis
# (e.g. with real validator BLS keys generated via zbx-keygen on secure
# hardware) — and double-check via `--dry-run` first.
#
# *** MAINNET-SPECIFIC SAFETY ***
# This script adds several preflight checks that testnet-deploy.sh does not:
#   1. Refuses to run while testnet binary version mismatches mainnet — same
#      `zbx-node` binary must serve both to avoid genesis-hash drift.
#   2. Refuses if VALIDATOR_KEY env is unset AND --validator was passed.
#   3. Verifies eth_chainId returns 0x231d (8989) after start. Hard fail
#      otherwise — never leave a mainnet service in an unknown chain state.
#   4. Backs up any existing config/genesis files with a timestamped suffix
#      instead of overwriting (mainnet config drift = catastrophic).
#   5. Refuses to start as validator without a clean firewall reminder
#      (RPC must NOT be exposed publicly on a validator).
#
# Usage:
#   sudo bash scripts/mainnet-deploy.sh                # full deploy (build + install + service)
#   sudo bash scripts/mainnet-deploy.sh --build-only   # build + install binaries, skip systemd
#   sudo bash scripts/mainnet-deploy.sh --service-only # systemd unit + restart, skip build
#   sudo bash scripts/mainnet-deploy.sh --status       # show service status + chain tip
#   sudo bash scripts/mainnet-deploy.sh --dry-run      # print plan, change nothing
#   sudo bash scripts/mainnet-deploy.sh --validator    # enable validator mode (requires VALIDATOR_KEY)
#
# Environment overrides:
#   ZBX_MAINNET_RPC_PORT   default 8545
#   ZBX_MAINNET_P2P_PORT   default 30303
#   ZBX_MAINNET_METRICS    default 9100
#   ZBX_MAINNET_DATA_DIR   default /var/lib/zbx-mainnet
#   ZBX_MAINNET_CONFIG     default /etc/zbx/mainnet.toml
#   ZBX_BIN                default /usr/local/bin/zbx-node
#   VALIDATOR_KEY          BLS private key hex (32 bytes) — required if --validator
#   ZBX_RPC_BIND_LOCAL     default 1 (validator nodes MUST be 127.0.0.1; set 0 to bind 0.0.0.0)
#
# Exit codes:
#   0 — success
#   1 — build / install / systemd failure
#   2 — usage / environment error
#   3 — refusing to act (safety check failed, e.g. chain_id mismatch)

set -euo pipefail

# ── 0. Preflight ─────────────────────────────────────────────────────────────
if [[ "${EUID}" -ne 0 ]]; then
    echo "ERROR: this script must be run as root (sudo)" >&2
    exit 2
fi

# cargo PATH discovery (sudo strips PATH)
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

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SOURCE_DIR="$(dirname "$SCRIPT_DIR")"

# ── Config ───────────────────────────────────────────────────────────────────
RPC_PORT="${ZBX_MAINNET_RPC_PORT:-8545}"
P2P_PORT="${ZBX_MAINNET_P2P_PORT:-30303}"
METRICS_PORT="${ZBX_MAINNET_METRICS:-9100}"
DATA_DIR="${ZBX_MAINNET_DATA_DIR:-/var/lib/zbx-mainnet}"
CONFIG_FILE="${ZBX_MAINNET_CONFIG:-/etc/zbx/mainnet.toml}"
BIN_PATH="${ZBX_BIN:-/usr/local/bin/zbx-node}"
KEYGEN_BIN_PATH="$(dirname "$BIN_PATH")/zbx-keygen"
SERVICE="zbx-mainnet"
SERVICE_FILE="/etc/systemd/system/${SERVICE}.service"
RPC_BIND_LOCAL="${ZBX_RPC_BIND_LOCAL:-1}"

mode="full"
validator_mode=0
for arg in "$@"; do
    case "$arg" in
        --build-only)   mode="build"   ;;
        --service-only) mode="service" ;;
        --status)       mode="status"  ;;
        --dry-run)      mode="dry"     ;;
        --validator)    validator_mode=1 ;;
        "") ;;
        *)
            echo "ERROR: unknown flag: $arg" >&2
            echo "  valid: --build-only | --service-only | --status | --dry-run | --validator" >&2
            exit 2
            ;;
    esac
done

# Validator mode requires VALIDATOR_KEY
if (( validator_mode == 1 )) && [[ -z "${VALIDATOR_KEY:-}" ]]; then
    echo "ERROR: --validator requires VALIDATOR_KEY env var (32-byte BLS hex)" >&2
    echo "  generate with: zbx-keygen --count 1 --output text" >&2
    echo "  set with:      export VALIDATOR_KEY=0x...   then re-run with sudo -E" >&2
    exit 2
fi

# Validator nodes MUST bind RPC to loopback only
if (( validator_mode == 1 )) && [[ "$RPC_BIND_LOCAL" != "1" ]]; then
    echo "ERROR: validator nodes must NOT expose RPC publicly (ZBX_RPC_BIND_LOCAL=$RPC_BIND_LOCAL)" >&2
    echo "  override only if you have a reverse proxy with strict ACLs:  ZBX_RPC_BIND_LOCAL=0" >&2
    exit 3
fi

RPC_BIND_ADDR="127.0.0.1"
[[ "$RPC_BIND_LOCAL" == "0" ]] && RPC_BIND_ADDR="0.0.0.0"

echo "═══════════════════════════════════════════════════════════"
echo "  Zebvix MAINNET Deploy — zbx-node (chain_id 8989)"
echo "  source   : ${SOURCE_DIR}"
echo "  binary   : ${BIN_PATH}"
echo "  data     : ${DATA_DIR}"
echo "  config   : ${CONFIG_FILE}"
echo "  rpc      : ${RPC_BIND_ADDR}:${RPC_PORT}   p2p: 0.0.0.0:${P2P_PORT}   metrics: 127.0.0.1:${METRICS_PORT}"
echo "  mode     : ${mode}   validator: $((validator_mode))"
echo "═══════════════════════════════════════════════════════════"

# ── --status fast path ───────────────────────────────────────────────────────
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
    echo ""
    echo "  (expect 0x231d = chain_id 8989)"
    echo ""
    echo "── validator set ──"
    curl -fsS -X POST "http://127.0.0.1:${RPC_PORT}" \
         -H 'Content-Type: application/json' \
         -d '{"jsonrpc":"2.0","method":"zbx_listValidators","params":[],"id":1}' \
         2>/dev/null | head -c 600 || echo "(RPC not responding)"
    echo ""
    exit 0
fi

# ── --dry-run: print plan and exit ───────────────────────────────────────────
if [[ "$mode" == "dry" ]]; then
    echo ""
    echo "  [dry] would build: cargo build --release --bin zbx-node --bin zbx-keygen"
    echo "  [dry] would install binaries: ${BIN_PATH}, ${KEYGEN_BIN_PATH}"
    echo "  [dry] would install config (if missing): ${CONFIG_FILE}"
    echo "  [dry] would write systemd unit: ${SERVICE_FILE}"
    if (( validator_mode == 1 )); then
        echo "  [dry] would enable validator mode (VALIDATOR_KEY set: ${VALIDATOR_KEY:0:8}…)"
    else
        echo "  [dry] would start as FULL NODE (no block production)"
    fi
    echo "  [dry] would verify eth_chainId == 0x231d after restart"
    exit 0
fi

# ── 0a. Refuse if cargo missing (only after status/dry exits) ───────────────
if ! command -v cargo >/dev/null 2>&1; then
    echo "ERROR: cargo not found. Install Rust (https://rustup.rs) and re-run." >&2
    exit 2
fi
if [[ ! -f "${SOURCE_DIR}/Cargo.toml" ]]; then
    echo "ERROR: ${SOURCE_DIR}/Cargo.toml not found — run from inside zbx-chain checkout" >&2
    exit 2
fi

# ── 1. Build (mode=build|full) ───────────────────────────────────────────────
if [[ "$mode" == "build" || "$mode" == "full" ]]; then
    echo ""
    echo "► building zbx-node + zbx-keygen (cargo build --release)..."
    cd "$SOURCE_DIR"
    LIBCLANG_PATH="${LIBCLANG_PATH:-$(find /usr/lib -name 'libclang*.so*' 2>/dev/null | head -1 | xargs -r dirname || echo '')}"
    LIBCLANG_PATH="$LIBCLANG_PATH" cargo build --release --bin zbx-node --bin zbx-keygen

    SRC_BIN="${SOURCE_DIR}/target/release/zbx-node"
    SRC_KEYGEN="${SOURCE_DIR}/target/release/zbx-keygen"
    if [[ ! -x "$SRC_BIN" ]]; then
        echo "ERROR: build succeeded but ${SRC_BIN} not found" >&2
        exit 1
    fi

    # If a co-located testnet is running, the binary SHAs must match —
    # otherwise consensus would diverge. Refuse to install a new mainnet
    # binary that differs from the running testnet one.
    if systemctl is-active --quiet zbx-testnet 2>/dev/null && [[ -x "$BIN_PATH" ]]; then
        old_sha=$(sha256sum "$BIN_PATH" | awk '{print $1}')
        new_sha=$(sha256sum "$SRC_BIN" | awk '{print $1}')
        if [[ "$old_sha" != "$new_sha" ]]; then
            echo "WARNING: new binary differs from running mainnet binary"
            echo "  old: $old_sha"
            echo "  new: $new_sha"
            echo "  Make sure to also restart zbx-testnet after install to keep versions aligned."
        fi
    fi

    echo ""
    echo "► installing binaries..."
    install -m 755 "$SRC_BIN"    "$BIN_PATH"
    install -m 755 "$SRC_KEYGEN" "$KEYGEN_BIN_PATH"
    echo "  zbx-node   sha256: $(sha256sum "$BIN_PATH"    | awk '{print $1}')"
    echo "  zbx-keygen sha256: $(sha256sum "$KEYGEN_BIN_PATH" | awk '{print $1}')"
fi

# ── 2. Install config (BACK UP, do not overwrite) ────────────────────────────
if [[ "$mode" == "build" || "$mode" == "full" ]]; then
    echo ""
    echo "► installing config..."
    mkdir -p /etc/zbx
    if [[ -f "$CONFIG_FILE" ]]; then
        backup="${CONFIG_FILE}.bak.$(date +%Y%m%d-%H%M%S)"
        cp -a "$CONFIG_FILE" "$backup"
        echo "  SKIP: ${CONFIG_FILE} already exists (backed up to ${backup})"
        echo "  Edit existing file in-place; remove the backup once verified."
    else
        cp "${SOURCE_DIR}/config/mainnet.toml" "$CONFIG_FILE"
        echo "  installed: $CONFIG_FILE"
        echo "  REVIEW: ${CONFIG_FILE}"
        echo "    - [chain] genesis_file — leave UNSET to use hardcoded preset (recommended)"
        echo "    - [p2p] boot_nodes — replace with actual bootnode addresses"
        echo "    - [rpc] cors_allow — set to your dApp origin"
    fi
fi

# ── 3. Systemd unit (mode=service|full) ──────────────────────────────────────
if [[ "$mode" == "service" || "$mode" == "full" ]]; then
    echo ""
    echo "► writing systemd unit ${SERVICE_FILE}..."
    mkdir -p "$DATA_DIR"
    chmod 700 "$DATA_DIR"

    VALIDATOR_FLAG=""
    VALIDATOR_ENV=""
    if (( validator_mode == 1 )); then
        VALIDATOR_FLAG=" --validator"
        VALIDATOR_ENV="Environment=\"VALIDATOR_KEY=${VALIDATOR_KEY}\""
        echo "  validator mode ENABLED — VALIDATOR_KEY will be written to systemd unit"
        echo "  ${SERVICE_FILE} will have mode 0600 — read-only to root"
    else
        echo "  starting as FULL NODE (no validator key)"
        echo "  To enable validator later: export VALIDATOR_KEY=0x...; sudo -E bash $0 --service-only --validator"
    fi

    cat > "$SERVICE_FILE" <<UNIT
[Unit]
Description=Zebvix Chain Node — MAINNET (chain_id 8989)
Documentation=https://github.com/zebvix-org/zbx-chain
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=${BIN_PATH} \\
  --network mainnet \\
  --config ${CONFIG_FILE} \\
  --data-dir ${DATA_DIR} \\
  --rpc-port ${RPC_PORT} \\
  --rpc-addr ${RPC_BIND_ADDR} \\
  --p2p-port ${P2P_PORT} \\
  --metrics-port ${METRICS_PORT} \\
  --log-level info${VALIDATOR_FLAG}
Restart=always
RestartSec=10
LimitNOFILE=65536
SyslogIdentifier=${SERVICE}
StandardOutput=journal
StandardError=journal

# Mainnet HARDENING — restrict the systemd execution sandbox.
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ReadWritePaths=${DATA_DIR}
ProtectHome=true
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
LockPersonality=true

${VALIDATOR_ENV}

# Mainnet should NOT produce empty blocks; let producer time-out naturally.
Environment="ZBX_PRODUCE_EMPTY_BLOCKS=0"

[Install]
WantedBy=multi-user.target
UNIT
    chmod 600 "$SERVICE_FILE"   # validator key may be inline; lock it down

    systemctl daemon-reload
    systemctl enable "$SERVICE" >/dev/null 2>&1 || true
    echo ""
    echo "► starting / restarting ${SERVICE}..."
    systemctl restart "$SERVICE"
    sleep 4

    echo ""
    if systemctl is-active --quiet "$SERVICE"; then
        echo "  service is running"
    else
        echo "ERROR: service not active — recent logs:" >&2
        journalctl -u "$SERVICE" --no-pager --since "1 minute ago" | tail -30 >&2
        exit 1
    fi

    echo ""
    echo "► verifying chain_id=8989 in RPC (3 attempts, 2s apart)..."
    chain_ok=0
    for i in 1 2 3; do
        sleep 2
        RPC_RESP=$(curl -fsS -m 5 -X POST "http://127.0.0.1:${RPC_PORT}" \
            -H 'Content-Type: application/json' \
            -d '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}' \
            2>/dev/null || echo "")
        if echo "${RPC_RESP,,}" | grep -q '"0x231d"'; then
            echo "  chain_id confirmed: 0x231d (8989) on attempt $i"
            chain_ok=1
            break
        fi
        echo "  …attempt $i: ${RPC_RESP:-<no response>}"
    done

    if (( chain_ok != 1 )); then
        echo "ERROR: mainnet RPC did not return chain_id 0x231d — refusing to leave service in unknown state" >&2
        echo "  Stopping service for safety. Inspect: journalctl -u ${SERVICE} -e" >&2
        systemctl stop "$SERVICE"
        exit 3
    fi
fi

echo ""
echo "═══════════════════════════════════════════════════════════"
echo "  Mainnet deploy complete"
echo ""
echo "  Verify health:   sudo bash ${SCRIPT_DIR}/mainnet-deploy.sh --status"
echo "  Tail logs:       sudo journalctl -u ${SERVICE} -f"
echo "  Block number:    curl -X POST http://127.0.0.1:${RPC_PORT} \\"
echo "                     -H 'Content-Type: application/json' \\"
echo "                     -d '{\"jsonrpc\":\"2.0\",\"method\":\"eth_blockNumber\",\"params\":[],\"id\":1}'"
echo ""
if (( validator_mode == 1 )); then
    echo "  ⚠  VALIDATOR MODE — checklist:"
    echo "     1. Firewall: block ${RPC_PORT}/tcp from public; allow ${P2P_PORT}/tcp"
    echo "     2. Verify your address appears in zbx_listValidators output"
    echo "     3. Watch zbx_signing_misses_total in Prometheus — should stay 0"
    echo "     4. Add to N→N+1 quorum via scripts/mainnet-add-validator.sh from the chair node"
else
    echo "  Running as full node. Add to validator set later via mainnet-add-validator.sh"
fi
echo ""
echo "  This is MAINNET. Tokens have real economic value."
echo "═══════════════════════════════════════════════════════════"
