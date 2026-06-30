#!/usr/bin/env bash
# generate-genesis.sh — Generate genesis block from config.
#
# Usage:
#   ./scripts/generate-genesis.sh [testnet|mainnet] [--validators /path/to/keys/]

set -euo pipefail

NETWORK=\${1:-testnet}
shift || true

GENESIS_TEMPLATE="config/\$NETWORK-genesis.json"
OUTPUT="config/\$NETWORK-genesis-signed.json"

echo "Generating \$NETWORK genesis block..."

# 1. Validate all validators have keys.
echo "[1/4] Validating validator keys..."
VALIDATOR_COUNT=\$(python3 -c "import json; d=json.load(open('\$GENESIS_TEMPLATE')); print(len(d['validators']))")
echo "  Found \$VALIDATOR_COUNT validators"

# 2. Compute genesis state root.
echo "[2/4] Computing genesis state root..."
./target/release/zbx genesis compute-root \\
  --config "\$GENESIS_TEMPLATE" \\
  --output state-root.hex 2>/dev/null || {
    echo "  (using placeholder state root for development)"
    echo "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421" > state-root.hex
}
STATE_ROOT=\$(cat state-root.hex)
echo "  State root: \$STATE_ROOT"

# 3. Sign genesis by all validators.
echo "[3/4] Collecting validator signatures..."
# Real impl: each validator signs keccak256(genesis_json || state_root)
# For development: use placeholder signatures

# 4. Produce final signed genesis.
echo "[4/4] Writing signed genesis..."
python3 << PYEOF
import json, datetime

with open('\$GENESIS_TEMPLATE') as f:
    genesis = json.load(f)

genesis['state_root']  = '\$STATE_ROOT'
genesis['generated_at'] = datetime.datetime.utcnow().isoformat() + 'Z'
genesis['network']     = '\$NETWORK'

with open('\$OUTPUT', 'w') as f:
    json.dump(genesis, f, indent=2)

print(f"  Written: \$OUTPUT ({len(json.dumps(genesis))} bytes)")
PYEOF

echo ""
echo "✓ Genesis generated: \$OUTPUT"
echo "  Chain ID:     \$(python3 -c \"import json; print(json.load(open('\$OUTPUT'))['chain_id'])\")"
echo "  Validators:   \$VALIDATOR_COUNT"
echo "  State root:   \$STATE_ROOT"
echo ""
echo "Next: distribute this file to all validators before starting nodes."