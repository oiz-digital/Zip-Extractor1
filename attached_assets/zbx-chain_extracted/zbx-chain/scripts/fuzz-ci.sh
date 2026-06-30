#!/usr/bin/env bash
# Run all fuzz targets briefly in CI (30 seconds each).
# Full long fuzz runs should happen in a dedicated fuzzing environment.
#
# Usage:
#   bash scripts/fuzz-ci.sh                # 30s per target (default)
#   FUZZ_TIME=120 bash scripts/fuzz-ci.sh  # 2 min per target
#   FUZZ_TARGET=fuzz_zvm_bytecode bash scripts/fuzz-ci.sh  # single target

set -euo pipefail

FUZZ_TIME="\${FUZZ_TIME:-30}"
TARGETS=(
    fuzz_zvm_bytecode
    fuzz_zvm_native_opcodes
    fuzz_zvm_opcodes
    fuzz_rlp_encode_decode
    fuzz_rlp_decode_arbitrary
    fuzz_payid_parser
)

# If a specific target is requested, run only that
if [ -n "\${FUZZ_TARGET:-}" ]; then
    TARGETS=("$FUZZ_TARGET")
fi

echo "=== ZBX Fuzz Suite (\${FUZZ_TIME}s per target) ==="
echo "Targets: \${#TARGETS[@]}"
echo ""

PASS=0
FAIL=0
CRASH=0

for target in "\${TARGETS[@]}"; do
    echo "── \$target ──────────────────────────────────────────"
    set +e
    cargo +nightly fuzz run "\$target" -- \\
        -max_total_time="\${FUZZ_TIME}" \\
        -artifact_prefix="fuzz/artifacts/\${target}/"
    EXIT_CODE=\$?
    set -e

    if [ \$EXIT_CODE -eq 0 ]; then
        echo "✓ \$target — PASS"
        PASS=\$((PASS + 1))
    elif [ \$EXIT_CODE -eq 77 ]; then
        echo "✗ \$target — CRASH FOUND"
        CRASH=\$((CRASH + 1))
    else
        echo "? \$target — FAILED (exit code \$EXIT_CODE)"
        FAIL=\$((FAIL + 1))
    fi
    echo ""
done

echo "═══════════════════════════════════════"
echo "Results: \${PASS} passed, \${FAIL} failed, \${CRASH} crashes"

# Also run proptest suites (work in stable Rust)
echo ""
echo "=== Proptest Suites ==="
cargo test --test proptest_zvm -p zbx-zvm -- --test-threads=4
echo "✓ proptest_zvm"
cargo test --test proptest_rlp -p zbx-rlp -- --test-threads=4
echo "✓ proptest_rlp"

if [ \$CRASH -gt 0 ] || [ \$FAIL -gt 0 ]; then
    echo "FAIL: Crashes or failures detected."
    exit 1
fi

echo "All fuzz targets passed."