#!/usr/bin/env bash
# =============================================================================
# genesis-fill.sh — Fill all PLACEHOLDER values in mainnet-genesis.json
# =============================================================================
# Run this BEFORE vps-setup.sh OR after keygen.sh to patch in real addresses.
#
# Usage:
#   bash deploy/genesis-fill.sh
#   bash deploy/genesis-fill.sh --genesis /path/to/mainnet-genesis.json
# =============================================================================

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BOLD='\033[1m'; NC='\033[0m'
log()  { echo -e "${GREEN}[ZBX]${NC} $*"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }

GENESIS="${1:-$(dirname "$0")/../config/mainnet-genesis.json}"

if [[ ! -f "$GENESIS" ]]; then
  echo -e "${RED}Error: genesis file not found: $GENESIS${NC}"; exit 1
fi

echo -e "${BOLD}"
cat << 'BANNER'
  ╔══════════════════════════════════════════╗
  ║   ZBX Chain — Genesis Fill Wizard        ║
  ║   Fill in all real addresses & keys      ║
  ╚══════════════════════════════════════════╝
BANNER
echo -e "${NC}"

# ── Helpers ────────────────────────────────────────────────────────────────

prompt_addr() {
  local label="$1" var_name="$2" optional="${3:-false}"
  local addr=""
  while true; do
    read -rp "  $label: " addr
    if [[ -z "$addr" && "$optional" == "true" ]]; then
      eval "$var_name=''"
      return
    fi
    if [[ "$addr" =~ ^0x[0-9a-fA-F]{40}$ ]]; then
      eval "$var_name='$addr'"
      return
    fi
    warn "Invalid Ethereum address. Must be 0x + 40 hex characters."
  done
}

prompt_bls() {
  local label="$1" var_name="$2"
  local key=""
  while true; do
    read -rp "  $label: " key
    if [[ "$key" =~ ^0x[0-9a-fA-F]{96,}$ ]]; then
      eval "$var_name='$key'"
      return
    fi
    warn "Invalid BLS key format. Must be 0x + 96+ hex chars."
  done
}

# ── Collect info ───────────────────────────────────────────────────────────

echo ""
echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD} GENESIS ALLOCATIONS (pre-minted balances)${NC}"
echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "  These are multisig/contract addresses that receive tokens at genesis."
echo "  Generate them with MetaMask, Gnosis Safe, or hardware wallet."
echo ""

prompt_addr "Foundation Multisig  (9,990,000 ZBX)  [0x...]" FOUNDATION_ADDR
prompt_addr "AMM Pool Contract    (20,000,000 ZBX)  [0x...]" AMM_ADDR
prompt_addr "Treasury/Governance  (5,000,000 ZBX)   [0x...]" TREASURY_ADDR
prompt_addr "Team Vesting Wallet  (3,000,000 ZBX)   [0x...]" TEAM_ADDR
prompt_addr "Ecosystem Grants     (2,000,000 ZBX)   [0x...]" ECOSYSTEM_ADDR

echo ""
echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD} GENESIS VALIDATORS${NC}"
echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "  These are the initial validator nodes. Run keygen.sh on each VPS first."
echo "  You need: validator Ethereum address + BLS public key per node."
echo ""

read -rp "  How many genesis validators? [1-10]: " VAL_COUNT
VAL_COUNT="${VAL_COUNT:-4}"

declare -a VAL_ADDRS VAL_BLSKEYS VAL_NAMES VAL_COMMISSIONS

for i in $(seq 1 "$VAL_COUNT"); do
  echo ""
  echo -e "  ${BOLD}Validator $i:${NC}"
  read -rp "    Name (e.g. 'Zebvix-Val-$i'): " VAL_NAMES[$i]
  VAL_NAMES[$i]="${VAL_NAMES[$i]:-Zebvix-Val-$i}"
  prompt_addr "    Ethereum address" VAL_ADDRS[$i]
  prompt_bls  "    BLS public key (from keygen.sh output)" VAL_BLSKEYS[$i]
  read -rp "    Commission % [default 5]: " comm
  VAL_COMMISSIONS[$i]=$(echo "${comm:-5} * 100" | bc | cut -d. -f1)
done

echo ""
echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BOLD} LAUNCH TIMESTAMP${NC}"
echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
read -rp "  Launch timestamp (YYYY-MM-DDTHH:MM:SSZ) [2025-06-01T00:00:00Z]: " LAUNCH_TS
LAUNCH_TS="${LAUNCH_TS:-2025-06-01T00:00:00Z}"

# ── Write the filled genesis ───────────────────────────────────────────────

echo ""
log "Writing filled genesis to $GENESIS..."

# Build validator JSON array
VALIDATORS_JSON=""
for i in $(seq 1 "$VAL_COUNT"); do
  [[ -n "$VALIDATORS_JSON" ]] && VALIDATORS_JSON+=",$'\n'"
  VALIDATORS_JSON+="    {
      \"address\":       \"${VAL_ADDRS[$i]}\",
      \"bls_pubkey\":    \"${VAL_BLSKEYS[$i]}\",
      \"stake\":         \"100000000000000000000000\",
      \"commission_bps\": ${VAL_COMMISSIONS[$i]},
      \"name\":          \"${VAL_NAMES[$i]}\"
    }"
done

python3 << PYEOF
import json

with open('$GENESIS') as f:
    g = json.load(f)

# Update timestamp
g['timestamp'] = '$LAUNCH_TS'

# Update validators
validators = []
names = '${VAL_NAMES[*]}'.split()
addrs = '${VAL_ADDRS[*]}'.split()
blskeys = '${VAL_BLSKEYS[*]}'.split()
comms = '${VAL_COMMISSIONS[*]}'.split()

for i in range(len(addrs)):
    validators.append({
        'address':       addrs[i],
        'bls_pubkey':    blskeys[i],
        'stake':         '100000000000000000000000',
        'commission_bps': int(comms[i]) if i < len(comms) else 500,
        'name':          names[i] if i < len(names) else f'Validator-{i+1}'
    })

g['validators'] = validators

# Update allocations
for alloc in g.get('allocations', []):
    c = alloc.get('comment','').lower()
    if 'foundation' in c:
        alloc['address'] = '$FOUNDATION_ADDR'
    elif 'amm' in c or 'liquidity' in c:
        alloc['address'] = '$AMM_ADDR'
    elif 'treasury' in c or 'governance' in c:
        alloc['address'] = '$TREASURY_ADDR'
    elif 'team' in c or 'vesting' in c:
        alloc['address'] = '$TEAM_ADDR'
    elif 'ecosystem' in c or 'grant' in c:
        alloc['address'] = '$ECOSYSTEM_ADDR'

with open('$GENESIS', 'w') as f:
    json.dump(g, f, indent=2)

print('Done!')
PYEOF

# Verify no placeholders remain
if grep -q "PLACEHOLDER\|0xVALIDATOR_\|0xFOUNDATION_\|0xLIQUIDITY_\|0xTREASURY_\|0xTEAM_\|0xECOSYSTEM_" "$GENESIS"; then
  warn "Some placeholders still remain in genesis. Check manually:"
  grep -n "PLACEHOLDER\|0xVALIDATOR_\|0xFOUNDATION_\|0xLIQUIDITY_\|0xTREASURY_\|0xTEAM_\|0xECOSYSTEM_" "$GENESIS" || true
else
  log "All placeholders replaced successfully!"
fi

echo ""
echo -e "${GREEN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${GREEN}${BOLD} Genesis file ready: $GENESIS${NC}"
echo -e "${GREEN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "  Next steps:"
echo "  1. Copy genesis to all validator VPS nodes"
echo "  2. Run: zbx-node init --genesis $GENESIS --config config/mainnet.toml"
echo "  3. Coordinate with all validators — agree on genesis hash"
echo "  4. Run: systemctl start zbx-mainnet"
echo ""

# Print filled genesis summary
echo -e "${BOLD}━━━━━━━━━━━ GENESIS SUMMARY ━━━━━━━━━━━${NC}"
echo "  Launch:      $LAUNCH_TS"
echo "  Validators:  $VAL_COUNT"
echo "  Foundation:  $FOUNDATION_ADDR"
echo "  AMM Pool:    $AMM_ADDR"
echo "  Treasury:    $TREASURY_ADDR"
echo "  Team:        $TEAM_ADDR"
echo "  Ecosystem:   $ECOSYSTEM_ADDR"
echo ""
