# ZVM — Zebvix Virtual Machine

> **⚠ Known limitation (Session 13, OPEN — `S7-EVM3` CRITICAL):** the CALL family
> (CALL / DELEGATECALL / STATICCALL / CALLCODE / CREATE / CREATE2 / SELFDESTRUCT /
> REVERT) is **not yet wired** in the ZVM dispatch table either. ZVM today executes
> single-contract Solidity / ZVM-bytecode but cannot do contract-to-contract calls.
> See `docs/proposals/S7-EVM3-call-family-implementation.md` (~16 dev-days, P0-T03)
> for the implementation plan.

## Overview

ZVM (Zebvix Virtual Machine) is ZBX Chain's execution environment.
It is a **superset of EVM** — every Ethereum contract runs unchanged,
plus 10 new ZBX-native opcodes for chain-specific features.

## Key Features

| Feature               | EVM | ZVM |
|-----------------------|-----|-----|
| Solidity contracts    | ✅  | ✅  |
| Vyper contracts       | ✅  | ✅  |
| Existing ETH bytecode | ✅  | ✅  |
| Pay ID resolution     | ❌  | ✅ PAYID opcode   |
| Native ZUSD balance   | ❌  | ✅ ZUSDBAL opcode |
| On-chain price feed   | ❌  | ✅ ZBXPRICE opcode|
| AA sender tracking    | ❌  | ✅ AASENDER opcode|
| ZBX burn mechanism    | ❌  | ✅ ZBXBURN opcode |
| KZG proof precompile  | ❌  | ✅ 0x0B precompile|
| Ed25519 verify        | ❌  | ✅ 0x0D precompile|

## Quick Start

### Writing a ZVM-native contract

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import "./libraries/ZvmOpcodes.sol";

contract MyZvmContract {
    using ZvmOpcodes for *;

    // Send to a Pay ID instead of raw address
    function sendToPayId(string calldata payId) external payable {
        address wallet = ZvmOpcodes.resolvePayId(payId);
        require(wallet != address(0), "Pay ID not found");
        payable(wallet).transfer(msg.value);
    }

    // Get ZBX price without oracle imports
    function getZbxPrice() external view returns (uint256) {
        return ZvmOpcodes.zbxPrice();
    }

    // Check who really sent this (AA-aware)
    function doSomething() external {
        address realSender = ZvmOpcodes.aaSender();
        // realSender is the user, not the bundler
        emit Action(realSender);
    }

    event Action(address indexed user);
}
```

## ZVM vs EVM Opcodes (0xC0–0xC9)

These bytes were INVALID in EVM. In ZVM they have new meaning:

```
0xC0 PAYID    — resolve Pay ID
0xC1 ZUSDBAL  — ZUSD balance
0xC2 ZBXPRICE — ZBX/USD price
0xC3 ZBXTIME  — block time (5000ms)
0xC4 AASENDER — AA original sender
0xC5 CHAINVER — ZVM version
0xC6 BLOBFEE  — blob base fee
0xC7 PAYIDSET — has Pay ID?
0xC8 ZBXBURN  — burn ZBX
0xC9 ZVMLOG   — structured log
```

## ZVM Precompiles (0x0A–0x0F)

```
0x01–0x09  Standard EVM (same as Ethereum)
0x0A       Pay ID resolver
0x0B       KZG proof verification
0x0C       ZBX price oracle
0x0D       Ed25519 signature verify
0x0E       VRF output verify
0x0F       ZUSD balance query
```

## Crate: zbx-zvm

```
crates/zbx-zvm/
  src/
    lib.rs          — Public API, ZVM_VERSION, ZVM_MAGIC
    opcodes.rs      — All opcode definitions (EVM + ZVM)
    interpreter.rs  — Main execution loop
    stack.rs        — 256-bit stack (max 1024 items)
    memory.rs       — Linear byte memory
    gas.rs          — Gas cost tables
    precompiles.rs  — Standard + ZVM precompiles
    context.rs      — Input/output types
    host.rs         — State access interface
    executor.rs     — Top-level entry point
    tracer.rs       — Debug tracing
    error.rs        — Error types
```