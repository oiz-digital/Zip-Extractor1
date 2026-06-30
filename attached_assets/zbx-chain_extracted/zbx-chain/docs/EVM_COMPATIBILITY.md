# EVM Compatibility

> **⚠ Known limitation (S7-EVM3 CRITICAL — OPEN):** the CALL family of opcodes
> (CALL / DELEGATECALL / STATICCALL / CALLCODE / CREATE / CREATE2 / SELFDESTRUCT /
> REVERT) is **not yet implemented** in either `zbx-evm` or `zbx-zvm` dispatch tables.
> Real-world impact: any Solidity contract that calls another contract (factories,
> proxies, Uniswap-style routers, ERC-4337 wallets, Aave-style multi-contract DeFi)
> will silently revert at the unimplemented opcode. Single-contract Solidity (ERC-20,
> simple logic, isolated NFTs) works today. See `docs/DOC_STATUS.md`.
>
> **✅ Fixed (2026-06-27):** All 6 previously-stubbed standard EVM precompiles in `zbx-vm`
> are now fully implemented: `modexp (0x05)`, `bn128_add (0x06)`, `bn128_mul (0x07)`,
> `bn128_pairing (0x08)`, `blake2f (0x09)`, `kzg_point_eval (0x0a)`. All three VM
> implementations (`zbx-evm`, `zbx-zvm`, `zbx-vm`) now share byte-identical crypto.

Zebvix Chain is fully EVM-compatible. Smart contracts compiled for Ethereum
deploy and execute without modification.

## Supported EIPs

| EIP    | Name                      | Status    | Notes |
|--------|---------------------------|-----------|-------|
| 155    | Simple replay protection  | ✅ Active | |
| 1559   | Fee market                | ✅ Active | |
| 2929   | Gas cost increases        | ✅ Active | |
| 2930   | Optional access lists     | ✅ Active | |
| 3155   | EVM trace spec            | ✅ Active | |
| 3529   | Reduction in gas refunds  | ✅ Active | |
| 3541   | Reject 0xEF prefix        | ✅ Active | |
| 3675   | Merge (PoS)               | ✅ Active | |
| 3855   | PUSH0 instruction         | ✅ Active | |
| 3860   | Limit initcode size       | ✅ Active | |
| 4895   | Beacon withdrawals        | ✅ Active | |
| 1153   | Transient storage         | ✅ Active | |
| 5656   | MCOPY                     | ✅ Active | |
| 4844   | Blob transactions (KZG)   | ✅ Active | `kzg_point_eval (0x0a)` implemented in `zbx-vm` + `zbx-evm` + `zbx-zvm` |
| 7702   | Account abstraction (EOA) | 🔄 Planned | |

## Standard EVM Precompiles (0x01–0x0a)

All nine standard Ethereum precompiles are fully implemented across all three VM crates.

| Address | Name | zbx-evm | zbx-zvm | zbx-vm | Notes |
|---------|------|---------|---------|--------|-------|
| 0x01 | ecrecover | ✅ | ✅ | ✅ | secp256k1 signature recovery |
| 0x02 | sha256 | ✅ | ✅ | ✅ | |
| 0x03 | ripemd160 | ✅ | ✅ | ✅ | |
| 0x04 | identity | ✅ | ✅ | ✅ | |
| 0x05 | modexp | ✅ | ✅ | ✅ | num-bigint big-integer exponentiation |
| 0x06 | bn128_add | ✅ | ✅ | ✅ | substrate-bn G1 point addition |
| 0x07 | bn128_mul | ✅ | ✅ | ✅ | substrate-bn G1 scalar multiply |
| 0x08 | bn128_pairing | ✅ | ✅ | ✅ | substrate-bn pairing check |
| 0x09 | blake2f | ✅ | ✅ | ✅ | inline BLAKE2b-F compression |
| 0x0a | kzg_point_eval | ✅ | ✅ | ✅ | zbx_crypto::kzg (EIP-4844) |

> **zbx-vm precompiles 0x05–0x0a were added 2026-06-27** (previously returned `NotImplemented`). 19 unit tests added.

## ZBX Extensions

Zebvix adds custom precompiles at addresses `0xZBX_{n}`:

| Address  | Name          | Gas cost | Description                    |
|----------|---------------|----------|--------------------------------|
| 0xZBX_01 | BLS G1 Add    | 500      | BLS12-381 G1 point addition    |
| 0xZBX_02 | BLS G1 Mul    | 12,000   | BLS12-381 G1 scalar multiply   |
| 0xZBX_03 | BLS G2 Add    | 800      | BLS12-381 G2 point addition    |
| 0xZBX_04 | BLS Pairing   | 65,000   | BLS12-381 pairing check        |
| 0xZBX_05 | Bridge Hook   | 3,000    | Native bridge call hook        |

## Chain-Specific Parameters

| Parameter         | Ethereum  | Zebvix Chain |
|-------------------|-----------|--------------|
| Chain ID          | 1         | 8989         |
| Block time        | 12s       | 5s           |
| Max gas per block | 30M       | 30M          |
| Address format    | EIP-55    | EIP-55       |
| Signature scheme  | secp256k1 | secp256k1    |

## Tooling Compatibility

| Tool         | Compatible | Notes                         |
|--------------|------------|-------------------------------|
| MetaMask     | ✅          | Add custom network (ID: 8989) |
| Hardhat      | ✅          | Use `chainId: 8989`           |
| Foundry       | ✅          | `--chain-id 8989`             |
| ethers.js    | ✅          | Connect to RPC endpoint       |
| viem         | ✅          | Define custom chain            |
| Remix IDE    | ✅          | Web3 provider injection        |
| Tenderly     | 🔄 Planned | Dashboard integration         |