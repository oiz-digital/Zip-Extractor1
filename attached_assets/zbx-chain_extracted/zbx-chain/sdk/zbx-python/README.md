# zbx-chain — Zebvix Chain Python SDK

Production Python SDK for Zebvix Chain.

## Installation

```bash
pip install zbx-chain
```

## Quick Start

```python
import asyncio
from zbx import ZbxClient, Wallet, to_wei, from_wei

async def main():
    async with ZbxClient("https://testnet-rpc.zebvix.com") as client:
        # Chain info
        print(f"Chain ID: {client.chain_id}")
        
        # Latest block
        block_num = await client.get_block_number()
        print(f"Block: {block_num}")
        
        # Balance
        balance_wei = await client.get_balance("0xYourAddress...")
        print(f"Balance: {from_wei(balance_wei):.4f} ZBX")
        
        # Validators
        validators = await client.get_validators()
        print(f"Active validators: {len(validators)}")

# Wallet operations
wallet = Wallet.generate()
print(f"New address: {wallet.address}")

# Import from private key
wallet = Wallet.from_private_key("0x...")

# Import from mnemonic  
wallet = Wallet.from_mnemonic("word1 word2 ... word12")

asyncio.run(main())
```

## Running Tests

```bash
pip install -e ".[dev]"
pytest tests/ -v
```

## Chain IDs

| Network  | Chain ID |
|----------|----------|
| Mainnet  | 8989     |
| Testnet  | 8990     |
| Devnet   | 8991     |
