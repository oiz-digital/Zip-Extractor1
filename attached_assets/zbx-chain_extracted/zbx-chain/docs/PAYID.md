# ZBX Pay ID

## Kya hai?

ZBX Pay ID ek **UPI-style** human-readable wallet address hai.

Jaise UPI mein hota hai:
```
paytm@upi
9876543210@okaxis
ali@ybl
```

ZBX mein exactly waise hi:
```
ali@zbx
myshop@zbx
zebvix@zbx
```

Kisi ko ZBX ya ZUSD bhejne ke liye bas type karo: **ali@zbx**
Badi ugly address `0x742d35Cc6634C0532925a3b844Bc454e4438f44e` ki zaroorat nahi.

---

## Format

| Type             | Example             | Matlab                                  |
|------------------|---------------------|-----------------------------------------|
| Standard         | `ali@zbx`           | Ali ka ZBX wallet                       |
| Sub-ID (branch)  | `shop.ali@zbx`      | Ali ki shop ka alag wallet              |
| Sub-ID (team)    | `treasury.zebvix@zbx` | Zebvix ka treasury wallet             |

---

## Register karne ke rules

```
Format:    [name]@zbx
Min chars: 3
Max chars: 32
Allowed:   a-z, 0-9, hyphen (-)
Fee:       0.01 ZBX (one-time, squatting se bachane ke liye)
```

**Valid examples:**
- `ali@zbx` ✓
- `my-shop@zbx` ✓
- `trader123@zbx` ✓

**Invalid:**
- `ab@zbx` ✗ (too short, min 3)
- `ali shop@zbx` ✗ (space not allowed)
- `-ali@zbx` ✗ (hyphen at start)

---

## Features

### 1. Multi-chain addresses
Ek hi `ali@zbx` pe alag chains ke addresses:
```
ali@zbx → ZBX chain:  0x742d35Cc...
ali@zbx → Ethereum:   0x742d35Cc... (same ya alag)
ali@zbx → Bitcoin:    bc1qxy2kgdygjrsqtzq2n0yrf...
```

### 2. Sub-IDs (Businesses ke liye)
`zebvix` apne system mein sub-IDs issue kar sakta hai:
```
treasury.zebvix@zbx    → Treasury wallet
operations.zebvix@zbx  → Ops wallet
customer1.zebvix@zbx   → Customer 1
customer2.zebvix@zbx   → Customer 2
```

### 3. Transfer / Sell
Ache Pay IDs valuable hote hain — transfer ya sell kar sako.

### 4. Reverse lookup
Address se Pay ID pata karo:
```
0x742d35Cc... → ali@zbx
```

---

## Send karna kaise?

### Wallet app mein (UPI jaisi feel):
```
1. "Send" dabao
2. "ali@zbx" type karo
3. App resolve karta hai → 0x742d... dikh jata hai
4. Amount enter karo
5. Confirm
```

### Code mein (Rust):
```rust
let resolver = PayIdResolver::new("https://rpc.zbx.network");
let result = resolver.resolve("ali@zbx").await?;
// result.pay_id   = "ali@zbx"
// result.address  = "0x742d35Cc6634C0532925a3b844Bc454e4438f44e"
println!("Sending to: {}", result.address);
```

### Code mein (Solidity):
```solidity
IZbxPayId payIdRegistry = IZbxPayId(0x7e4a7f8b...);
address wallet = payIdRegistry.resolve("ali@zbx");
// Ab is address pe ZBX transfer karo
```

---

## Smart Wallet ke saath integration

AA wallet deploy karte waqt automatically Pay ID register karo:
```solidity
ZbxSmartWallet wallet = factory.deploy(owner, salt);
payIdRegistry.register{value: 0.01 ether}("ali@zbx", address(wallet));
// Ab ali@zbx seedha smart wallet pe point karta hai
```

---

## Pricing

| Action               | Cost                     |
|----------------------|--------------------------|
| Register             | 0.01 ZBX (one-time)      |
| Sub-ID issue         | Free (parent owner pays) |
| Update wallet        | Free (gas only)          |
| Transfer Pay ID      | Free (gas only)          |
| Multi-chain address  | Free (gas only)          |

---

## Contract Addresses

| Network         | Address                                      |
|-----------------|----------------------------------------------|
| ZBX Mainnet (8989) | `0x7e4a7f8bCE8CfD1765CdE34Dbf5e8D7fB1A43e9` |
| ZBX Testnet     | `0x1a2b3c...testnet`                        |