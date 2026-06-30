# Contract Upgrade Guide

**Pattern**: UUPS (Universal Upgradeable Proxy Standard)  
**Contract**: ZbxProxy.sol  
**Timelock**: 48 hours on mainnet, 0 hours on devnet

---

## When to Upgrade

- Bug fix in contract logic
- Feature addition (e.g., new LP fee tier)
- Gas optimisation
- Parameter change (e.g., new interest rate model)

---

## Upgrade Process

### 1. Write the new implementation

```solidity
contract ZbxOracleV2 is ZbxOracle {
    // New feature: multi-source TWAP
    function getTwapPrice(address asset, uint256 period) external view override returns (uint256) {
        // ... new implementation ...
    }
}
```

### 2. Deploy new implementation (NOT proxy)

```bash
forge create contracts/ZbxOracleV2.sol:ZbxOracleV2 \\
  --rpc-url $ZBX_MAINNET_RPC \\
  --private-key $PRIVATE_KEY
# → Deployed to: 0xNewImpl...
```

### 3. Test upgrade on devnet first

```bash
./scripts/upgrade-contracts.sh devnet ZbxOracle 0xNewImpl...
```

### 4. Schedule upgrade via governance (mainnet)

```bash
./scripts/upgrade-contracts.sh mainnet ZbxOracle 0xNewImpl...
# → Scheduled (48h delay)
# → Execute after: 2025-01-03 14:00 UTC
```

### 5. Execute after timelock (mainnet)

```bash
cast send $TIMELOCK_ADDRESS "execute(address,uint256,bytes,bytes32,bytes32)" \\
  $PROXY_ADDRESS 0 $UPGRADE_CALLDATA 0x0 0x0 \\
  --rpc-url $ZBX_MAINNET_RPC --private-key $PRIVATE_KEY
```

---

## Storage Layout Rules

When upgrading, you MUST preserve the storage layout:

```solidity
// ✅ Safe: add new vars at the end
contract ZbxOracleV2 is ZbxOracle {
    address public newFeature;  // new slot: OK
}

// ❌ UNSAFE: inserting or removing vars
contract ZbxOracleV2 is ZbxOracle {
    uint256 public inserted; // shifts all subsequent slots!
    address public owner;    // was at slot 0, now at slot 1 — CORRUPTS STATE
}
```

Use the `@custom:oz-upgrades-unsafe-allow` annotation for intentional exceptions.