"""Tests for ZbxClient."""

import json
import pytest
import respx
import httpx
from zbx import ZbxClient, ZbxError
from zbx.exceptions import RpcError


BASE_URL = "https://testnet-rpc.zebvix.com"


def rpc_response(result, request_id: int = 1) -> dict:
    return {"jsonrpc": "2.0", "id": request_id, "result": result}


def rpc_error_response(code: int, message: str, request_id: int = 1) -> dict:
    return {"jsonrpc": "2.0", "id": request_id, "error": {"code": code, "message": message}}


@pytest.mark.asyncio
async def test_get_chain_id():
    with respx.mock(base_url=BASE_URL) as mock:
        mock.post("/").respond(200, json=rpc_response("0x232e"))
        async with ZbxClient(BASE_URL) as c:
            assert c.chain_id == 8990


@pytest.mark.asyncio
async def test_get_block_number():
    with respx.mock(base_url=BASE_URL) as mock:
        mock.post("/").respond(200, json=rpc_response("0x64"))
        async with ZbxClient(BASE_URL) as c:
            # Override to avoid chain ID call changing mock
            c._chain_id = 8990
            num = await c.get_block_number()
            assert num == 100


@pytest.mark.asyncio
async def test_rpc_error_raises():
    with respx.mock(base_url=BASE_URL) as mock:
        mock.post("/").respond(200, json=rpc_error_response(-32601, "Method not found"))
        async with ZbxClient(BASE_URL) as c:
            c._chain_id = 8990
            with pytest.raises(RpcError) as exc_info:
                await c._call("nonexistent_method")
            assert exc_info.value.code == -32601


@pytest.mark.asyncio
async def test_invalid_address_rejected():
    async with ZbxClient.__new__(ZbxClient) as c:
        c._url = BASE_URL
        c._timeout = 30.0
        c._id = 0
        c._http = None
        c._chain_id = 8990
        with pytest.raises(Exception):
            await c.get_balance("not-an-address")


def test_empty_url_raises():
    with pytest.raises(ZbxError):
        ZbxClient("")


@pytest.mark.asyncio
async def test_get_balance():
    with respx.mock(base_url=BASE_URL) as mock:
        mock.post("/").respond(200, json=rpc_response("0xde0b6b3a7640000"))
        async with ZbxClient(BASE_URL) as c:
            c._chain_id = 8990
            bal = await c.get_balance("0x" + "ab" * 20)
            assert bal == 10 ** 18  # 1 ZBX


def test_to_wei():
    from zbx.utils import to_wei, from_wei
    assert to_wei(1) == 10**18
    assert to_wei("0.5") == 5 * 10**17
    assert from_wei(10**18) == 1.0
