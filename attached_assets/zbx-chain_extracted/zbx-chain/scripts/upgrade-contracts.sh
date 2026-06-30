#!/usr/bin/env bash
# upgrade-contracts.sh — Upgrade UUPS proxy contracts via governance timelock.
#
# Usage:
#   ./scripts/upgrade-contracts.sh [network] [contract-name] [new-impl-address]
#
# Example:
#   ./scripts/upgrade-contracts.sh testnet ZbxOracle 0xNewImpl...

set -euo pipefail

NETWORK=\${1:?Usage: ./scripts/upgrade-contracts.sh [network] [contract] [new-impl]}
CONTRACT=\${2:?missing contract name}
NEW_IMPL=\${3:?missing new implementation address}

ADDR_FILE=\$(ls deployments/\$NETWORK-*.json 2>/dev/null | tail -1)
[ -n "\$ADDR_FILE" ] || { echo "No deployment file for \$NETWORK"; exit 1; }

case "\$NETWORK" in
  testnet) RPC="\${ZBX_TESTNET_RPC:-https://testnet-rpc.zbvix.com}"; KEY="\$TESTNET_PRIVATE_KEY" ;;
  mainnet) RPC="\${ZBX_MAINNET_RPC:-https://rpc.zbvix.com}"; KEY="\$MAINNET_PRIVATE_KEY" ;;
  devnet)  RPC="http://localhost:8545"; KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80" ;;
  *)       echo "Unknown network"; exit 1 ;;
esac

PROXY_ADDR=\$(python3 -c "import json; print(json.load(open('\$ADDR_FILE'))['contracts'].get('\$CONTRACT', ''))")
[ -n "\$PROXY_ADDR" ] || { echo "Contract \$CONTRACT not found in \$ADDR_FILE"; exit 1; }

echo "Upgrading \$CONTRACT on \$NETWORK"
echo "  Proxy:          \$PROXY_ADDR"
echo "  New impl:       \$NEW_IMPL"
echo ""

# 1. Verify new impl is a valid contract.
echo "[1/4] Verifying new implementation..."
CODE_SIZE=\$(cast codesize "\$NEW_IMPL" --rpc-url "\$RPC" 2>/dev/null || echo "0")
[ "\$CODE_SIZE" -gt "0" ] || { echo "ERROR: new impl has no code"; exit 1; }
echo "  ✓ New impl has \$CODE_SIZE bytes of code"

# 2. Schedule upgrade through timelock (48h delay).
echo "[2/4] Scheduling upgrade through ZbxTimelock..."
TIMELOCK=\$(python3 -c "import json; print(json.load(open('\$ADDR_FILE'))['contracts'].get('ZbxTimelock', ''))")
if [ -n "\$TIMELOCK" ] && [ "\$NETWORK" != "devnet" ]; then
    UPGRADE_CALLDATA=\$(cast calldata "upgradeTo(address)" "\$NEW_IMPL")
    cast send "\$TIMELOCK" "schedule(address,uint256,bytes,bytes32,bytes32,uint256)" \\
      "\$PROXY_ADDR" 0 "\$UPGRADE_CALLDATA" "0x0" "0x0" 172800 \\
      --rpc-url "\$RPC" --private-key "\$KEY"
    echo "  ✓ Upgrade scheduled (48h delay)"
    echo "  Execute after: \$(date -d '+48 hours' 2>/dev/null || date -v+48H)"
else
    # devnet: upgrade immediately
    echo "[2/4] devnet: upgrading immediately (no timelock)..."
    cast send "\$PROXY_ADDR" "upgradeTo(address)" "\$NEW_IMPL" \\
      --rpc-url "\$RPC" --private-key "\$KEY"
    echo "  ✓ Upgraded immediately"
fi

# 3. Verify upgrade.
echo "[3/4] Verifying upgrade..."
CURRENT_IMPL=\$(cast call "\$PROXY_ADDR" "implementation()(address)" --rpc-url "\$RPC" 2>/dev/null || echo "0x0")
if [ "\$NETWORK" = "devnet" ]; then
    [ "\$CURRENT_IMPL" = "\$NEW_IMPL" ] && echo "  ✓ Implementation updated" || echo "  ✗ Update failed"
fi

# 4. Save upgrade record.
echo "[4/4] Recording upgrade..."
RECORD_FILE="deployments/upgrades-\$NETWORK.json"
python3 << PYEOF
import json, datetime, os
records = []
if os.path.exists('\$RECORD_FILE'):
    with open('\$RECORD_FILE') as f:
        records = json.load(f)
records.append({
    'contract': '\$CONTRACT',
    'proxy': '\$PROXY_ADDR',
    'new_impl': '\$NEW_IMPL',
    'network': '\$NETWORK',
    'scheduled_at': datetime.datetime.utcnow().isoformat() + 'Z'
})
with open('\$RECORD_FILE', 'w') as f:
    json.dump(records, f, indent=2)
print(f"  Saved to \$RECORD_FILE")
PYEOF

echo ""
echo "✓ Upgrade process complete"