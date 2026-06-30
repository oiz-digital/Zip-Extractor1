# ZEP-009: AI Inference Precompile (AIINFER)

| Field      | Value                                     |
|:---|:---|
| ZEP Number | ZEP-009                                   |
| Title      | AI Inference ZVM Precompile (AIINFER)     |
| Status     | **Research** — targets block 300,000      |
| Category   | ZVM / AI                                  |
| Authors    | Zebvix Core Team                          |

## Abstract

ZEP-009 adds a new ZVM native opcode `0xCA` (AIINFER) that enables smart
contracts to call AI/ML model inference directly on-chain. Models are stored
on the ZBX DA layer (ZEP-003) and executed deterministically by all validators.

## Motivation

Traditional blockchains cannot use AI because inference is non-deterministic.
ZBX solves this by:
1. Quantizing all models to INT8 (fully deterministic integer arithmetic)
2. Storing model weights on the DA layer (content-addressed, immutable)
3. Running inference via a deterministic host function exposed to ZVM

## Supported Models

| Model ID | Purpose                    | Gas Cost |
|:---|:---|:---|
| 0x01     | Spam / rug-pull detection  | 500,000  |
| 0x02     | DeFi risk scoring          | 750,000  |
| 0x03     | NFT trait tagging          | 2,000,000|
| 0x04     | ZUSD collateral risk       | 600,000  |

## Example (Solidity)

```solidity
contract AntiRug {
    function isRug(address token) external view returns (bool) {
        bytes memory input = abi.encode(token);
        (bytes memory result,) = address(0xCA).staticcall(
            abi.encode(uint8(1), input)  // model 0x01 = spam classifier
        );
        return result[0] > 128; // confidence > 50% = likely rug
    }
}
```