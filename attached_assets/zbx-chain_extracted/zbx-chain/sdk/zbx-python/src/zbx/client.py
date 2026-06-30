"""Async JSON-RPC 2.0 client for Zebvix Chain."""

from __future__ import annotations
import asyncio
from typing import Any, List, Optional
import httpx
from .exceptions import RpcError, ZbxError
from .utils import hex_to_int, require_valid_address, require_valid_hash

__all__ = ["ZbxClient"]


class ZbxClient:
    """Async Zebvix Chain JSON-RPC client.

    Usage::

        async with ZbxClient("https://testnet-rpc.zebvix.com") as client:
            num = await client.get_block_number()
    """

    def __init__(self, rpc_url: str, timeout: float = 30.0) -> None:
        if not rpc_url:
            raise ZbxError("rpc_url must not be empty")
        self._url = rpc_url
        self._timeout = timeout
        self._id: int = 0
        self._http: Optional[httpx.AsyncClient] = None
        self._chain_id: Optional[int] = None

    # ── Context manager ───────────────────────────────────────────────────────

    async def __aenter__(self) -> "ZbxClient":
        self._http = httpx.AsyncClient(timeout=self._timeout)
        self._chain_id = await self.get_chain_id()
        return self

    async def __aexit__(self, *_: Any) -> None:
        if self._http:
            await self._http.aclose()

    # ── Properties ────────────────────────────────────────────────────────────

    @property
    def chain_id(self) -> Optional[int]:
        """The chain ID of the connected network (set after first call)."""
        return self._chain_id

    # ── Internal RPC call ─────────────────────────────────────────────────────

    async def _call(self, method: str, *params: Any) -> Any:
        """Execute a JSON-RPC call and return the result."""
        self._id += 1
        payload = {
            "jsonrpc": "2.0",
            "method": method,
            "params": list(params),
            "id": self._id,
        }
        http = self._http or httpx.AsyncClient(timeout=self._timeout)
        try:
            resp = await http.post(self._url, json=payload)
            resp.raise_for_status()
            data = resp.json()
        except httpx.HTTPError as exc:
            raise ZbxError(f"HTTP error: {exc}") from exc

        if err := data.get("error"):
            raise RpcError(err.get("code", -1), err.get("message", "unknown"))
        return data.get("result")

    # ── eth_* ─────────────────────────────────────────────────────────────────

    async def get_chain_id(self) -> int:
        """Return the chain ID (eth_chainId)."""
        result = await self._call("eth_chainId")
        return hex_to_int(result)

    async def get_block_number(self) -> int:
        """Return the latest block number (eth_blockNumber)."""
        result = await self._call("eth_blockNumber")
        return hex_to_int(result)

    async def get_balance(self, address: str, block: str = "latest") -> int:
        """Return the ZBX balance of *address* in wei.

        Args:
            address: EIP-55 or hex address.
            block:   Block tag (``'latest'``, ``'finalized'``, hex number).

        Returns:
            Balance in wei.
        """
        require_valid_address(address, "address")
        result = await self._call("eth_getBalance", address, block)
        return hex_to_int(result)

    async def get_nonce(self, address: str, block: str = "latest") -> int:
        """Return the transaction count (nonce) for *address*."""
        require_valid_address(address, "address")
        result = await self._call("eth_getTransactionCount", address, block)
        return hex_to_int(result)

    async def get_gas_price(self) -> int:
        """Return the current gas price in wei (eth_gasPrice)."""
        result = await self._call("eth_gasPrice")
        return hex_to_int(result)

    async def get_block(
        self,
        block: int | str = "latest",
        full_transactions: bool = False,
    ) -> Optional[dict]:
        """Fetch a block by number or tag.

        Args:
            block:            Block number (int) or tag (``'latest'``, etc.).
            full_transactions: If True, include full tx objects; else tx hashes.

        Returns:
            Block dict or ``None`` if not found.
        """
        tag = hex(block) if isinstance(block, int) else block
        return await self._call("eth_getBlockByNumber", tag, full_transactions)

    async def get_transaction(self, tx_hash: str) -> Optional[dict]:
        """Fetch a transaction by hash."""
        require_valid_hash(tx_hash, "tx_hash")
        return await self._call("eth_getTransactionByHash", tx_hash)

    async def get_receipt(self, tx_hash: str) -> Optional[dict]:
        """Fetch a transaction receipt by hash."""
        require_valid_hash(tx_hash, "tx_hash")
        return await self._call("eth_getTransactionReceipt", tx_hash)

    async def send_raw_transaction(self, raw_tx: str) -> str:
        """Broadcast a signed RLP transaction.

        Args:
            raw_tx: 0x-prefixed RLP-encoded signed transaction.

        Returns:
            Transaction hash.
        """
        if not raw_tx.startswith("0x"):
            raise ZbxError("raw_tx must be 0x-prefixed")
        return await self._call("eth_sendRawTransaction", raw_tx)

    async def call(self, to: str, data: str, block: str = "latest") -> str:
        """Execute a read-only contract call (eth_call)."""
        require_valid_address(to, "to")
        return await self._call("eth_call", {"to": to, "data": data}, block)

    async def estimate_gas(
        self,
        to: str,
        data: str = "0x",
        from_addr: Optional[str] = None,
        value: int = 0,
    ) -> int:
        """Estimate gas for a transaction (eth_estimateGas)."""
        params: dict[str, Any] = {"to": to, "data": data}
        if from_addr:
            params["from"] = from_addr
        if value:
            params["value"] = hex(value)
        result = await self._call("eth_estimateGas", params)
        return hex_to_int(result)

    async def get_logs(
        self,
        from_block: str = "latest",
        to_block: str = "latest",
        address: Optional[str] = None,
        topics: Optional[List[Optional[str]]] = None,
    ) -> List[dict]:
        """Fetch event logs (eth_getLogs)."""
        filt: dict[str, Any] = {"fromBlock": from_block, "toBlock": to_block}
        if address:
            filt["address"] = address
        if topics:
            filt["topics"] = topics
        return await self._call("eth_getLogs", filt) or []

    # ── zbx_* ─────────────────────────────────────────────────────────────────

    async def get_validators(self) -> List[dict]:
        """Return the active validator set (zbx_getValidators)."""
        return await self._call("zbx_getValidators") or []

    async def get_epoch(self) -> int:
        """Return the current epoch (zbx_getEpoch)."""
        result = await self._call("zbx_getEpoch")
        return hex_to_int(result)

    async def get_staking_info(self, address: str) -> dict:
        """Return staking info for a delegator address."""
        require_valid_address(address, "address")
        return await self._call("zbx_getStakingInfo", address) or {}

    async def get_bridge_status(self) -> dict:
        """Return bridge operational status."""
        return await self._call("zbx_getBridgeStatus") or {}

    async def get_oracle_price(self, pair: str) -> Optional[int]:
        """Return the latest oracle price for a pair (e.g. 'ZBX/USD')."""
        result = await self._call("zbx_getOraclePrice", pair)
        if result is None:
            return None
        return hex_to_int(result) if isinstance(result, str) else int(result)

    # ── Polling helpers ───────────────────────────────────────────────────────

    async def wait_for_transaction(
        self, tx_hash: str, poll_interval: float = 2.0, timeout: float = 120.0
    ) -> dict:
        """Poll until a transaction is mined and return its receipt.

        Raises:
            asyncio.TimeoutError: If the transaction is not mined within *timeout*.
        """
        require_valid_hash(tx_hash, "tx_hash")
        elapsed = 0.0
        while elapsed < timeout:
            receipt = await self.get_receipt(tx_hash)
            if receipt is not None:
                return receipt
            await asyncio.sleep(poll_interval)
            elapsed += poll_interval
        raise asyncio.TimeoutError(
            f"Transaction {tx_hash} not mined within {timeout}s"
        )
