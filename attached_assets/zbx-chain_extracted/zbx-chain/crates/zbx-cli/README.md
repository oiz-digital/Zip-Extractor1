# zbxctl -- Zebvix Chain CLI

The official command-line interface for the ZBX chain (Chain ID: 8989).

## Install

    cargo install --path crates/zbx-cli

## Quick start

    # Generate a brand-new wallet
    zbxctl wallet new --mnemonic

    # Import from raw private key
    zbxctl wallet import --private-key 0xdeadbeef...

    # Import from BIP-39 mnemonic
    zbxctl wallet import --mnemonic "word1 word2 ... word12"

    # Export (decrypt) private key from keystore
    zbxctl wallet export --keystore ./my.keystore

    # Show ZBX native balance
    zbxctl wallet balance

    # Send native ZBX
    zbxctl tx send --to 0xRecipient... --value 1000000000000000000

    # Deploy a contract
    zbxctl contract deploy --bytecode ./out/Token.bin --abi ./out/Token.abi

    # Read-only contract call
    zbxctl contract call --address 0xContract... --abi ./Token.abi --fn "balanceOf(address)" --args "0xUser..."

    # Write transaction to contract (contract send)
    zbxctl contract send --address 0xContract... --abi ./Token.abi --fn "transfer(address,uint256)" --args "0xTo...,1e18"

    # Get swap quote (read-only, no gas)
    zbxctl defi swap-quote --token-in 0xEeeeEeeeEeEeEeEeEeEeEeEeEeEeEeEeEeEeEeEe --token-out 0xZUSD... --amount-in 1000000000000000000

    # Execute swap with 0.5% slippage
    zbxctl defi swap-execute --token-in 0xEeee... --token-out 0xZUSD... --amount-in 1000000000000000000 --slippage 50

    # Single validator info
    zbxctl stake validator --address 0xValidator...

    # Single proposal info
    zbxctl governance info --id 1

    # Oracle price query
    zbxctl defi oracle-price --feed ZBX/USD

## Global flags

  --rpc-url       ZBX_RPC_URL     default: http://localhost:8545
  --chain-id      ZBX_CHAIN_ID    default: 8989
  --keystore      ZBX_KEYSTORE    (path to encrypted keystore)
  --private-key   ZBX_PRIVATE_KEY (raw hex, use keystore in prod)
  --output                        text | json | table

## Testnet

    zbxctl --rpc-url https://testnet-rpc.zebvix.io --chain-id 8990 wallet balance
    zbxctl --rpc-url https://testnet-rpc.zebvix.io --chain-id 8990 dev faucet --address 0xMe...