"""
zbx — Zebvix Chain Python SDK.

Provides a production-ready async client for Zebvix Chain including:
- JSON-RPC 2.0 HTTP + WebSocket client
- EIP-155 transaction signing
- BIP-32/44 HD wallet support
- Staking, bridge, oracle, governance helpers

Quick start::

    import asyncio
    from zbx import ZbxClient

    async def main():
        async with ZbxClient("https://testnet-rpc.zebvix.com") as client:
            block_num = await client.get_block_number()
            print(f"Block: {block_num}")

    asyncio.run(main())
"""

from .client import ZbxClient
from .wallet import Wallet
from .exceptions import ZbxError, RpcError, WalletError
from .types import (
    Block, Transaction, Receipt, Account,
    Validator, GasPrice, NetworkInfo,
)
from .constants import (
    CHAIN_ID_MAINNET, CHAIN_ID_TESTNET, CHAIN_ID_DEVNET,
    ZBX_DECIMALS, WEI_PER_ZBX,
)
from .utils import to_wei, from_wei, checksum_address, is_valid_address

__version__ = "1.0.0"
__all__ = [
    "ZbxClient", "Wallet",
    "ZbxError", "RpcError", "WalletError",
    "Block", "Transaction", "Receipt", "Account", "Validator",
    "GasPrice", "NetworkInfo",
    "CHAIN_ID_MAINNET", "CHAIN_ID_TESTNET", "CHAIN_ID_DEVNET",
    "ZBX_DECIMALS", "WEI_PER_ZBX",
    "to_wei", "from_wei", "checksum_address", "is_valid_address",
]
