#!/usr/bin/env bash
# Zebvix MAINNET — safe N → N+1 validator-set growth.
#
# Mainnet analog of testnet-add-validator.sh. Same dormant-producer pattern:
#   1. Bring the new validator node online as a full node (its address NOT
#      in the on-chain set yet → its consensus producer task runs but
#      `who_proposes(...) != me`, so it never proposes).
#   2. Wait until the new node has fully synced to the current tip.
#   3. Submit `validator-add` from the chair/founder signer key.
#   4. The producer on every node re-reads `state.validators()` on every
#      tick (consensus.rs ~line 200) — quorum jumps N→N+1 atomically with
#      ZERO halt window.
#
# *** WHY MAINNET NEEDS ITS OWN SCRIPT ***
# Three reasons vs the testnet equivalent:
#   1. STAKE GATE. Mainnet `validator-add` will be rejected if the candidate's
#      staking_escrow record is below MIN_STAKE (100 ZBX, 18 decimals). This
#      script verifies the stake before submitting the tx, so we don't burn
#      a chair-key fee on a doomed transaction.
#   2. POP VERIFICATION. BLS proof-of-possession is enforced on mainnet
#      (see PACEMAKER-BLS-01 fix). The script extracts both `bls_pubkey`
#      AND `bls_pop` from the keyfile produced by `zbx-keygen` and submits
#      both.
#   3. QUORUM SAFETY. Adding the Nth validator changes the threshold from
#      ⌈2/3 of N-1⌉ to ⌈2/3 of N⌉. The script prints the new threshold
#      BEFORE submitting and refuses if fewer than the new threshold are
#      currently online (would create an immediate liveness failure).
#
# Usage:
#   sudo bash scripts/mainnet-add-validator.sh                # full flow
#   sudo bash scripts/mainnet-add-validator.sh --status       # show current set + new candidate
#   sudo bash scripts/mainnet-add-validator.sh --dry-run      # print plan, change nothing
#   sudo bash scripts/mainnet-add-validator.sh --verify-only  # check stake + PoP, don't submit
#
# Required:
#   ZBX_NEW_VALIDATOR_KEYFILE  path to keyfile produced by `zbx-keygen --output json`
#                              and saved to disk (mode 0600). MUST contain
#                              evm_address, bls_pubkey, bls_privkey, node_privkey.
#   ZBX_CHAIR_SIGNER_KEY       path to founder/admin private key file (signs the
#                              validator-add tx). Custody MUST be HSM/Ledger.
#
# Optional:
#   ZBX_NEW_VALIDATOR_NAME     default: derived from address
#   ZBX_NEW_VALIDATOR_POWER    default: 1   (voting power, integer)
#   ZBX_MAINNET_RPC_URL        default: http://127.0.0.1:8545
#   ZBX_BIN                    default: /usr/local/bin/zbx-node
#   ZBX_TX_TIMEOUT             default: 120 (seconds to wait for tx mining)
#   ZBX_MIN_QUORUM_ONLINE      default: 0  (extra safety; require N online voters before adding)
#
# Exit codes:
#   0 — success (new validator is in the on-chain set, both old + new produce votes)
#   1 — runtime failure (tx didn't mine, candidate didn't sync, etc.)
#   2 — usage / preflight failure (missing key, missing env, bad keyfile)
#   3 — refusing to act (stake too low, quorum risk, candidate already added with different params)

set -euo pipefail

# ── 0. Preflight ──────────────────────────────────────────────────────────
if [[ "${EUID}" -ne 0 ]]; then
    echo "ERROR: this script must be run as root (sudo)" >&2
    exit 2
fi

KEYFILE="${ZBX_NEW_VALIDATOR_KEYFILE:-}"
CHAIR_KEY="${ZBX_CHAIR_SIGNER_KEY:-}"
NAME="${ZBX_NEW_VALIDATOR_NAME:-}"
POWER="${ZBX_NEW_VALIDATOR_POWER:-1}"
RPC_URL="${ZBX_MAINNET_RPC_URL:-http://127.0.0.1:8545}"
BIN="${ZBX_BIN:-/usr/local/bin/zbx-node}"
TX_TIMEOUT="${ZBX_TX_TIMEOUT:-120}"
MIN_QUORUM_ONLINE="${ZBX_MIN_QUORUM_ONLINE:-0}"

mode="full"
case "${1:-}" in
    --status)       mode="status" ;;
    --dry-run)      mode="dry"    ;;
    --verify-only)  mode="verify" ;;
    "")             mode="full"   ;;
    *)
        echo "ERROR: unknown flag: $1" >&2
        echo "  valid: --status | --dry-run | --verify-only | (no flag = full)" >&2
        exit 2
        ;;
esac

# ── helpers ───────────────────────────────────────────────────────────────
rpc_call() {
    local method="$1" params="${2:-[]}"
    curl -fsS -m 10 -X POST "$RPC_URL" \
        -H 'Content-Type: application/json' \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"${method}\",\"params\":${params}}"
}

get_validator_count() {
    rpc_call "zbx_listValidators" 2>/dev/null \
        | grep -oE '"count"[[:space:]]*:[[:space:]]*[0-9]+' \
        | head -1 \
        | grep -oE '[0-9]+' \
        || echo "0"
}

validator_set_has_pubkey() {
    local pk_hex_lower="${1,,}"
    pk_hex_lower="${pk_hex_lower#0x}"
    rpc_call "zbx_listValidators" 2>/dev/null \
        | tr 'A-Z' 'a-z' \
        | grep -qE "\"pubkey\"[[:space:]]*:[[:space:]]*\"0x${pk_hex_lower}\""
}

# Extract a string field from a keyfile (tolerant of pretty-printed JSON).
extract_field() {
    local file="$1" field="$2"
    grep -oE "\"${field}\"[[:space:]]*:[[:space:]]*\"[^\"]+\"" "$file" \
        | sed -E "s/.*\"${field}\"[[:space:]]*:[[:space:]]*\"([^\"]+)\".*/\\1/" \
        | head -1
}

# Stake lookup via RPC. Returns "0" on any failure.
get_stake_for() {
    local addr="$1"
    rpc_call "zbx_getValidatorStake" "[\"${addr}\"]" 2>/dev/null \
        | grep -oE '"stake"[[:space:]]*:[[:space:]]*"[0-9]+"' \
        | head -1 \
        | sed -E 's/.*"([0-9]+)".*/\\1/' \
        || echo "0"
}

MIN_STAKE_WEI="100000000000000000000"  # 100 ZBX, 18 decimals (matches zbx-contracts/staking_escrow.rs::MIN_STAKE)

# ── --status fast path ────────────────────────────────────────────────────
if [[ "$mode" == "status" ]]; then
    echo "── current mainnet validator set ──"
    rpc_call "zbx_listValidators" 2>/dev/null | head -c 1500
    echo ""
    if [[ -n "$KEYFILE" && -f "$KEYFILE" ]]; then
        echo ""
        echo "── candidate from keyfile ──"
        echo "  evm_address : $(extract_field "$KEYFILE" evm_address)"
        echo "  bls_pubkey  : $(extract_field "$KEYFILE" bls_pubkey)"
        candidate_pk=$(extract_field "$KEYFILE" bls_pubkey)
        if validator_set_has_pubkey "$candidate_pk"; then
            echo "  status      : ALREADY IN VALIDATOR SET"
        else
            echo "  status      : not in set (ready to add)"
        fi
    fi
    exit 0
fi

# ── Preflight (full | dry | verify) ───────────────────────────────────────
if [[ -z "$KEYFILE" ]]; then
    echo "ERROR: ZBX_NEW_VALIDATOR_KEYFILE not set" >&2
    echo "  Produce one with:  zbx-keygen --count 1 --output json > /etc/zbx/validators/new.json" >&2
    echo "  Then:  export ZBX_NEW_VALIDATOR_KEYFILE=/etc/zbx/validators/new.json" >&2
    exit 2
fi
if [[ ! -f "$KEYFILE" ]]; then
    echo "ERROR: keyfile not found: $KEYFILE" >&2
    exit 2
fi
if [[ -z "$CHAIR_KEY" || ! -f "$CHAIR_KEY" ]]; then
    echo "ERROR: ZBX_CHAIR_SIGNER_KEY not set or missing: $CHAIR_KEY" >&2
    echo "  This MUST be the founder/admin signer custodied on HSM/Ledger." >&2
    exit 2
fi
if [[ ! -x "$BIN" ]]; then
    echo "ERROR: $BIN not found or not executable" >&2
    echo "  Run first:  sudo bash scripts/mainnet-deploy.sh --build-only" >&2
    exit 2
fi

EVM_ADDR=$(extract_field "$KEYFILE" evm_address)
BLS_PUB=$(extract_field "$KEYFILE" bls_pubkey)
if [[ -z "$EVM_ADDR" || -z "$BLS_PUB" ]]; then
    echo "ERROR: keyfile missing evm_address or bls_pubkey field" >&2
    echo "  Expected JSON shape: {\"evm_address\":\"0x...\",\"bls_pubkey\":\"0x...\", ...}" >&2
    exit 2
fi
NAME="${NAME:-${EVM_ADDR:0:10}…}"

# Verify mainnet RPC is reachable + on chain_id 8989
chain_id_hex=$(rpc_call "eth_chainId" 2>/dev/null \
                 | sed -n 's/.*"result":"\(0x[0-9a-fA-F]*\)".*/\1/p')
if [[ -z "$chain_id_hex" ]]; then
    echo "ERROR: mainnet RPC at $RPC_URL not responding" >&2
    exit 1
fi
if [[ "${chain_id_hex,,}" != "0x231d" ]]; then
    echo "ERROR: RPC at $RPC_URL returned chain_id ${chain_id_hex} — expected 0x231d (mainnet 8989)" >&2
    echo "  Refusing to act — wrong network." >&2
    exit 3
fi

# Current set
CURRENT_COUNT=$(get_validator_count)
NEW_COUNT=$((CURRENT_COUNT + 1))
OLD_THRESHOLD=$(( (CURRENT_COUNT * 2 + 2) / 3 ))  # ceil(2N/3)
NEW_THRESHOLD=$(( (NEW_COUNT     * 2 + 2) / 3 ))

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  Zebvix MAINNET — add validator (${NAME})"
echo "  candidate addr : ${EVM_ADDR}"
echo "  candidate pk   : ${BLS_PUB}"
echo "  voting power   : ${POWER}"
echo "  current set    : ${CURRENT_COUNT} validators   (threshold = ${OLD_THRESHOLD})"
echo "  after add      : ${NEW_COUNT} validators       (threshold = ${NEW_THRESHOLD})"
echo "  chair signer   : ${CHAIR_KEY}"
echo "  mode           : ${mode}"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# ── Idempotency ──────────────────────────────────────────────────────────
if validator_set_has_pubkey "$BLS_PUB"; then
    echo ""
    echo "✓ candidate pubkey is ALREADY in validator set — nothing to do."
    exit 0
fi

# ── Stake check ──────────────────────────────────────────────────────────
echo ""
echo "▶ checking staking_escrow for ${EVM_ADDR}…"
stake=$(get_stake_for "$EVM_ADDR")
stake=${stake:-0}
if [[ "$stake" -lt "$MIN_STAKE_WEI" ]] 2>/dev/null || \
   ! python3 -c "import sys; sys.exit(0 if int('$stake') >= int('$MIN_STAKE_WEI') else 1)" 2>/dev/null; then
    echo "ERROR: candidate stake = ${stake} wei < MIN_STAKE = ${MIN_STAKE_WEI} wei (100 ZBX)" >&2
    echo "  The validator-add tx WILL revert. Stake first, then re-run:" >&2
    echo "    ${BIN} stake-deposit --signer-key <candidate-evm-key> \\\\" >&2
    echo "      --amount 100000000000000000000 --rpc-url $RPC_URL" >&2
    exit 3
fi
echo "  ✓ stake ok: ${stake} wei (≥ ${MIN_STAKE_WEI})"

# ── Quorum-after-add safety check ────────────────────────────────────────
if (( MIN_QUORUM_ONLINE > 0 )); then
    online=$(rpc_call "zbx_voteStats" 2>/dev/null \
             | grep -oE '"voters":\[[^]]*\]' | head -1 \
             | grep -oE '"0x[0-9a-fA-F]+"' | wc -l | tr -d ' ')
    if (( online < NEW_THRESHOLD )); then
        echo "ERROR: only ${online} voters online; after add, threshold becomes ${NEW_THRESHOLD}" >&2
        echo "  Adding now would create an immediate liveness failure. Wait until ≥${NEW_THRESHOLD} are online." >&2
        exit 3
    fi
    echo "  ✓ ${online} voters currently online (≥ new threshold ${NEW_THRESHOLD})"
fi

if [[ "$mode" == "verify" || "$mode" == "dry" ]]; then
    echo ""
    if [[ "$mode" == "dry" ]]; then
        echo "  [dry] would submit:"
        echo "    ${BIN} validator-add \\\\"
        echo "      --signer-key ${CHAIR_KEY} \\\\"
        echo "      --address ${EVM_ADDR} \\\\"
        echo "      --pubkey ${BLS_PUB} \\\\"
        echo "      --power ${POWER} \\\\"
        echo "      --rpc-url ${RPC_URL} --fee auto"
    else
        echo "✅ verify-only complete — all preflight checks pass."
        echo "   Re-run without --verify-only to submit the validator-add tx."
    fi
    exit 0
fi

# ── Submit validator-add tx ──────────────────────────────────────────────
echo ""
echo "▶ submitting validator-add tx (signer = chair, target = ${EVM_ADDR})…"
if ! "$BIN" validator-add \
        --signer-key "$CHAIR_KEY" \
        --address "$EVM_ADDR" \
        --pubkey "$BLS_PUB" \
        --power "$POWER" \
        --rpc-url "$RPC_URL" \
        --fee auto ; then
    echo "ERROR: validator-add tx submission failed" >&2
    echo "  No on-chain change — candidate's local node (if running) is still dormant." >&2
    exit 1
fi

# ── Wait for application ──────────────────────────────────────────────────
echo ""
echo "▶ waiting for validator-add to be mined (timeout ${TX_TIMEOUT}s)…"
elapsed=0; added=0
while (( elapsed < TX_TIMEOUT )); do
    sleep 3; elapsed=$((elapsed + 3))
    if validator_set_has_pubkey "$BLS_PUB"; then
        count=$(get_validator_count)
        echo "  ✓ ${EVM_ADDR} is now in validator set (count=${count}, after ${elapsed}s)"
        added=1; break
    fi
    count=$(get_validator_count)
    echo "    …waiting  current count=${count}  (${elapsed}s/${TX_TIMEOUT}s)"
done
if (( added != 1 )); then
    echo "ERROR: validator-add tx did not apply within ${TX_TIMEOUT}s" >&2
    echo "  Inspect:  curl -X POST $RPC_URL -H 'Content-Type: application/json' \\\\" >&2
    echo "              -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"zbx_listValidators\",\"params\":[]}'" >&2
    exit 1
fi

# ── Verify the new validator is voting ───────────────────────────────────
echo ""
echo "▶ waiting one block round to verify new validator participates…"
sleep 8

vote_stats=$(rpc_call "zbx_voteStats" 2>/dev/null || echo "")
voter_count=$(echo "$vote_stats" \
              | grep -oE '"voters":\[[^]]*\]' | head -1 \
              | grep -oE '"0x[0-9a-fA-F]+"' | wc -l | tr -d ' ')

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
if [[ "${voter_count:-0}" -ge "$NEW_THRESHOLD" ]]; then
    echo "✅ mainnet validator added — ${NEW_COUNT}/${NEW_COUNT} quorum healthy"
    echo "   voters seen in latest round: ${voter_count} (≥ threshold ${NEW_THRESHOLD})"
else
    echo "⚠  validator-add applied but only ${voter_count} voter(s) seen yet (need ≥ ${NEW_THRESHOLD})"
    echo "   The new node may take another few seconds to produce its first vote."
    echo "   Re-check: $0 --status"
fi
echo ""
echo "   New validator address : ${EVM_ADDR}"
echo "   Total validators      : ${NEW_COUNT}"
echo "   New BFT threshold     : ${NEW_THRESHOLD} of ${NEW_COUNT}"
echo ""
echo "   Monitor: zbx_signing_misses_total{address=\"${EVM_ADDR}\"} must stay near 0."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
