# ZBX SDK Developer Guide

**Package**: `@zbvix/sdk` (TypeScript/JavaScript)  
**Rust crate**: `zbx-sdk`  
**Version**: 0.1.0

---

## Installation

```bash
# TypeScript SDK
npm install @zbvix/sdk
# or
pnpm add @zbvix/sdk

# Rust crate (add to Cargo.toml)
[dependencies]
zbx-sdk = { path = "crates/zbx-sdk" }
```

---

## Quick Start

```typescript
import { ZbxProvider, ZbxWallet, ZbxContract } from '@zbvix/sdk';

// Connect to ZBX Chain
const provider = new ZbxProvider('https://rpc.zbvix.com');

// Check balance
const balance = await provider.getBalance('0xYourAddress');
console.log(`Balance: \${balance.toZBX()} ZBX`);

// Send a transaction
const wallet = ZbxWallet.fromPrivateKey(process.env.PRIVATE_KEY, provider);
const tx = await wallet.sendTransaction({
  to:    '0xRecipient',
  value: ZbxProvider.parseZBX('1.5'), // 1.5 ZBX
});
await tx.wait();
console.log(`Sent! Hash: \${tx.hash}`);
```

---

## Contract Interaction

```typescript
import { ZbxContract, ZRC20_ABI } from '@zbvix/sdk';

// ZRC-20 v1.1 — all standard methods + ZEP-006 extensions
const token = new ZbxContract(
    '0xTokenAddress',
    ZRC20_ABI,  // includes freeze/lock/mintFlags/batchTransfer ABIs
    wallet
);

// Standard reads
const bal = await token.balanceOf(wallet.address);
const info = await token.tokenInfo(); // name, symbol, decimals, supply, cap, logoURI, owner

// ZRC-20 v1.1 extensions (ZEP-006)
const frozen = await token.isFrozen('0xAddress');           // §3.1 freeze
const locked = await token.lockedBalanceOf(wallet.address); // §3.2 native lock
const canSend = await token.transferableBalance(wallet.address);

// Write
const tx = await token.transfer('0xRecipient', 100_000_000n);
await tx.wait();

// Batch transfer (up to 512 recipients)
const batchTx = await token.batchTransfer(
    ['0xAddr1', '0xAddr2'],
    [50_000_000n, 50_000_000n],
);
await batchTx.wait();
```

---

## Account Abstraction (ERC-4337)

```typescript
import { ZbxBundler, ZbxSmartWallet } from '@zbvix/sdk';

const bundler = new ZbxBundler({
    entryPoint: '0xZbxEntryPoint_Address',
    bundlerRpc: 'https://bundler.zbvix.com',
});

// Create/load smart wallet
const wallet = await ZbxSmartWallet.create(provider, ownerSigner);

// Send gasless tx (paymaster pays)
const userOpHash = await bundler.sendUserOp({
    sender:   wallet.address,
    callData: wallet.encodeCall('transfer', [recipient, amount]),
    paymaster: PAYMASTER_ADDRESS,
});
```

---

## ZK State Proofs (Light Client)

```typescript
import { ZbxLightClient } from '@zbvix/sdk';

const lightClient = new ZbxLightClient('https://light.zbvix.com');

// Verify balance without trusting the server
const proof = await lightClient.getProof(address, blockNumber);
const isValid = await proof.verify();
console.log(`Balance verified: \${proof.balance} wei`);
```

---

## Rust SDK Usage

```rust
use zbx_sdk::{Provider, Wallet, ZRC20};
// ZRC-20 v1.1 runtime types (zbx-contracts crate — ZEP-006)
use zbx_contracts::{Zrc20Token, TokenInfo, LockInfo, DEFAULT_DECIMALS};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = Provider::new("https://rpc.zbvix.com")?;
    let wallet   = Wallet::from_env("PRIVATE_KEY")?.connect(&provider);

    // Get balance
    let balance = provider.get_balance(wallet.address(), None).await?;
    println!("Balance: {} ZBX", balance.to_zbx());

    // Send ZBX
    let tx = wallet.send_transaction(
        "0xRecipient".parse()?,
        zbx_sdk::parse_zbx("1.5")?,
    ).await?;
    println!("Tx: {}", tx.hash);

    Ok(())
}
```

---

## RPC Methods Reference

| Method | Description |
|--------|-------------|
| `eth_getBalance` | Account ZBX balance |
| `eth_sendRawTransaction` | Submit signed tx |
| `eth_call` | Call contract (no state change) |
| `eth_getBlockByNumber` | Block data |
| `zbx_getStateProof` | ZK state proof for address |
| `zbx_getBlockProof` | ZK block execution proof |
| `zbx_sendPrivateTransaction` | MEV-protected tx submission |
| `zbx_feeHistory` | EIP-1559 fee history |