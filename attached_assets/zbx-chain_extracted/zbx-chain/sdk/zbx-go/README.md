# zbx-go — Zebvix Chain Go SDK

Production Go client for Zebvix Chain.

## Installation

```bash
go get github.com/zebvix/zbx-go
```

## Quick Start

```go
package main

import (
    "context"
    "fmt"
    "log"

    zbx "github.com/zebvix/zbx-go"
)

func main() {
    // Connect to testnet
    client, err := zbx.Dial("https://testnet-rpc.zebvix.com")
    if err != nil { log.Fatal(err) }
    defer client.Close()

    ctx := context.Background()

    // Get chain info
    fmt.Printf("Chain ID: %d\n", client.ChainID())

    // Get latest block number
    num, err := client.GetBlockNumber(ctx)
    if err != nil { log.Fatal(err) }
    fmt.Printf("Block: %d\n", num)

    // Get account balance
    bal, err := client.GetBalance(ctx, "0xYourAddress")
    if err != nil { log.Fatal(err) }
    fmt.Printf("Balance: %s wei\n", bal.String())

    // Get validators
    validators, err := client.GetValidators(ctx)
    if err != nil { log.Fatal(err) }
    fmt.Printf("Validators: %d\n", len(validators))
}
```

## Chain IDs

| Network  | Chain ID |
|----------|----------|
| Mainnet  | 8989     |
| Testnet  | 8990     |
| Devnet   | 8991     |

## API Reference

### Client

| Method | Description |
|--------|-------------|
| `Dial(url)` | Connect to an RPC endpoint |
| `GetChainID(ctx)` | Get chain ID |
| `GetBlockNumber(ctx)` | Get latest block number |
| `GetBalance(ctx, addr)` | Get ZBX balance in wei |
| `GetTransactionCount(ctx, addr)` | Get account nonce |
| `GetGasPrice(ctx)` | Get current gas price |
| `GetLatestBlock(ctx)` | Get latest full block |
| `GetBlockByNumber(ctx, n, fullTxs)` | Get block by number |
| `GetTransactionByHash(ctx, hash)` | Get transaction |
| `GetTransactionReceipt(ctx, hash)` | Get receipt |
| `SendRawTransaction(ctx, rawTx)` | Broadcast signed tx |
| `Call(ctx, to, data)` | Read-only contract call |
| `EstimateGas(ctx, from, to, data, value)` | Estimate gas |
| `GetValidators(ctx)` | Get active validators |
| `GetEpoch(ctx)` | Get current epoch |

## Running Tests

```bash
go test ./...
```
