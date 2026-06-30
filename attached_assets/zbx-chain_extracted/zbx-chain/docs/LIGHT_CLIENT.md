# Light Client Guide

**Crate**: zbx-light  
**Proof type**: ZK state proofs (FRI-STARK)

---

## What is a Light Client?

A light client syncs only block headers (not full state). It uses ZK proofs to verify account balances and contract state without downloading gigabytes of data.

| Client Type | Data Downloaded | Verification |
|-------------|-----------------|--------------|
| Full node | 500 GB+ | Re-executes all blocks |
| Light client | ~10 MB (headers) | ZK proof verification |
| Ultra-light (browser) | ~100 KB | Single ZK proof |

---

## How ZBX Light Client Works

```
Block N header (32 bytes state root)
        │
        ▼
ZK State Proof for address X
  ├─ Account: balance=500, nonce=3
  ├─ Merkle path: leaf → ... → state_root
  └─ FRI-STARK proof: path is correct
        │
        ▼
Light client: verifies proof locally
  → "balance of X at block N = 500 ZBX" ✓
```

---

## Using the Light Client

### JavaScript (browser wallet)

```typescript
import { ZbxLightClient } from '@zbvix/sdk';

const client = new ZbxLightClient({
  endpoint: 'https://light.zbvix.com',
  trustLevel: 'full',  // verify every proof
});

// Verify balance without trusting the server
const proof = await client.getProof('0xYourAddress', 'latest');
if (await proof.verify()) {
  console.log(`Balance: \${proof.balance} ZBX`);
}
```

### Rust (embedded)

```rust
use zbx_light::{LightClient, SyncMode};

let client = LightClient::new("https://light.zbvix.com", SyncMode::Light)?;
client.sync().await?;

let proof = client.get_proof(address, None).await?;
let balance = proof.verify_account(&client.state_root())?;
```

---

## Proof Sizes and Verification Times

| Proof Type | Size | Verify Time |
|------------|------|-------------|
| Account state proof | ~2 KB | ~5ms |
| Storage slot proof | ~3 KB | ~8ms |
| Block proof | ~48 KB | ~50ms |
| Recursive (1000 blocks) | ~48 KB | ~50ms |