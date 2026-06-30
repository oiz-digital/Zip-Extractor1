# ZEP-001: Pay ID — UPI-style Human-Readable Addresses

| Field         | Value                                              |
|---------------|----------------------------------------------------|
| **ZEP**       | 001                                                |
| **Title**     | Pay ID — UPI-style Human-Readable Wallet Addresses |
| **Author**    | Zebvix Core Team                                   |
| **Status**    | ACCEPTED                                           |
| **Category**  | Standard                                           |
| **Created**   | Block 0 (Genesis)                                  |
| **Activation**| Block 50,000                                       |
| **Contract**  | `0x7e4a7f8bCE8CfD1765CdE34Dbf5e8D7fB1A43e9`     |

---

## Summary

ZEP-001 ZBX Chain pe **UPI-style Pay IDs** introduce karta hai.

Iske baad users seedha `ali@zbx` pe ZBX ya ZUSD bhej sakte hain —
64-character hex address `0x742d35Cc6634C0532925a3b844Bc454e4438f44e` yaad karne ki zaroorat nahi.

---

## Motivation

Aaj ke blockchain wallets mein ek badi problem hai: addresses unreadable hain.

| Problem       | Example                                              |
|---------------|------------------------------------------------------|
| Ugly address  | `0x742d35Cc6634C0532925a3b844Bc454e4438f44e`        |
| Human error   | Ek char galat → funds gaye                           |
| Not memorable | Dost ko address batana mushkil                       |
| No verification | Kaise pata yahi sahi address hai?                  |

UPI ne India mein ye problem solve ki bank accounts ke liye:
- `ali@upi` yaad rakhna easy hai
- Copy-paste error ka dar kam
- Human-friendly aur verifiable

ZBX Pay ID wahi idea blockchain pe laata hai.

---

## Specification

### Format

```
[name]@zbx

name:
  - 3 se 32 characters
  - Allowed: a-z, 0-9, hyphen (-)
  - No leading/trailing hyphen
  - Case-insensitive (internally lowercase)

@zbx:
  - Fixed network suffix
  - ZBX mainnet (Chain ID 8989)
```

### Examples

```
ali@zbx              ← simple personal wallet
my-shop@zbx          ← merchant wallet
trader123@zbx        ← trader's wallet
shop.ali@zbx         ← sub-ID: Ali ki shop (issued by ali)
treasury.zebvix@zbx  ← Zebvix company treasury sub-ID
```

### Resolution Flow

```
Step 1: User enters "ali@zbx" in wallet app

Step 2: App calls PayIdResolver.resolve("ali@zbx")
        ↓
        Parse: name="ali", handle="zbx"
        ↓
        Strip suffix: "ali"

Step 3: On-chain call:
        ZbxPayId.resolve("ali") → 0x742d35Cc...

Step 4: Return address to app

Step 5: User confirms → transaction sent to 0x742d35Cc...
```

### Smart Contract Interface

```solidity
interface IZbxPayId {
    // Register karo: register("ali@zbx", myWallet)
    function register(string calldata payId, address wallet) external payable;

    // Resolve karo: resolve("ali@zbx") → 0x742d...
    function resolve(string calldata payId) external view returns (address);

    // Reverse: reverseLookup(0x742d...) → "ali@zbx"
    function reverseLookup(address wallet) external view returns (string memory);

    // Multi-chain: ETH/BTC address bhi link karo
    function setChainAddress(string calldata payId, uint256 chainId, string calldata addr) external;

    // Sub-ID: issueSubId("ali", "shop", shopWallet) → shop.ali@zbx
    function issueSubId(string calldata parent, string calldata sub, address to) external;
}
```

### Registration Fee

```
Fee: 0.01 ZBX (one-time, non-refundable)
Purpose: Prevent mass name squatting
Fee goes to: ZBX Protocol Treasury
```

### Sub-IDs

Parent Pay ID owner sub-IDs issue kar sakta hai **free mein**:

```
zebvix@zbx registers karke:
  → treasury.zebvix@zbx  (company treasury)
  → ops.zebvix@zbx       (operations)
  → customer1.zebvix@zbx (issued to a customer)
```

---

## Implementation

### On-chain: `ZbxPayId.sol`
- Registry contract: stores name → address mapping
- Fully on-chain — no DNS, no central server
- Immutable mappings (owner can update, but history on-chain)
- Events: `PayIdRegistered`, `PayIdUpdated`, `PayIdTransferred`

### Off-chain: `zbx-payid` crate (Rust)
- `PayIdResolver`: resolves Pay IDs via RPC
- `PayIdRegistry`: in-memory cache (5-min TTL)
- `parse_pay_id()`: validates and parses any format

---

## Multi-chain Support

Ek `ali@zbx` ke under alag chains ke addresses store ho sakte hain:

```
ali@zbx → ZBX (8989):  0x742d35Cc...    [primary]
ali@zbx → ETH (1):     0x742d35Cc...    [optional]
ali@zbx → BTC:         bc1qxy2kg...     [optional]
```

Isse "universal payment handle" ban jata hai — ek address sabke liye.

---

## Security Considerations

| Risk                  | Mitigation                                              |
|-----------------------|---------------------------------------------------------|
| Name squatting        | 0.01 ZBX registration fee                               |
| Phishing (fake IDs)   | UI mein verified badge (like Twitter blue tick)         |
| Front-running         | Commit-reveal scheme (future ZEP)                       |
| Owner key compromise  | Link with AA smart wallet (multi-sig recovery)          |
| Centralized resolver  | Fully on-chain — no single point of failure             |

---

## Backwards Compatibility

ZEP-001 naye smart contract introduce karta hai — existing wallets ya transactions pe koi effect nahi.

Purani 0x addresses kaam karte rahenge. Pay IDs optional convenience feature hain.

---

## Test Cases

```
parse_pay_id("ali")         → name="ali",      canonical="ali@zbx"  ✓
parse_pay_id("ali@zbx")     → name="ali",      canonical="ali@zbx"  ✓
parse_pay_id("ALI@ZBX")     → name="ali",      canonical="ali@zbx"  ✓ (normalized)
parse_pay_id("shop.ali@zbx")→ sub-ID, parent="ali"                  ✓
parse_pay_id("ab@zbx")      → ERROR: too short                       ✓
parse_pay_id("ali@eth")     → ERROR: unsupported handle              ✓
parse_pay_id("0x742d...")   → ERROR: not a Pay ID                    ✓
```

---

## Activation

- **Testnet**: Block 10,000
- **Mainnet**: Block 50,000 (~2.9 days after genesis @ 5s/block)
- **Contract deploy**: Before block 49,990

---

## Reference Implementation

- Contract: `contracts/ZbxPayId.sol`
- Interface: `contracts/interfaces/IZbxPayId.sol`
- Rust crate: `crates/zbx-payid/`
- Tests: `tests/unit/payid.rs`
- Docs: `docs/PAYID.md`