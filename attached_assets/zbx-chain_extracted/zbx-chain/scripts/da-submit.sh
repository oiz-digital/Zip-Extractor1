#!/usr/bin/env bash
# da-submit.sh — Submit blob data to the ZBX DA layer.
#
# Usage:
#   ./scripts/da-submit.sh <data-file> [rpc-url]
#
# Example:
#   ./scripts/da-submit.sh /tmp/rollup_batch.bin http://localhost:8545

set -euo pipefail

DATA_FILE=\${1:-}
RPC_URL=\${2:-http://localhost:8545}

if [[ -z "\$DATA_FILE" ]]; then
    echo "Usage: $0 <data-file> [rpc-url]"
    exit 1
fi

if [[ ! -f "\$DATA_FILE" ]]; then
    echo "Error: file '\$DATA_FILE' not found"
    exit 1
fi

DATA_SIZE=\$(wc -c < "\$DATA_FILE")
MAX_BLOB_SIZE=131072

if [[ "\$DATA_SIZE" -gt "\$MAX_BLOB_SIZE" ]]; then
    echo "Error: data size \$DATA_SIZE exceeds max blob size \$MAX_BLOB_SIZE bytes"
    exit 1
fi

echo "=== ZBX DA Blob Submission ==="
echo "File:     \$DATA_FILE"
echo "Size:     \$DATA_SIZE bytes"
echo "RPC URL:  \$RPC_URL"
echo ""

# Hex-encode the data
DATA_HEX="0x\$(xxd -p "\$DATA_FILE" | tr -d '\\n')"

# Submit via zbx-cli (wraps eth_sendRawTransaction with blob type)
echo "Submitting blob..."
RESULT=\$(zbx-cli da submit-blob \\
    --data "\$DATA_HEX" \\
    --rpc-url "\$RPC_URL" \\
    --chain-id 8989 \\
    2>&1)

if echo "\$RESULT" | grep -q "versioned_hash"; then
    HASH=\$(echo "\$RESULT" | grep versioned_hash | awk '{print \$2}')
    echo ""
    echo "Blob submitted successfully!"
    echo "Versioned hash: \$HASH"
    echo ""
    echo "To verify availability:"
    echo "  zbx-cli da check-availability --hash \$HASH --rpc-url \$RPC_URL"
else
    echo "Error submitting blob:"
    echo "\$RESULT"
    exit 1
fi