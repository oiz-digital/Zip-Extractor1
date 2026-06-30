#!/usr/bin/env bash
# deploy-contracts.sh — Deploy all ZBX Chain smart contracts to a target network.
#
# Usage:
#   ./scripts/deploy-contracts.sh [network] [options]
#
# Networks:
#   devnet     — Local development network (default, Chain ID 8990)
#   testnet    — ZBX Public Testnet (Chain ID 8990)
#   mainnet    — ZBX Mainnet (Chain ID 8989, requires hardware key)
#
# Chain IDs MUST match `crates/zbx-types/src/lib.rs::CHAIN_ID_MAINNET = 8989`
# (and CHAIN_ID + 1 = 8990 for testnet/devnet). Do NOT introduce a fourth
# value here.
#
# Examples:
#   ./scripts/deploy-contracts.sh devnet
#   ./scripts/deploy-contracts.sh testnet --verify
#   ./scripts/deploy-contracts.sh mainnet --verify --ledger

set -euo pipefail

NETWORK=\${1:-devnet}
SHIFT=0; [ $# -ge 1 ] && SHIFT=1
shift \$SHIFT || true

# ── Configuration ─────────────────────────────────────────────────────────
case "\$NETWORK" in
  devnet)
    RPC="http://localhost:8545"
    CHAIN_ID=8990
    PRIVATE_KEY="\${DEVNET_PRIVATE_KEY:-0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80}"
    ;;
  testnet)
    RPC="\${ZBX_TESTNET_RPC:-https://testnet-rpc.zbvix.com}"
    CHAIN_ID=8990
    PRIVATE_KEY="\${TESTNET_PRIVATE_KEY:?TESTNET_PRIVATE_KEY not set}"
    ;;
  mainnet)
    RPC="\${ZBX_MAINNET_RPC:-https://rpc.zbvix.com}"
    CHAIN_ID=8989
    PRIVATE_KEY="\${MAINNET_PRIVATE_KEY:?MAINNET_PRIVATE_KEY not set — use hardware wallet}"
    echo "⚠️  MAINNET DEPLOYMENT — double-check everything before proceeding"
    read -rp "Type 'yes I am sure' to continue: " CONFIRM
    [ "\$CONFIRM" = "yes I am sure" ] || { echo "Aborted."; exit 1; }
    ;;
  *)
    echo "Unknown network: \$NETWORK"; exit 1 ;;
esac

echo "Deploying to \$NETWORK (Chain ID \$CHAIN_ID, RPC \$RPC)"

# ── Deploy order (respects dependencies) ─────────────────────────────────
# 1. Libraries (no deps)
echo "[1/8] Deploying libraries..."
FIXED_POINT=\$(forge create contracts/libraries/FixedPoint.sol:FixedPoint \\
  --rpc-url "\$RPC" --private-key "\$PRIVATE_KEY" --json | jq -r '.deployedTo')
echo "  FixedPoint: \$FIXED_POINT"

# 2. ZbxOracle (needs governance addr → use deployer for now)
echo "[2/8] Deploying ZbxOracle..."
ORACLE=\$(forge create contracts/ZbxOracle.sol:ZbxOracle \\
  --rpc-url "\$RPC" --private-key "\$PRIVATE_KEY" \\
  --constructor-args "\$(cast wallet address \$PRIVATE_KEY)" \\
  --json | jq -r '.deployedTo')
echo "  ZbxOracle: \$ORACLE"

# 3. ZbxVerifier (ZK proof verifier)
echo "[3/8] Deploying ZbxVerifier..."
VERIFIER=\$(forge create contracts/ZbxVerifier.sol:ZbxVerifier \\
  --rpc-url "\$RPC" --private-key "\$PRIVATE_KEY" \\
  --constructor-args "0x0000000000000000000000000000000000000000" \\
  --json | jq -r '.deployedTo')
echo "  ZbxVerifier: \$VERIFIER"

# 4. EntryPoint (ERC-4337 Account Abstraction)
echo "[4/8] Deploying ZbxEntryPoint..."
ENTRY_POINT=\$(forge create contracts/ZbxEntryPoint.sol:ZbxEntryPoint \\
  --rpc-url "\$RPC" --private-key "\$PRIVATE_KEY" \\
  --json | jq -r '.deployedTo')
echo "  ZbxEntryPoint: \$ENTRY_POINT"

# 5. ZbxGovernor + ZbxTimelock
echo "[5/8] Deploying governance..."
TIMELOCK=\$(forge create contracts/ZbxTimelock.sol:ZbxTimelock \\
  --rpc-url "\$RPC" --private-key "\$PRIVATE_KEY" \\
  --constructor-args 172800 "[]" "[]" \\
  --json | jq -r '.deployedTo')
echo "  ZbxTimelock: \$TIMELOCK"

# 6. ZBX20 token ecosystem
echo "[6/8] Deploying ZBX20 tokens..."
WZBX=\$(forge create contracts/tokens/WZBX.sol:WZBX \\
  --rpc-url "\$RPC" --private-key "\$PRIVATE_KEY" \\
  --json | jq -r '.deployedTo')
echo "  WZBX: \$WZBX"

# 7. AMM + Router
echo "[7/8] Deploying AMM infrastructure..."
DEPLOYER="\$(cast wallet address \$PRIVATE_KEY)"
# ZbxAMM factory placeholder address
AMM_FACTORY="\$DEPLOYER"
ROUTER=\$(forge create contracts/ZbxRouter.sol:ZbxRouter \\
  --rpc-url "\$RPC" --private-key "\$PRIVATE_KEY" \\
  --constructor-args "\$AMM_FACTORY" "\$WZBX" \\
  --json | jq -r '.deployedTo')
echo "  ZbxRouter: \$ROUTER"

# 8. BridgeVault + BridgeMultisig
echo "[8/8] Deploying bridge..."
# (relayer keys should be pre-generated with scripts/testnet-genesis-keygen.sh)

# ── Save addresses ─────────────────────────────────────────────────────────
ADDR_FILE="deployments/\$NETWORK-\$(date +%Y%m%d-%H%M%S).json"
mkdir -p deployments
cat > "\$ADDR_FILE" << JSON
{
  "network":      "\$NETWORK",
  "chain_id":     \$CHAIN_ID,
  "deployed_at":  "\$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "contracts": {
    "FixedPoint":    "\$FIXED_POINT",
    "ZbxOracle":     "\$ORACLE",
    "ZbxVerifier":   "\$VERIFIER",
    "ZbxEntryPoint": "\$ENTRY_POINT",
    "ZbxTimelock":   "\$TIMELOCK",
    "WZBX":          "\$WZBX",
    "ZbxRouter":     "\$ROUTER"
  }
}
JSON

echo ""
echo "✓ Deployment complete. Addresses saved to \$ADDR_FILE"

# ── Verify on explorer (if --verify flag) ─────────────────────────────────
if [[ " \$* " == *" --verify "* ]]; then
  echo "Verifying contracts on ZBX Explorer..."
  ./scripts/verify-contracts.sh "\$ADDR_FILE"
fi