#!/usr/bin/env bash
# Phase E / Phase 0 — Generate a per-node validator keypair for the testnet.
#
# This is a thin wrapper around `zebvix-node-testnet keygen` that adds:
#   1. Loud testnet warnings so the operator never reuses these keys on mainnet.
#   2. Auto-saves the private key to a path with restrictive perms (0600).
#   3. Prints next-step commands for adding this validator to the testnet
#      validator set via `validator-add` tx (governor-signed).
#
# The genesis founder validator is hardcoded in tokenomics.rs (same pubkey
# as mainnet — operator convenience). Additional validators (N=2, N=3, etc.)
# are added post-genesis via `validator-add` txs once the testnet chain is
# live and the operator has decided how many nodes to run.
#
# Usage:
#   bash scripts/testnet-genesis-keygen.sh                  # default name "node-2"
#   bash scripts/testnet-genesis-keygen.sh node-3
#   bash scripts/testnet-genesis-keygen.sh --out /custom/path
#
# Output:
#   /root/.zebvix-testnet/validator-keys/<name>.key   (mode 0600)
#
# Exit codes:
#   0 — keypair generated and persisted
#   1 — keygen failure
#   2 — usage / environment error

set -euo pipefail

BIN="\${ZBX_TESTNET_BIN:-/usr/local/bin/zebvix-node-testnet}"
KEY_DIR="\${ZBX_TESTNET_KEY_DIR:-/root/.zebvix-testnet/validator-keys}"

if [[ ! -x "$BIN" ]]; then
    echo "❌ \${BIN} not found or not executable" >&2
    echo "   Run:  sudo bash scripts/testnet-deploy.sh --build-only" >&2
    exit 2
fi

name="node-2"
custom_out=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --out)
            shift
            custom_out="\${1:?--out requires a path}"
            shift
            ;;
        -h|--help)
            sed -n '2,28p' "\${BASH_SOURCE[0]}"
            exit 0
            ;;
        --*)
            echo "❌ unknown flag: $1" >&2
            exit 2
            ;;
        *)
            name="$1"
            shift
            ;;
    esac
done

if [[ -n "$custom_out" ]]; then
    out_path="$custom_out"
    out_dir="$(dirname "$out_path")"
else
    out_dir="$KEY_DIR"
    out_path="\${out_dir}/\${name}.key"
fi

if [[ -f "$out_path" ]]; then
    echo "❌ \${out_path} already exists — refusing to overwrite" >&2
    echo "   Move or delete the existing key first." >&2
    exit 2
fi

mkdir -p "$out_dir"
chmod 700 "$out_dir"

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🧪  TESTNET validator keypair generation"
echo "    binary : \${BIN}"
echo "    out    : \${out_path}"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "⚠  These keys are TESTNET ONLY. Reusing them on mainnet would expose"
echo "   mainnet funds because the same private key derives the same address"
echo "   on every secp256k1 chain — but cross-chain replay is BLOCKED by the"
echo "   chain_id field in TxBody (testnet=8990, mainnet=8989)."
echo ""

"$BIN" keygen --out "$out_path"
chmod 600 "$out_path"

echo ""
echo "✅ keypair saved (mode 0600): \${out_path}"
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Next steps to add this node as a testnet validator:"
echo ""
echo "  1. Inspect the new pubkey + address:"
echo "       cat \${out_path}    # contains hex-encoded secret + derived pubkey/address"
echo ""
echo "  2. Sign a validator-add tx from the GOVERNOR address (founder by default):"
echo "       zebvix-node-testnet validator-add \\\\"
echo "           --pubkey-hex <NEW_NODE_PUBKEY_HEX> --power 1 \\\\"
echo "           --rpc http://127.0.0.1:18545 \\\\"
echo "           --governor-key /path/to/governor.key"
echo ""
echo "  3. Wait ~10s, then verify the validator set:"
echo "       curl -X POST http://127.0.0.1:18545 \\\\"
echo "           -H 'Content-Type: application/json' \\\\"
echo "           -d '{\\"jsonrpc\\":\\"2.0\\",\\"method\\":\\"zbx_validators\\",\\"params\\":[],\\"id\\":1}'"
echo ""
echo "  4. Start the new node (on whatever VPS will run it):"
echo "       zebvix-node-testnet start \\\\"
echo "           --validator-key \${out_path} \\\\"
echo "           --rpc 0.0.0.0:18545 --p2p-port 31333 \\\\"
echo "           --home /root/.zebvix-testnet \\\\"
echo "           --peer /ip4/<GENESIS_NODE_IP>/tcp/31333/p2p/<PEER_ID>"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"