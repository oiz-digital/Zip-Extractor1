#!/usr/bin/env bash
# benchmark.sh — run the ZBX chain benchmarks and emit a Markdown report.
set -euo pipefail

OUTPUT_DIR="target/criterion"
REPORT_FILE="BENCHMARK_REPORT.md"
BASELINE=""

usage() {
  echo "Usage: $0 [--baseline <name>] [--compare <name>]"
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --baseline) BASELINE="$2"; shift 2 ;;
    -h|--help)  usage ;;
    *)          echo "Unknown option: $1"; usage ;;
  esac
done

echo "==> Running benchmarks..."

BENCH_ARGS="--bench block_execution --bench tx_throughput"
if [[ -n "$BASELINE" ]]; then
  BENCH_ARGS="$BENCH_ARGS -- --save-baseline $BASELINE"
fi

cargo bench $BENCH_ARGS 2>&1 | tee /tmp/bench_output.txt

echo "==> Generating report..."

{
  echo "# Benchmark Report"
  echo ""
  echo "Date: $(date -u '+%Y-%m-%d %H:%M UTC')"
  echo "Commit: $(git rev-parse --short HEAD)"
  echo ""
  echo "## Block Execution"
  echo ""
  grep -A3 "block_execution" /tmp/bench_output.txt || echo "No block_execution results"
  echo ""
  echo "## Transaction Throughput"
  echo ""
  grep -A3 "tx_throughput" /tmp/bench_output.txt || echo "No tx_throughput results"
  echo ""
  echo "## System"
  echo ""
  echo "- CPU: $(grep 'model name' /proc/cpuinfo 2>/dev/null | head -1 | cut -d: -f2 | xargs || sysctl -n machdep.cpu.brand_string 2>/dev/null || echo 'unknown')"
  echo "- RAM: $(free -h 2>/dev/null | grep Mem | awk '{print $2}' || echo 'unknown')"
  echo "- Rust: $(rustc --version)"
} > "$REPORT_FILE"

echo "==> Report written to $REPORT_FILE"