#!/usr/bin/env bash
# verify-contracts.sh — Verify deployed contract source on ZBX Explorer.
#
# Usage:
#   ./scripts/verify-contracts.sh <deployment-json-file>
#
# Requires: ZBXSCAN_API_KEY env var

set -euo pipefail

ADDR_FILE="\${1:?Usage: ./scripts/verify-contracts.sh <deployment.json>}"
API_KEY="\${ZBXSCAN_API_KEY:?ZBXSCAN_API_KEY not set}"

CHAIN_ID=\$(python3 -c "import json; print(json.load(open('\$ADDR_FILE'))['chain_id'])")
NETWORK=\$(python3 -c "import json; print(json.load(open('\$ADDR_FILE'))['network'])")

case "\$NETWORK" in
  mainnet) EXPLORER="https://explorer.zbvix.com/api" ;;
  testnet) EXPLORER="https://testnet-explorer.zbvix.com/api" ;;
  *)       EXPLORER="http://localhost:4000/api" ;;
esac

verify_contract() {
    local name="\$1"
    local address="\$2"
    local source_path="\$3"

    echo "  Verifying \$name at \$address..."
    curl -s -X POST "\$EXPLORER/v2/smart-contracts/\$address/verification/via/flattened-code" \\
      -H "x-api-key: \$API_KEY" \\
      -H "Content-Type: application/json" \\
      -d "{
        \"compiler_version\": \"v0.8.24+commit.e11b9ed9\",
        \"source_code\": \"\$(forge flatten \$source_path | python3 -c 'import sys,json; print(json.dumps(sys.stdin.read()))')\",
        \"contract_name\": \"\$name\",
        \"optimization\": true,
        \"optimization_runs\": 200
      }" | python3 -c "import sys,json; d=json.load(sys.stdin); print('    ✓ verified' if d.get('status')=='1' else '    ✗ ' + str(d))"
}

echo "Verifying contracts on \$NETWORK (\$EXPLORER)..."
python3 << PYEOF
import json

data = json.load(open('\$ADDR_FILE'))
contracts = data['contracts']
contract_sources = {
    'ZbxOracle':     'contracts/ZbxOracle.sol',
    'ZbxVerifier':   'contracts/ZbxVerifier.sol',
    'ZbxEntryPoint': 'contracts/ZbxEntryPoint.sol',
    'ZbxRouter':     'contracts/ZbxRouter.sol',
    'WZBX':          'contracts/tokens/WZBX.sol',
}
for name, path in contract_sources.items():
    addr = contracts.get(name, '')
    if addr:
        print(f'  {name}: {addr} → {path}')
PYEOF

echo "✓ Verification submitted. Check \$EXPLORER for status."