#!/usr/bin/env bash
# keygen.sh — Generate cryptographic keys for a new ZBX validator node.
#
# Generates:
#   1. Node identity key    (libp2p peer ID, secp256k1)
#   2. Validator key        (consensus signing, Ed25519)
#   3. BLS key              (aggregate signatures for HotStuff BFT)
#   4. Keystore file        (encrypted with password)
#
# Usage:
#   ./scripts/keygen.sh [--output-dir /path/to/keys] [--network testnet|mainnet]
#
# ⚠️  Keep all generated keys SECRET. Never commit to git.

set -euo pipefail

OUTPUT_DIR="${OUTPUT_DIR:-./keys}"
NETWORK="${NETWORK:-testnet}"
ZBX_BIN="${ZBX_BIN:-./target/release/zbx}"

# Audit M-19: hard-fail if the keygen binary is missing. The previous version
# fell through to silent fallbacks — `openssl ecparam` for the node key
# (wrong format), `dd if=/dev/urandom` for the validator + BLS keys (random
# bytes are *not* a valid Ed25519/BLS key without proper derivation), and a
# hard-coded JSON literal with no real encryption for the "keystore". A node
# booted with any of these fakes would either refuse to start or, far worse,
# silently sign with an all-zero / unencryptable key. Refuse to proceed.
if [[ ! -x "$ZBX_BIN" ]]; then
  echo "ERROR: zbx binary not found at $ZBX_BIN" >&2
  echo "       Build it first:  cargo build --release -p zbx-cli" >&2
  echo "       Or set ZBX_BIN=/path/to/zbx" >&2
  exit 1
fi

mkdir -p "$OUTPUT_DIR"
chmod 700 "$OUTPUT_DIR"

echo "Generating ZBX validator keys for $NETWORK..."
echo ""

# ── 1. Node identity key ─────────────────────────────────────────────────
echo "  [1/4] Node identity (secp256k1 / ENR)"
"$ZBX_BIN" key generate \
  --type secp256k1 \
  --output "$OUTPUT_DIR/node.key"
echo "  ✓ Node key: $OUTPUT_DIR/node.key"

# ── 2. Validator key ─────────────────────────────────────────────────────
echo "  [2/4] Validator key (Ed25519)"
"$ZBX_BIN" key generate \
  --type ed25519 \
  --output "$OUTPUT_DIR/validator.key"
echo "  ✓ Validator key: $OUTPUT_DIR/validator.key"

# ── 3. BLS key ───────────────────────────────────────────────────────────
echo "  [3/4] BLS key (BLS12-381)"
"$ZBX_BIN" key generate \
  --type bls \
  --output "$OUTPUT_DIR/bls.key"
echo "  ✓ BLS key: $OUTPUT_DIR/bls.key"

# ── 4. Encrypted keystore ────────────────────────────────────────────────
echo "  [4/4] Encrypted keystore"
echo "  Enter a strong password to encrypt the keystore:"
read -rs PASSWORD
echo ""
echo "  Confirm password:"
read -rs PASSWORD2
echo ""
[ "$PASSWORD" = "$PASSWORD2" ] || { echo "Passwords don't match."; exit 1; }
# Refuse trivially weak passwords on mainnet keys.
if [[ "$NETWORK" == "mainnet" && ${#PASSWORD} -lt 16 ]]; then
  echo "ERROR: mainnet keystore password must be ≥16 chars." >&2
  exit 1
fi

"$ZBX_BIN" key export-keystore \
  --key "$OUTPUT_DIR/validator.key" \
  --password "$PASSWORD" \
  --output "$OUTPUT_DIR/keystore.json"
echo "  ✓ Keystore: $OUTPUT_DIR/keystore.json"

chmod 600 "$OUTPUT_DIR"/*.key "$OUTPUT_DIR"/*.json 2>/dev/null || true

echo ""
echo "════════════════════════════════════════════"
echo "✓ Keys generated in $OUTPUT_DIR/"
echo ""
echo "CRITICAL: Back up these files securely."
echo "  - Do NOT commit them to version control."
echo "  - Store the keystore password in a password manager."
echo "  - Keep at least 2 encrypted backups in separate locations."
echo ""
echo "Your validator public key (for genesis):"
cat "$OUTPUT_DIR/validator.key" | head -1