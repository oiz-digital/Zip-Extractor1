#!/usr/bin/env bash
# snapshot.sh — create a state snapshot for a running ZBX node.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "\${BASH_SOURCE[0]}")" && pwd)"
DATA_DIR="\${ZBX_DATA_DIR:-/var/lib/zbx}"
SNAPSHOT_DIR="\${ZBX_SNAPSHOT_DIR:-/var/lib/zbx/snapshots}"
BLOCK_NUM=""
RPC_URL="\${ZBX_RPC_URL:-http://localhost:8545}"
KEEP_LAST=5

usage() {
  echo "Usage: $0 [--block <num>] [--data-dir <path>] [--output <dir>]"
  echo ""
  echo "Options:"
  echo "  --block     Block number to snapshot (default: latest finalized)"
  echo "  --data-dir  Node data directory (default: /var/lib/zbx)"
  echo "  --output    Snapshot output directory (default: /var/lib/zbx/snapshots)"
  echo "  --keep      Number of snapshots to retain (default: 5)"
  exit 1
}

# Parse args.
while [[ $# -gt 0 ]]; do
  case "$1" in
    --block)    BLOCK_NUM="$2"; shift 2 ;;
    --data-dir) DATA_DIR="$2"; shift 2 ;;
    --output)   SNAPSHOT_DIR="$2"; shift 2 ;;
    --keep)     KEEP_LAST="$2"; shift 2 ;;
    -h|--help)  usage ;;
    *)          echo "Unknown option: $1"; usage ;;
  esac
done

mkdir -p "$SNAPSHOT_DIR"

# Get finalized block if not specified.
if [[ -z "$BLOCK_NUM" ]]; then
  BLOCK_NUM=$(curl -sf -X POST "$RPC_URL" \\
    -H 'Content-Type: application/json' \\
    -d '{"jsonrpc":"2.0","method":"zbx_getFinalizedBlock","params":[],"id":1}' \\
    | python3 -c "import sys,json; d=json.load(sys.stdin); print(int(d['result']['number'],16))")
  echo "Using finalized block #$BLOCK_NUM"
fi

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
SNAPSHOT_PATH="$SNAPSHOT_DIR/snapshot_\${BLOCK_NUM}_\${TIMESTAMP}"
mkdir -p "$SNAPSHOT_PATH"

echo "==> Creating snapshot at block #$BLOCK_NUM ..."

# Pause compaction to get a consistent snapshot.
curl -sf -X POST "$RPC_URL" \\
  -H 'Content-Type: application/json' \\
  -d '{"jsonrpc":"2.0","method":"zbx_pauseCompaction","params":[],"id":1}' \\
  > /dev/null || true

# Copy RocksDB checkpoint.
if command -v zbx &>/dev/null; then
  zbx snapshot create --block "$BLOCK_NUM" --output "$SNAPSHOT_PATH"
else
  cp -r "$DATA_DIR/db" "$SNAPSHOT_PATH/db"
fi

# Resume compaction.
curl -sf -X POST "$RPC_URL" \\
  -H 'Content-Type: application/json' \\
  -d '{"jsonrpc":"2.0","method":"zbx_resumeCompaction","params":[],"id":1}' \\
  > /dev/null || true

# Write metadata.
cat > "$SNAPSHOT_PATH/metadata.json" << EOF
{
  "block_number": $BLOCK_NUM,
  "timestamp": "$TIMESTAMP",
  "chain_id": 8989,
  "created_by": "$(hostname)"
}
EOF

# Compress.
echo "==> Compressing snapshot ..."
tar -czf "\${SNAPSHOT_PATH}.tar.gz" -C "$SNAPSHOT_DIR" "$(basename "$SNAPSHOT_PATH")"
rm -rf "$SNAPSHOT_PATH"

SNAPSHOT_SIZE=$(du -sh "\${SNAPSHOT_PATH}.tar.gz" | cut -f1)
echo "==> Snapshot created: \${SNAPSHOT_PATH}.tar.gz ($SNAPSHOT_SIZE)"

# Prune old snapshots.
ls -t "$SNAPSHOT_DIR"/snapshot_*.tar.gz 2>/dev/null | tail -n "+$((KEEP_LAST+1))" | xargs rm -f || true
echo "==> Pruned old snapshots, kept last $KEEP_LAST"