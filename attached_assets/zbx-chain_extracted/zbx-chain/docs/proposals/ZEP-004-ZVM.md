# ZEP-004: ZVM — Zebvix Virtual Machine

| Field         | Value                                      |
|---------------|--------------------------------------------|
| **ZEP**       | 004                                        |
| **Title**     | ZVM — Zebvix Virtual Machine               |
| **Author**    | Zebvix Core Team                           |
| **Status**    | ACCEPTED                                   |
| **Category**  | Core                                       |
| **Activation**| Block 100,000                              |
| **Version**   | ZVM v1                                     |

---

## Summary

ZEP-004 replaces the standard EVM with the **Zebvix Virtual Machine (ZVM)**.

ZVM is a strict **superset** of the EVM:
- All existing Ethereum/Solidity contracts run unchanged
- 10 new ZBX-native opcodes added (0xC0–0xC9)
- 6 new precompiled contracts added (0x0A–0x0F)
- Same gas accounting for all EVM opcodes

---

## New ZVM Opcodes

| Opcode   | Hex  | Gas  | Stack In         | Stack Out       | Description                        |
|----------|------|------|------------------|-----------------|------------------------------------|
| PAYID    | 0xC0 | 200  | [ptr, len]       | [address]       | Resolve ali@zbx → wallet address   |
| ZUSDBAL  | 0xC1 | 100  | [address]        | [uint256]       | ZUSD balance of address            |
| ZBXPRICE | 0xC2 | 50   | []               | [uint256]       | ZBX/USD price (18 decimals)        |
| ZBXTIME  | 0xC3 | 2    | []               | [5000]          | ZBX block time (ms, always 5000)   |
| AASENDER | 0xC4 | 2    | []               | [address]       | AA UserOperation original sender   |
| CHAINVER | 0xC5 | 2    | []               | [uint256]       | ZVM version number                 |
| BLOBFEE  | 0xC6 | 50   | []               | [uint256]       | Current blob base fee (wei/byte)   |
| PAYIDSET | 0xC7 | 100  | [address]        | [0 or 1]        | Does address have a Pay ID?        |
| ZBXBURN  | 0xC8 | 500  | [amount]         | []              | Burn ZBX from caller               |
| ZVMLOG   | 0xC9 | 600  | [kp,kl,vp,vl]   | []              | Emit structured key-value log      |

---

## New ZVM Precompiles

| Address | Name          | Gas    | Description                              |
|---------|---------------|--------|------------------------------------------|
| 0x0A    | PAYID_RESOLVE | 2,000  | Resolve Pay ID string → address          |
| 0x0B    | KZG_VERIFY    | 50,000 | Verify KZG proof (DA layer)              |
| 0x0C    | PRICE_ORACLE  | 500    | ZBX/USD price from on-chain oracle       |
| 0x0D    | ED25519_VERIFY| 3,000  | Verify Ed25519 signature                 |
| 0x0E    | VRF_VERIFY    | 8,000  | Verify VRF output                        |
| 0x0F    | ZUSD_BALANCE  | 100    | ZUSD balance of address                  |

---

## ZVM Magic Prefix (Optional)

ZVM-native contracts MAY start with magic bytes `0xEF 0x5A 0x42`:

```
EF 5A 42  [bytecode...]
```

- `EF` — reserved in EVM (INVALID), safe marker
- `5A 42` — "ZB" in ASCII (Zebvix)

Contracts with this prefix get ZVM-native gas discounts and tooling support.
Standard Solidity contracts (no prefix) continue to work unchanged.

---

## Backwards Compatibility

| Contract Type      | Behaviour in ZVM                                    |
|--------------------|-----------------------------------------------------|
| Ethereum Solidity  | Runs identically — no changes needed                |
| Ethereum Vyper     | Runs identically                                    |
| EIP-2929 warm/cold | Same storage access accounting                      |
| EIP-3855 PUSH0     | Supported                                           |
| Cancun opcodes     | BLOBHASH, BLOBBASEFEE, TLOAD/TSTORE, MCOPY all work |
| ZVM opcodes (0xC0+)| Were INVALID in EVM — now have new semantics        |

No existing deployed contract is affected by ZEP-004 activation.

---

## Use Cases

### 1. Pay ID in contracts
```solidity
// Send ZBX to "ali@zbx" directly from a contract:
address wallet = ZvmOpcodes.resolvePayId("ali@zbx");
payable(wallet).transfer(msg.value);
```

### 2. ZUSD-aware DeFi
```solidity
// Check ZUSD balance without external call:
uint256 zusdBal = ZvmOpcodes.zusdBalance(user);
require(zusdBal >= repayAmount, "Not enough ZUSD");
```

### 3. Price-aware contracts
```solidity
// Native price feed without oracle imports:
uint256 zbxUsd = ZvmOpcodes.zbxPrice();
uint256 usdValue = (zbxAmount * zbxUsd) / 1e18;
```

### 4. AA-aware contracts
```solidity
// Know who REALLY sent this (not the bundler):
address realUser = ZvmOpcodes.aaSender();
emit UserAction(realUser, action);
```

### 5. Deflationary burn
```solidity
// Burn 0.1% of every swap as ZBX deflationary fee:
ZvmOpcodes.burnZbx(swapAmount / 1000);
```

---

## Crate

- `crates/zbx-zvm/` — ZVM execution engine (12 files)
- `src/zvm/` — Chain integration (3 files)
- `contracts/libraries/ZvmOpcodes.sol` — Solidity wrapper library

---

## Activation

- **Testnet**: Block 20,000
- **Mainnet**: Block 100,000 (~5.8 days after genesis @ 5s/block)
- **Governance vote required**: Yes (Core category)
- **Backwards compatible**: Yes — all existing contracts unaffected