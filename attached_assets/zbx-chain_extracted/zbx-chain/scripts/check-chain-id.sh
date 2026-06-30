#!/usr/bin/env bash
# check-chain-id.sh — S13.2 chain-id drift CI guard.
#
# Fails (exit 1) if any forbidden chain-id literal (\b7878\b, \b7879\b,
# \b7880\b) appears in the codebase OUTSIDE the explicit allowlist.
#
# Locked chain IDs (post-S13):
#   mainnet            = 8989  (0x231D)
#   testnet + devnet   = 8990  (0x231E)
#
# 7878 / 7879 / 7880 are LEGACY chain-id values and must never reappear
# in source/config/test code. They MAY still appear in:
#
#   1. SLIP-44 BIP-44 derivation paths (coin_type 7878 is SLIP-44
#      registered for ZBX and is INDEPENDENT of the EVM chain ID).
#   2. Frozen historical artifacts (CHANGELOG, audits, migration docs,
#      proposals describing the drift).
#   3. SDK back-compat constants explicitly tagged "RETIRED".
#   4. This script itself.
#
# All such files are listed in the ALLOWLIST array below. Everything
# else is checked. Add new exemptions deliberately and with a comment.
#
# Usage:
#   bash scripts/check-chain-id.sh            # repo scan (CI default)
#   bash scripts/check-chain-id.sh --verbose  # show allowlist hits too
#
# Exit codes:
#   0 — clean (no forbidden literals outside allowlist)
#   1 — at least one forbidden literal found in non-allowlisted file
#   2 — usage / environment error

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHAIN_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$CHAIN_ROOT"

VERBOSE=0
if [[ "${1:-}" == "--verbose" || "${1:-}" == "-v" ]]; then
  VERBOSE=1
fi

if ! command -v rg >/dev/null 2>&1; then
  echo "ERROR: ripgrep (rg) not installed; required for chain-id guard." >&2
  exit 2
fi

# ── Allowlist ──────────────────────────────────────────────────────────
# Files where 7878/7879/7880 are EXPECTED and intentional.
# Match is by exact relative path from CHAIN_ROOT.
ALLOWLIST=(
  # SLIP-44 BIP-44 coin type (7878) — independent of chain ID.
  "crates/zbx-types/src/lib.rs"
  "crates/zbx-wallet/src/create_import.rs"
  "crates/zbx-wallet/src/hd.rs"           # BIP-44 path docs: m/44'/7878'/...
  "crates/zbx-wallet/src/mnemonic.rs"     # BIP-44 path docs: m/44'/7878'/...
  "crates/zbx-wallet/src/pq_wallet.rs"    # BIP-44 path docs: m/44'/7878'/...
  "crates/zbx-sdk/src/hd_wallet.rs"
  "crates/zbx-sdk/src/types.rs"

  # SDK historical constants tagged RETIRED for back-compat.
  "sdk/zebvix-js/src/constants.ts"
  "sdk/ethers-zbx/src/chain.ts"

  # Frozen audit / changelog / migration history (root + docs/).
  "AUDIT_2026-04-30.md"
  "CHANGELOG.md"
  "HARDENING_TODO.md"
  "docs/DOC_STATUS.md"
  "docs/CHANGELOG.md"                      # docs-tree changelog; references S13 fix
  "docs/SECURITY_AUDIT.md"                 # audit record quoting old chain-id literals

  # Proposals describing the drift / launch plan itself.
  "docs/proposals/S13-CHAIN-ID-DRIFT-fix.md"
  "docs/proposals/PHASE-PLAN-2026-05-01.md"
  "docs/proposals/DEVNET-LAUNCH-PLAN-2026-05-01.md"
  "docs/proposals/S33-state-root-mpt.md"  # mentions chain IDs in historical context
  "docs/MAINNET-READINESS-2026-05-09.md"  # spec doc, naturally references the locked chain IDs
  "docs/SECURITY_FIXES_2026-05-09.md"     # security log, references chain IDs in narrative

  # Bridge doc keeps an intentional "stale" warning quoting old IDs.
  "docs/BRIDGE.md"

  # This script.
  "scripts/check-chain-id.sh"
)

is_allowlisted() {
  local f="$1"
  local entry
  for entry in "${ALLOWLIST[@]}"; do
    if [[ "$f" == "$entry" ]]; then
      return 0
    fi
  done
  return 1
}

# ── Scan ───────────────────────────────────────────────────────────────
# rg flags:
#   -n        line numbers
#   --hidden  include dotfiles (catch CI configs)
#   -g !…     skip build artifacts and lockfiles
#
# PATTERN: literal 7878/7879/7880 with optional integer/BigInt suffix.
# Catches all these forms:
#   7878          plain decimal
#   7878n         JS/TS BigInt literal      ← was missed by old `\b…\b` regex
#   7878u64       Rust unsigned suffix
#   7878i32       Rust signed suffix
#   7878isize / 7878usize / 7878f64
#   7878L         C/Java long suffix
#
# Still does NOT match prefix collisions like 78780 (boundary fails between
# the literal-tail and the trailing digit). `-w` is intentionally NOT used
# because it would conflict with optional-suffix matching — the explicit
# `\b…\b` anchors in the pattern handle word boundaries correctly.
PATTERN='\b(7878|7879|7880)(n|i8|i16|i32|i64|i128|isize|u8|u16|u32|u64|u128|usize|f32|f64|L|UL|ULL)?\b'

mapfile -t HITS < <(
  rg -n "$PATTERN" \
    --hidden \
    -g '!target/**' \
    -g '!node_modules/**' \
    -g '!**/Cargo.lock' \
    -g '!**/pnpm-lock.yaml' \
    -g '!**/*.lock' \
    -g '!**/dist/**' \
    -g '!**/build/**' \
    -g '!.git/**' \
    || true
)

VIOLATIONS=()
ALLOWED_HITS=()

for line in "${HITS[@]}"; do
  # rg output: path:line:content
  file="${line%%:*}"
  if is_allowlisted "$file"; then
    ALLOWED_HITS+=("$line")
  else
    VIOLATIONS+=("$line")
  fi
done

# ── Report ─────────────────────────────────────────────────────────────
if [[ $VERBOSE -eq 1 ]]; then
  echo "── Allowlisted hits (${#ALLOWED_HITS[@]}) ──────────────────────────"
  for h in "${ALLOWED_HITS[@]}"; do
    echo "  OK  $h"
  done
  echo
fi

if [[ ${#VIOLATIONS[@]} -gt 0 ]]; then
  echo "❌ check-chain-id: ${#VIOLATIONS[@]} forbidden chain-id literal(s) found."
  echo
  echo "Locked: mainnet=8989, testnet+devnet=8990."
  echo "7878/7879/7880 are legacy values. Use zbx_types::CHAIN_ID_MAINNET"
  echo "or zbx_types::CHAIN_ID_TESTNET. Update the allowlist in"
  echo "scripts/check-chain-id.sh ONLY for intentional historical refs."
  echo
  for v in "${VIOLATIONS[@]}"; do
    echo "  $v"
  done
  exit 1
fi

echo "✅ check-chain-id: clean (${#ALLOWED_HITS[@]} allowlisted hits)."
exit 0
