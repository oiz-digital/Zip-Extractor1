#!/usr/bin/env bash
# mainnet-launch.sh — ZBX Chain mainnet launch checklist.
#
# Run this script to verify the node is ready for mainnet.
# All checks must pass before the genesis block is produced.
#
# Usage:
#   ./scripts/mainnet-launch.sh

set -euo pipefail

PASS=0; FAIL=0
check() { local name="\$1"; shift; if "\$@" &>/dev/null; then echo "  ✓ \$name"; ((PASS++)); else echo "  ✗ \$name"; ((FAIL++)); fi; }

echo "=== ZBX Chain Mainnet Launch Checklist ==="
echo ""

echo "── 1. Node binary ──────────────────────────"
check "zbx binary exists"          test -f "./target/release/zbx"
check "zbx version matches"        ./target/release/zbx --version | grep -q "0.1"

echo ""
echo "── 2. Configuration ────────────────────────"
check "mainnet.toml exists"        test -f "config/mainnet.toml"
check "chain_id = 8989"            grep -qE "chain_id[[:space:]]*=[[:space:]]*8989" config/mainnet.toml
check "genesis.json exists"        test -f "config/mainnet-genesis.json"
check "validators in genesis"      python3 -c "import json; g=json.load(open('config/mainnet-genesis.json')); assert len(g.get('validators',[])) >= 4"

echo ""
echo "── 3. Cryptographic material ───────────────"
check "node key exists"            test -f "\${ZBX_DATA_DIR:-/data/zbx}/node.key"
check "validator key exists"       test -f "\${ZBX_DATA_DIR:-/data/zbx}/validator.key"
check "BLS key exists"             test -f "\${ZBX_DATA_DIR:-/data/zbx}/bls.key"

echo ""
echo "── 4. Network ──────────────────────────────"
check "port 30333 open"            nc -z localhost 30333
check "port 8545 open"             nc -z localhost 8545
check "port 8546 open"             nc -z localhost 8546
check "minimum peers connected"    curl -s localhost:8545 -d '{"id":1,"jsonrpc":"2.0","method":"net_peerCount","params":[]}' | python3 -c "import sys,json; d=json.load(sys.stdin); assert int(d['result'],16) >= 3"

echo ""
echo "── 5. Smart contracts ──────────────────────"
ADDR_FILE=\$(ls deployments/mainnet-*.json 2>/dev/null | tail -1)
if [ -n "\$ADDR_FILE" ]; then
    check "deployment file exists"     test -f "\$ADDR_FILE"
    check "ZbxOracle deployed"         python3 -c "import json; d=json.load(open('\$ADDR_FILE')); assert d['contracts']['ZbxOracle'] != ''"
    check "ZbxVerifier deployed"       python3 -c "import json; d=json.load(open('\$ADDR_FILE')); assert d['contracts']['ZbxVerifier'] != ''"
    check "ZbxEntryPoint deployed"     python3 -c "import json; d=json.load(open('\$ADDR_FILE')); assert d['contracts']['ZbxEntryPoint'] != ''"
else
    echo "  ⚠  No deployment file found. Run ./scripts/deploy-contracts.sh mainnet first."
    ((FAIL++))
fi

echo ""
echo "── 6. Security ─────────────────────────────"
check "SSL certs present"          test -f "/etc/zbx/tls.crt"
check "no debug flags in binary"   ! strings ./target/release/zbx | grep -q "RUST_BACKTRACE"
check "SECURITY.md read"           test -f "SECURITY.md"

echo ""
echo "── 7. Monitoring ───────────────────────────"
check "Prometheus accessible"      curl -sf localhost:9090/metrics | head -1 | grep -q "zbx"
check "Grafana accessible"         curl -sf localhost:3000 | grep -q "Grafana" || true

echo ""
echo "═══════════════════════════════════════════"
echo "Results: \$PASS passed, \$FAIL failed"
if [ "\$FAIL" -gt 0 ]; then
    echo "⛔ FAILED — resolve all issues before launching mainnet"
    exit 1
else
    echo "✓ ALL CHECKS PASSED — ready to produce genesis block"
    echo ""
    echo "Next steps:"
    echo "  1. Co-ordinate with other validators to agree on genesis block hash"
    echo "  2. Run: zbx node --config config/mainnet.toml --genesis config/mainnet-genesis.json"
    echo "  3. Monitor: zbx admin status"
fi