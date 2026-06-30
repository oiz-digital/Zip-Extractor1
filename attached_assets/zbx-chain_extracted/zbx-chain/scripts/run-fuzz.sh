#!/usr/bin/env bash
# run-fuzz.sh — Run cargo-fuzz targets locally.
#
# Usage:
#   ./scripts/run-fuzz.sh [target] [duration_seconds]
#
# Examples:
#   ./scripts/run-fuzz.sh                  # fuzz all targets for 60s each
#   ./scripts/run-fuzz.sh tx_decode 300    # fuzz tx_decode for 5 minutes
#   ./scripts/run-fuzz.sh block_import     # fuzz block_import for 60s

set -euo pipefail

TARGET=\${1:-all}
DURATION=\${2:-60}

# Check cargo-fuzz installed.
if ! command -v cargo-fuzz &>/dev/null; then
    echo "Installing cargo-fuzz..."
    cargo +nightly install cargo-fuzz
fi

ALL_TARGETS=(tx_decode block_import rlp_decode ssz_decode abi_decode)

fuzz_target() {
    local target="\$1"
    echo "──────────────────────────────────────"
    echo "Fuzzing: \$target (\${DURATION}s)"
    echo "──────────────────────────────────────"
    cargo +nightly fuzz run "\$target" -- \\
        -max_total_time="\$DURATION" \\
        -max_len=4096 \\
        -print_final_stats=1 \\
        2>&1 | tail -20

    CRASH_DIR="fuzz/artifacts/\$target"
    if [ -d "\$CRASH_DIR" ] && [ "$(ls -A \$CRASH_DIR 2>/dev/null)" ]; then
        echo "⚠️  CRASHES FOUND in \$target!"
        ls -la "\$CRASH_DIR"
        return 1
    fi
    echo "✓ No crashes in \$target"
}

if [ "\$TARGET" = "all" ]; then
    FAILED=()
    for t in "\${ALL_TARGETS[@]}"; do
        fuzz_target "\$t" || FAILED+=("\$t")
    done
    echo ""
    if [ \${#FAILED[@]} -eq 0 ]; then
        echo "✓ All fuzz targets clean"
    else
        echo "⚠️  Crashes in: \${FAILED[*]}"
        exit 1
    fi
else
    fuzz_target "\$TARGET"
fi