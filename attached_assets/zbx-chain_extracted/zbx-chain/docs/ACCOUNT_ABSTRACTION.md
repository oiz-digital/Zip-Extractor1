# Account Abstraction (ERC-4337) on ZBX Chain

**Version**: 0.2  
**Standard**: ERC-4337  
**Contracts**: ZbxEntryPoint, ZbxPaymaster, ZbxSmartWallet

---

## What is Account Abstraction?

Account abstraction (AA) replaces fixed EOA (Externally Owned Account) rules with programmable accounts. Instead of private-key-only signing, your account is a smart contract with custom validation logic.

### Problems AA solves:
| Problem | Traditional | With AA |
|---------|-------------|---------|
| Lost seed phrase | Funds locked forever | Social recovery via guardians |
| Gas payment | Must hold ZBX for gas | Paymaster pays, or pay in USDT |
| Batching | Multiple txs, multiple fees | One UserOperation = many actions |
| Security | Single key compromise = total loss | Multi-sig, hardware key, 2FA |

---

## Architecture

```
User
  │  Signs UserOperation (not a raw transaction)
  ▼
Bundler (off-chain)
  │  Submits batch of UserOps
  ▼
ZbxEntryPoint.handleOps()
  │
  ├─ validateUserOp()  →  ZbxSmartWallet.validateUserOp()
  │                              │
  │                              ├─ Verify signature (owner / session key)
  │                              └─ Verify nonce
  │
  ├─ (optional) validatePaymasterUserOp()  →  ZbxPaymaster
  │
  └─ Execute callData  →  ZbxSmartWallet.execute()
                                │
                                └─ Any contract call (DeFi, NFT, etc.)
```

---

## Deploying a Smart Wallet

```solidity
// Deploy via factory (counterfactual address — no tx until first use)
ZbxSmartWalletFactory factory = ZbxSmartWalletFactory(FACTORY_ADDRESS);
address wallet = factory.getAddress(ownerAddress, salt);
// wallet has a deterministic address before deployment!
```

---

## Gasless Transactions (Paymaster)

```typescript
import { ZbxBundler } from '@zbvix/sdk';

const bundler = new ZbxBundler({ rpc: 'https://rpc.zbvix.com' });

const userOp = {
  sender: walletAddress,
  callData: myContract.interface.encodeFunctionData('doSomething'),
  // Include paymaster to sponsor gas:
  paymasterAndData: PAYMASTER_ADDRESS + paymasterSignature,
};

const txHash = await bundler.sendUserOperation(userOp);
```

---

## Session Keys

Session keys allow temporary, limited signing permissions:

```solidity
// Add a session key for 24 hours, max 0.1 ZBX per call
wallet.addSessionKey(
    sessionKeyAddress,
    block.number + 17280,    // expires in 24h (at 5s blocks)
    0.1 ether,               // max value per call
    [gameContractAddress]    // only allowed to call this contract
);
```

---

## Social Recovery

```solidity
// Setup: add 3 guardians (friends/devices)
wallet.addGuardian(guardian1);
wallet.addGuardian(guardian2);
wallet.addGuardian(guardian3);
wallet.setRecoveryThreshold(2); // 2-of-3 needed

// If key is lost: guardian calls
wallet.executeRecovery(newOwnerAddress);  // from guardian's account
```