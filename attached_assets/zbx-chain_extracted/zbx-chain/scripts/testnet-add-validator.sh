#!/usr/bin/env bash
# Phase E.1 — Add a 2nd validator to the live testnet (1→2 BFT quorum).
#
# WHY THIS SCRIPT EXISTS
#   "Just submit a validator-add tx" naively halts the chain: total voting
#   power increases but the new validator produces no votes, so >2/3 quorum
#   becomes unreachable. This script enforces a SAFE order proven by reading
#   `consensus.rs::Producer::run` — the producer re-reads `state.validators()`
#   on EVERY tick (line ~200), so a node whose address is NOT yet in the set
#   sits idle automatically; the moment validator-add is applied, the next
#   tick picks it up and the node starts proposing/voting WITHOUT a restart.
#
#   Therefore the safe sequence is:
#     1. Bring up node-2 with its own key, BUT not yet a validator on-chain.
#        Its producer task runs but `who_proposes(...) != me`, so it never
#        proposes. It still receives & applies blocks via P2P (full sync).
#     2. Wait for node-2 to catch up to node-1's tip.
#     3. ONLY THEN submit the validator-add tx via the founder key.
#     4. Both nodes' producers now see the updated set on next tick →
#        quorum jumps from 1/1 → 2/2 atomically across the network. No
#        restart, no halt window.
#
# TOPOLOGY AFTER THIS SCRIPT (single-VPS, two co-located testnet nodes):
#
#   ┌──────────────────────────┬──────────────────────────┐
#   │  Node-1 (genesis founder)│  Node-2 (this script)    │
#   ├──────────────────────────┼──────────────────────────┤
#   │  binary: zebvix-node-testnet  (SAME BINARY)         │
#   │  service: zebvix-testnet │  service: zebvix-testnet2│
#   │  home: /root/.zebvix-testnet                        │
#   │                          │  /root/.zebvix-testnet-node2
#   │  RPC:   18545            │  RPC:   18546            │
#   │  P2P:   31333            │  P2P:   31334            │
#   │  power: 1   (founder)    │  power: 1   (added)      │
#   │  key:   .../validator.key│  .../validator-keys/<n>.key
#   └──────────────────────────┴──────────────────────────┘
#
#   Mainnet (chain_id 8989, port 8545, P2P 30333) is COMPLETELY untouched —
#   this script only writes to /root/.zebvix-testnet-node2/ and the testnet
#   RPC at 18545.
#
# Usage:
#   sudo bash scripts/testnet-add-validator.sh                # full safe flow
#   sudo bash scripts/testnet-add-validator.sh --status       # show node-2 + validator-set
#   sudo bash scripts/testnet-add-validator.sh --keygen-only  # stop after step 2
#   sudo bash scripts/testnet-add-validator.sh --dry-run      # print plan, change nothing
#
# Environment overrides (rarely needed):
#   ZBX_TN2_NAME       default node-2
#   ZBX_TN2_RPC_PORT   default 18546
#   ZBX_TN2_P2P_PORT   default 31334
#   ZBX_TN2_HOME       default /root/.zebvix-testnet-node2
#   ZBX_TN1_HOME       default /root/.zebvix-testnet
#   ZBX_TN1_RPC_PORT   default 18545
#   ZBX_TN1_P2P_PORT   default 31333
#   ZBX_TN_BIN         default /usr/local/bin/zebvix-node-testnet
#   ZBX_FOUNDER_KEY    default \${ZBX_TN1_HOME}/validator.key
#   ZBX_SYNC_TIMEOUT   default 120  (seconds to wait for node-2 to catch up)
#   ZBX_TX_TIMEOUT     default  60  (seconds to wait for validator-add tx mining)
#
# Exit codes:
#   0 — success (validator set is now {founder, node-2}, both voting)
#   1 — runtime failure (node didn't sync, tx didn't mine, etc.)
#   2 — usage / preflight failure (wrong env, missing binary, etc.)
#   3 — refusing to act (a different validator is already added, etc.)

set -euo pipefail

# ── 0. Preflight ──────────────────────────────────────────────────────────
if [[ "\${EUID}" -ne 0 ]]; then
    echo "❌ this script must be run as root (sudo)" >&2
    exit 2
fi

NAME="\${ZBX_TN2_NAME:-node-2}"
TN2_RPC_PORT="\${ZBX_TN2_RPC_PORT:-18546}"
TN2_P2P_PORT="\${ZBX_TN2_P2P_PORT:-31334}"
TN2_HOME="\${ZBX_TN2_HOME:-/root/.zebvix-testnet-node2}"
TN1_HOME="\${ZBX_TN1_HOME:-/root/.zebvix-testnet}"
TN1_RPC_PORT="\${ZBX_TN1_RPC_PORT:-18545}"
TN1_P2P_PORT="\${ZBX_TN1_P2P_PORT:-31333}"
BIN="\${ZBX_TN_BIN:-/usr/local/bin/zebvix-node-testnet}"
FOUNDER_KEY="\${ZBX_FOUNDER_KEY:-\${TN1_HOME}/validator.key}"
SYNC_TIMEOUT="\${ZBX_SYNC_TIMEOUT:-120}"
TX_TIMEOUT="\${ZBX_TX_TIMEOUT:-60}"
TN1_SERVICE="zebvix-testnet"
TN2_SERVICE="zebvix-testnet2"
TN2_SERVICE_FILE="/etc/systemd/system/\${TN2_SERVICE}.service"
KEY_DIR="\${TN1_HOME}/validator-keys"
NEW_KEY="\${KEY_DIR}/\${NAME}.key"

mode="full"
case "\${1:-}" in
    --status)        mode="status"  ;;
    --keygen-only)   mode="keygen"  ;;
    --dry-run)       mode="dry"     ;;
    "")              mode="full"    ;;
    *)
        echo "❌ unknown flag: $1" >&2
        echo "    valid: --status | --keygen-only | --dry-run | (no flag = full)" >&2
        exit 2
        ;;
esac

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🧪  Zebvix TESTNET — add validator (1→2 BFT quorum)"
echo "    binary       : \${BIN}"
echo "    node-1 home  : \${TN1_HOME}        rpc:\${TN1_RPC_PORT}  p2p:\${TN1_P2P_PORT}"
echo "    node-2 home  : \${TN2_HOME}  rpc:\${TN2_RPC_PORT}  p2p:\${TN2_P2P_PORT}"
echo "    new key      : \${NEW_KEY}  (name: \${NAME})"
echo "    founder key  : \${FOUNDER_KEY}"
echo "    mode         : \${mode}"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# ── helpers ───────────────────────────────────────────────────────────────
rpc_call() {
    local port="$1" method="$2" params="\${3:-[]}"
    curl -fsS -m 10 -X POST "http://127.0.0.1:\${port}" \\
        -H 'Content-Type: application/json' \\
        -d "{\\"jsonrpc\\":\\"2.0\\",\\"id\\":1,\\"method\\":\\"\${method}\\",\\"params\\":\${params}}"
}

get_height() {
    # Returns decimal block height, or "" on error.
    local port="$1"
    local hex
    hex=$(rpc_call "$port" "eth_blockNumber" 2>/dev/null \\
          | sed -n 's/.*"result":"\\(0x[0-9a-fA-F]*\\)".*/\\1/p')
    if [[ -n "$hex" ]]; then printf "%d\\n" "$hex"; fi
}

get_validator_count() {
    # zbx_listValidators returns: {"result":{"count":N,"validators":[…]}}
    # Use the explicit `count` field rather than counting "address" matches
    # (which would also match the per-validator address sub-fields).
    local port="$1"
    rpc_call "$port" "zbx_listValidators" 2>/dev/null \\
        | grep -oE '"count"[[:space:]]*:[[:space:]]*[0-9]+' \\
        | head -1 \\
        | grep -oE '[0-9]+'
}

# Returns 0 (truthy) iff THIS specific pubkey-hex is in the validator set.
# Matches the precise JSON field `"pubkey":"0x<hex>"` so we never confuse
# a pubkey-hex-substring with an address-hex-substring.
validator_set_has_pubkey() {
    local port="$1" pk_hex_lower="\${2,,}"
    rpc_call "$port" "zbx_listValidators" 2>/dev/null \\
        | tr 'A-Z' 'a-z' \\
        | grep -qE "\\"pubkey\\"[[:space:]]*:[[:space:]]*\\"0x\${pk_hex_lower}\\""
}

# Discover node-1's libp2p peer ID by tailing the testnet service journal.
# The start command logs:  🌐 p2p listening on /ip4/.../tcp/PORT/p2p/PEERID
discover_node1_peer_id() {
    journalctl -u "\${TN1_SERVICE}" --no-pager 2>/dev/null \\
        | grep -oE '/p2p/12D3Koo[A-Za-z0-9]+' \\
        | tail -1 \\
        | sed 's|/p2p/||'
}

# Extract `pubkey_hex` from a Zebvix keyfile (JSON-like; the on-disk schema
# is `{"secret_hex":"…","pubkey_hex":"…","address":"0x…"}` — see
# `write_keyfile` in zebvix-chain/src/main.rs). Tolerates an optional `0x`
# prefix in case the on-disk format ever evolves to add one. Output is
# always raw hex (no prefix) so it can be paired with `--pubkey 0x\${...}`.
extract_pubkey_hex() {
    local keyfile="$1"
    grep -oE '"pubkey_hex"[[:space:]]*:[[:space:]]*"(0x)?[0-9a-fA-F]+"' "$keyfile" \\
        | sed -E 's/.*"pubkey_hex"[[:space:]]*:[[:space:]]*"(0x)?([0-9a-fA-F]+)".*/\\2/' \\
        | head -1
}

# Render the canonical (no-follower) systemd unit.
write_unit() {
    local peer_multiaddr="$1"
    cat > "$TN2_SERVICE_FILE" <<EOF
[Unit]
Description=Zebvix L1 Blockchain Node — TESTNET node-2 (Phase E.1)
Documentation=https://github.com/zebvix-org/zebvix-chain
After=network-online.target \${TN1_SERVICE}.service
Wants=network-online.target

[Service]
Type=simple
ExecStart=\${BIN} start --rpc 0.0.0.0:\${TN2_RPC_PORT} --p2p-port \${TN2_P2P_PORT} --home \${TN2_HOME} --peer \${peer_multiaddr}
Restart=always
RestartSec=5
LimitNOFILE=65536
SyslogIdentifier=\${TN2_SERVICE}
StandardOutput=journal
StandardError=journal
Environment=ZEBVIX_RPC_MAX_INFLIGHT=128
Environment=ZEBVIX_NETWORK=testnet
Environment=ZEBVIX_NODE_ROLE=node-2

[Install]
WantedBy=multi-user.target
EOF
}

# ── --status fast path ────────────────────────────────────────────────────
if [[ "$mode" == "status" ]]; then
    echo ""
    echo "── node-1 ──"
    if systemctl is-active --quiet "\${TN1_SERVICE}"; then
        echo "  ✓ \${TN1_SERVICE}: active   tip=$(get_height "\${TN1_RPC_PORT}" || echo '?')"
    else
        echo "  ✗ \${TN1_SERVICE}: NOT active"
    fi
    echo ""
    echo "── node-2 ──"
    if systemctl is-active --quiet "\${TN2_SERVICE}" 2>/dev/null; then
        echo "  ✓ \${TN2_SERVICE}: active   tip=$(get_height "\${TN2_RPC_PORT}" || echo '?')"
    else
        echo "  ✗ \${TN2_SERVICE}: NOT active (or unit not installed)"
    fi
    echo ""
    echo "── validator set (via node-1 RPC) ──"
    # zbx_listValidators result shape: {count, total_voting_power, quorum_threshold, validators:[{address,pubkey,voting_power}]}
    rpc_call "\${TN1_RPC_PORT}" "zbx_listValidators" 2>/dev/null \\
        | python3 -c '
import json, sys
try:
    r = json.load(sys.stdin)["result"]
    print(f"   count={r.get(\\"count\\")}  total_power={r.get(\\"total_voting_power\\")}  quorum={r.get(\\"quorum_threshold\\")}")
    for i, v in enumerate(r.get("validators", [])):
        print(f"   [{i+1}] addr={v.get(\\"address\\")}  power={v.get(\\"voting_power\\")}  pubkey={v.get(\\"pubkey\\")}")
except Exception as e:
    print(f"   (failed to parse: {e})")
' 2>/dev/null \\
        || echo "   (RPC unreachable or python3 missing)"
    echo ""
    echo "── recent vote stats ──"
    rpc_call "\${TN1_RPC_PORT}" "zbx_voteStats" 2>/dev/null | head -c 800
    echo ""
    exit 0
fi

# ── --dry-run: print plan and exit ────────────────────────────────────────
if [[ "$mode" == "dry" ]]; then
    echo ""
    echo "  [dry] would generate keypair (if absent): \${NEW_KEY}"
    echo "  [dry] would init node-2 home (if absent): \${BIN} init --validator-key \${NEW_KEY} --home \${TN2_HOME}"
    echo "  [dry] would discover node-1 peer ID from journalctl -u \${TN1_SERVICE}"
    echo "  [dry] would write systemd unit: \${TN2_SERVICE_FILE}"
    echo "  [dry] would: systemctl start \${TN2_SERVICE}  → poll until tip matches node-1"
    echo "  [dry] would submit: \${BIN} validator-add --signer-key \${FOUNDER_KEY} \\\\"
    echo "                          --pubkey 0x<NODE-2_PK> --power 1 \\\\"
    echo "                          --rpc-url http://127.0.0.1:\${TN1_RPC_PORT} --fee auto"
    echo "  [dry] would verify: zbx_listValidators returns 2 entries; zbx_voteStats lists both voters"
    echo "  [dry] NOTE: node-2 starts in voting-capable mode, but its producer stays dormant"
    echo "         until validator-add is applied (consensus.rs re-reads validators every tick)."
    echo "         => zero halt-window."
    exit 0
fi

# ── 1. Preflight checks (always for full or keygen) ──────────────────────
echo ""
echo "▶ preflight…"

if [[ ! -x "$BIN" ]]; then
    echo "❌ \${BIN} not found or not executable" >&2
    echo "   Run first:  sudo bash scripts/testnet-deploy.sh --build-only" >&2
    exit 2
fi

if ! systemctl is-active --quiet "\${TN1_SERVICE}"; then
    echo "❌ \${TN1_SERVICE} is not active — node-2 needs node-1 to sync from" >&2
    echo "   Run:  sudo systemctl start \${TN1_SERVICE}  ; then re-try" >&2
    exit 2
fi

tn1_height=$(get_height "\${TN1_RPC_PORT}" || echo "")
if [[ -z "$tn1_height" ]]; then
    echo "❌ testnet RPC at 127.0.0.1:\${TN1_RPC_PORT} not responding" >&2
    exit 2
fi
echo "  ✓ node-1 active at tip=\${tn1_height}"

if [[ ! -f "$FOUNDER_KEY" ]]; then
    echo "❌ founder/admin key not found at \${FOUNDER_KEY}" >&2
    echo "   Override with ZBX_FOUNDER_KEY=/path/to/admin.key" >&2
    exit 2
fi
echo "  ✓ founder key present"

# ── 2. Generate validator keypair (idempotent) ───────────────────────────
echo ""
echo "▶ generating validator keypair…"
if [[ -f "$NEW_KEY" ]]; then
    echo "  ↺ keypair already exists at \${NEW_KEY} — reusing"
else
    mkdir -p "$KEY_DIR"
    chmod 700 "$KEY_DIR"
    "$BIN" keygen --out "$NEW_KEY"
    chmod 600 "$NEW_KEY"
    echo "  ✓ keypair saved (mode 0600): \${NEW_KEY}"
fi

NEW_PK_HEX=$(extract_pubkey_hex "$NEW_KEY")
if [[ -z "$NEW_PK_HEX" ]]; then
    echo "❌ could not extract pubkey_hex from \${NEW_KEY}" >&2
    echo "   Expected JSON with field 'pubkey_hex' (see write_keyfile in main.rs)." >&2
    echo "   File contents (first 200 chars):" >&2
    head -c 200 "$NEW_KEY" >&2 ; echo "" >&2
    exit 1
fi
echo "  ✓ node-2 pubkey: 0x\${NEW_PK_HEX}"

if [[ "$mode" == "keygen" ]]; then
    echo ""
    echo "✅ keygen-only mode complete. Next:"
    echo "   sudo bash $0           # full deploy"
    exit 0
fi

# ── 2b. Idempotency check ────────────────────────────────────────────────
# If THIS pubkey is already in the validator set, the chain part is done —
# we only need to ensure the local service is running.
if validator_set_has_pubkey "\${TN1_RPC_PORT}" "\${NEW_PK_HEX}"; then
    echo ""
    echo "↺ node-2 pubkey is ALREADY in the validator set — chain side is done."
    if systemctl is-active --quiet "\${TN2_SERVICE}" 2>/dev/null; then
        echo "  ✓ \${TN2_SERVICE} is running. Nothing to do."
        echo ""
        echo "  Inspect:  sudo bash $0 --status"
        exit 0
    else
        echo "  ⚠  \${TN2_SERVICE} is not running — starting it."
        systemctl daemon-reload
        systemctl enable "\${TN2_SERVICE}" >/dev/null 2>&1 || true
        systemctl restart "\${TN2_SERVICE}"
        sleep 3
        if systemctl is-active --quiet "\${TN2_SERVICE}"; then
            echo "  ✓ \${TN2_SERVICE} restarted successfully."
            exit 0
        else
            echo "  ❌ \${TN2_SERVICE} failed to start. Check:" >&2
            echo "     journalctl -u \${TN2_SERVICE} --no-pager -n 30" >&2
            exit 1
        fi
    fi
fi

# A different validator was added (count >= 2 but not OUR pubkey) → refuse.
current_count=$(get_validator_count "\${TN1_RPC_PORT}")
if [[ "\${current_count:-0}" -ge 2 ]]; then
    echo "❌ validator set has \${current_count} entries, but none match this script's keypair." >&2
    echo "   Someone else added a different validator — manual cleanup required." >&2
    echo "   To add ANOTHER validator, set ZBX_TN2_NAME=node-3 (and bump ports/home)." >&2
    exit 3
fi
echo "  ✓ current validator count: \${current_count}  (need to add 1 more)"

# Refuse if would-be ports are already bound by something OTHER than us.
for port in "\${TN2_RPC_PORT}" "\${TN2_P2P_PORT}"; do
    if ss -ltn "sport = :\${port}" 2>/dev/null | grep -q LISTEN; then
        if ! systemctl is-active --quiet "\${TN2_SERVICE}" 2>/dev/null; then
            echo "❌ port \${port} already bound by another process (not \${TN2_SERVICE})." >&2
            echo "   Set ZBX_TN2_RPC_PORT or ZBX_TN2_P2P_PORT to free ports and re-try." >&2
            exit 2
        fi
    fi
done
echo "  ✓ ports \${TN2_RPC_PORT}/\${TN2_P2P_PORT} free (or owned by \${TN2_SERVICE})"

# ── 3. Initialize node-2 home (idempotent) ───────────────────────────────
echo ""
echo "▶ initializing node-2 home at \${TN2_HOME}…"
if [[ -f "\${TN2_HOME}/genesis.json" && -f "\${TN2_HOME}/node.json" ]]; then
    echo "  ↺ home already initialized — reusing (sync resumes from current tip)"
else
    # cmd_init always seeds the validator set deterministically with the
    # FOUNDER pubkey regardless of --validator-key, so passing node-2's key
    # here is fine (it just becomes the proposer key for THIS process).
    "$BIN" init --validator-key "$NEW_KEY" --home "$TN2_HOME"
    echo "  ✓ node-2 home initialized"
fi

# ── 4. Discover node-1's libp2p peer ID ───────────────────────────────────
echo ""
echo "▶ discovering node-1 libp2p peer ID from journalctl…"
PEER_ID=$(discover_node1_peer_id)
if [[ -z "$PEER_ID" ]]; then
    echo "❌ could not find peer_id in \${TN1_SERVICE} journal" >&2
    echo "   Try:  sudo systemctl restart \${TN1_SERVICE}  ; sleep 5 ; re-run this script" >&2
    exit 1
fi
PEER_MULTIADDR="/ip4/127.0.0.1/tcp/\${TN1_P2P_PORT}/p2p/\${PEER_ID}"
echo "  ✓ node-1 peer: \${PEER_MULTIADDR}"

# ── 5. Write canonical systemd unit (no --follower; producer is dormant
#       until our pubkey lands in the on-chain validator set) ────────────
echo ""
echo "▶ writing systemd unit \${TN2_SERVICE_FILE}…"
write_unit "$PEER_MULTIADDR"
systemctl daemon-reload
systemctl enable "\${TN2_SERVICE}" >/dev/null
echo "  ✓ unit written"

# ── 6. Start node-2 and wait for sync ────────────────────────────────────
echo ""
echo "▶ starting \${TN2_SERVICE} and waiting for sync…"
systemctl restart "\${TN2_SERVICE}"
sleep 3
if ! systemctl is-active --quiet "\${TN2_SERVICE}"; then
    echo "❌ \${TN2_SERVICE} failed to start. Recent logs:" >&2
    journalctl -u "\${TN2_SERVICE}" --no-pager --since "30 seconds ago" | tail -20 >&2
    exit 1
fi

elapsed=0; synced=0
while (( elapsed < SYNC_TIMEOUT )); do
    sleep 2; elapsed=$((elapsed + 2))
    h1=$(get_height "\${TN1_RPC_PORT}" || echo "")
    h2=$(get_height "\${TN2_RPC_PORT}" || echo "")
    if [[ -n "$h1" && -n "$h2" && "$h2" -ge $((h1 - 1)) ]]; then
        echo "  ✓ node-2 synced: tip h2=\${h2} ≥ h1-1=$((h1-1))  (after \${elapsed}s)"
        synced=1; break
    fi
    echo "    …syncing  h1=\${h1:-?}  h2=\${h2:-?}  (\${elapsed}s/\${SYNC_TIMEOUT}s)"
done
if (( synced != 1 )); then
    echo "❌ node-2 failed to sync within \${SYNC_TIMEOUT}s" >&2
    echo "   Inspect:  journalctl -u \${TN2_SERVICE} -n 50 --no-pager" >&2
    exit 1
fi

# ── 7. Submit validator-add tx ───────────────────────────────────────────
echo ""
echo "▶ submitting validator-add tx (signer = founder, target = 0x\${NEW_PK_HEX})…"
if ! "$BIN" validator-add \\
        --signer-key "$FOUNDER_KEY" \\
        --pubkey "0x\${NEW_PK_HEX}" \\
        --power 1 \\
        --rpc-url "http://127.0.0.1:\${TN1_RPC_PORT}" \\
        --fee auto ; then
    echo "❌ validator-add tx failed" >&2
    echo "   Node-2 is still alive (no chain damage — its producer is dormant)." >&2
    echo "   Inspect:  systemctl status \${TN2_SERVICE} ; journalctl -u \${TN1_SERVICE} -n 30" >&2
    exit 1
fi

# ── 8. Wait for validator set to update ──────────────────────────────────
echo ""
echo "▶ waiting for validator-add tx to be mined and applied…"
elapsed=0; added=0
while (( elapsed < TX_TIMEOUT )); do
    sleep 2; elapsed=$((elapsed + 2))
    if validator_set_has_pubkey "\${TN1_RPC_PORT}" "\${NEW_PK_HEX}"; then
        count=$(get_validator_count "\${TN1_RPC_PORT}")
        echo "  ✓ node-2 pubkey is now in validator set (count=\${count}, after \${elapsed}s)"
        added=1; break
    fi
    count=$(get_validator_count "\${TN1_RPC_PORT}")
    echo "    …waiting  current count=\${count:-?}  (\${elapsed}s/\${TX_TIMEOUT}s)"
done
if (( added != 1 )); then
    echo "❌ validator-add tx did not apply within \${TX_TIMEOUT}s" >&2
    echo "   Check:  curl -X POST http://127.0.0.1:\${TN1_RPC_PORT} -H 'Content-Type: application/json' \\\\" >&2
    echo "             -d '{\\"jsonrpc\\":\\"2.0\\",\\"id\\":1,\\"method\\":\\"zbx_listValidators\\",\\"params\\":[]}'" >&2
    exit 1
fi

# ── 9. Verify both validators participating ──────────────────────────────
# Producer re-reads the validator set every tick (consensus.rs ~line 200),
# so node-2 should automatically start voting on the next round — no
# restart needed. Wait for at least one block round to confirm.
echo ""
echo "▶ verifying both validators participate in voting…"
sleep 6   # enough for ≥1 block round at default BLOCK_TIME_SECS=2-3

vote_stats=$(rpc_call "\${TN1_RPC_PORT}" "zbx_voteStats" 2>/dev/null || echo "")
voter_count=$(echo "$vote_stats" \\
              | grep -oE '"voters":\\[[^]]*\\]' | head -1 \\
              | grep -oE '"0x[0-9a-fA-F]+"' | wc -l | tr -d ' ')
echo "  voter count in latest round: \${voter_count}"

if [[ "\${voter_count:-0}" -ge 2 ]]; then
    echo "  ✓ BOTH validators are voting — 2/2 quorum healthy"
else
    echo "  ⚠  only \${voter_count} voter(s) seen yet — node-2 may need another few seconds"
    echo "     Re-check:  sudo bash $0 --status"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✅ testnet validator added — chain now runs at 2/2 BFT quorum"
echo ""
echo "    Status:           sudo bash $0 --status"
echo "    node-1 logs:      sudo journalctl -u \${TN1_SERVICE} -f"
echo "    node-2 logs:      sudo journalctl -u \${TN2_SERVICE} -f"
echo "    Validator list:   curl -X POST http://127.0.0.1:\${TN1_RPC_PORT} \\\\"
echo "                          -H 'Content-Type: application/json' \\\\"
echo "                          -d '{\\"jsonrpc\\":\\"2.0\\",\\"id\\":1,\\"method\\":\\"zbx_listValidators\\",\\"params\\":[]}'"
echo ""
echo "    ⚠  Mainnet (chain_id 8989, port 8545) is UNTOUCHED."
echo "    ⚠  Testnet tokens have ZERO economic value."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"