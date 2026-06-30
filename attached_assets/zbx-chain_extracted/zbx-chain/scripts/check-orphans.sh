#!/usr/bin/env bash
# scripts/check-orphans.sh — CI guard preventing accidental "orphan" .rs files
#
# An "orphan" is a Rust source file at `crates/<crate>/src/*.rs` that is NOT
# declared via `mod <name>;` in its crate's `lib.rs` or `main.rs`. Cargo
# silently ignores such files, so they can drift and rot.
#
# Files inside `crates/<crate>/src/_archive/` are EXEMPT — they are an
# intentional design backlog (see `crates/_ARCHIVE_MANIFEST.md`).
#
# Exit codes:
#   0 — clean (no orphans)
#   1 — orphans detected (lists each one)
#   2 — bad invocation / missing dependency
#
# Flags:
#   --verbose   list every checked file even when clean
#   --help      print usage

set -euo pipefail

VERBOSE=0
for arg in "$@"; do
  case "$arg" in
    --verbose) VERBOSE=1 ;;
    --help|-h)
      sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *)
      echo "unknown arg: $arg" >&2
      exit 2 ;;
  esac
done

if ! command -v python3 >/dev/null 2>&1; then
  echo "❌ check-orphans: python3 required" >&2
  exit 2
fi

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

if [[ ! -d crates ]]; then
  echo "❌ check-orphans: crates/ not found (run from repo root)" >&2
  exit 2
fi

python3 - "$VERBOSE" <<'PY'
import os, re, pathlib, sys
verbose = sys.argv[1] == "1"
root = pathlib.Path('crates')
orphans = []
checked = 0
for crate_dir in sorted(root.iterdir()):
    if not crate_dir.is_dir() or crate_dir.name.startswith('_'): continue
    src = crate_dir / 'src'
    if not src.is_dir(): continue
    lib = src / 'lib.rs'
    if not lib.exists(): lib = src / 'main.rs'
    if not lib.exists(): continue
    declared = set(re.findall(r'\bmod\s+([a-zA-Z_][a-zA-Z0-9_]*)\b',
                              lib.read_text(errors='ignore')))
    # maxdepth-1 only — files in _archive/ subdirs are exempt
    for rs in src.glob('*.rs'):
        if rs.name in ('lib.rs', 'main.rs', 'mod.rs'): continue
        checked += 1
        if rs.stem not in declared:
            orphans.append(str(rs))
        elif verbose:
            print(f"  ✓ {rs}")

if orphans:
    print(f"❌ check-orphans: {len(orphans)} orphan file(s) found", file=sys.stderr)
    for o in orphans:
        print(f"  - {o}", file=sys.stderr)
    print("", file=sys.stderr)
    print("Each file above lives at `crates/<crate>/src/*.rs` but is NOT", file=sys.stderr)
    print("declared via `mod <name>;` in lib.rs/main.rs. Either:", file=sys.stderr)
    print("  1. Add `pub mod <name>;` to the crate's lib.rs (and ship it), OR", file=sys.stderr)
    print("  2. Move it to `crates/<crate>/src/_archive/` and append a row to", file=sys.stderr)
    print("     `crates/_ARCHIVE_MANIFEST.md` documenting it as backlog.", file=sys.stderr)
    sys.exit(1)

print(f"✅ check-orphans: clean ({checked} files checked, 0 orphans).")
PY
