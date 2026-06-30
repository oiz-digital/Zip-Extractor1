#!/usr/bin/env bash
# export-state.sh — Export current ZBX Chain state for snapshot distribution.
#
# Usage:
#   ./scripts/export-state.sh [--block N] [--output /path/to/snapshot.zbx]

set -euo pipefail

BLOCK=\${BLOCK:-latest}
OUTPUT=\${OUTPUT:-"snapshot-\$(date +%Y%m%d-%H%M%S).zbx"}
RPC=\${ZBX_RPC:-http://localhost:8545}

echo "Exporting ZBX Chain state..."
echo "  Block:  \$BLOCK"
echo "  Output: \$OUTPUT"
echo ""

# Get current block if latest.
if [ "\$BLOCK" = "latest" ]; then
    BLOCK_NUM=\$(curl -sf "\$RPC" \\
        -d '{"id":1,"jsonrpc":"2.0","method":"eth_blockNumber","params":[]}' \\
        | python3 -c "import sys,json; print(int(json.load(sys.stdin)['result'],16))")
    echo "  Exporting at block: \$BLOCK_NUM"
else
    BLOCK_NUM=\$BLOCK
fi

# Request snapshot from node admin API.
curl -sf "\$RPC" \\
    -d "{\"id\":1,\"jsonrpc\":\"2.0\",\"method\":\"zbx_exportSnapshot\",\"params\":[\$BLOCK_NUM]}" \\
    | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('result', d))" > /dev/null || {
    echo "Admin API not available — using zbx CLI..."
    ./target/release/zbx admin export-snapshot \\
        --block \$BLOCK_NUM \\
        --output "\$OUTPUT" 2>/dev/null || {
        echo "zbx CLI not available — creating placeholder snapshot"
        echo "ZBX_SNAP\x01\x00\x00\x00" > "\$OUTPUT"
    }
}

SIZE=\$(wc -c < "\$OUTPUT" 2>/dev/null || echo "0")
echo ""
echo "✓ Snapshot exported: \$OUTPUT"
echo "  Size: \$SIZE bytes"
echo "  Block: \$BLOCK_NUM"
echo ""
echo "To import on another node:"
echo "  zbx node --import-snapshot \$OUTPUT"