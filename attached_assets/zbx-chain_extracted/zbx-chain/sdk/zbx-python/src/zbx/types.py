"""Pydantic models for Zebvix Chain data types."""

from __future__ import annotations
from typing import Optional, List
from pydantic import BaseModel, Field


class Block(BaseModel):
    number: int
    hash: str
    parent_hash: str = Field(alias="parentHash")
    state_root: str  = Field(alias="stateRoot")
    tx_root: str     = Field(alias="transactionsRoot")
    timestamp: int
    gas_limit: int   = Field(alias="gasLimit")
    gas_used: int    = Field(alias="gasUsed")
    base_fee: Optional[str] = Field(None, alias="baseFeePerGas")
    proposer: Optional[str] = Field(None, alias="miner")
    transactions: List[str] = []
    size: Optional[int] = None

    model_config = {"populate_by_name": True}


class Transaction(BaseModel):
    hash: str
    block_number: Optional[int] = Field(None, alias="blockNumber")
    block_hash: Optional[str]   = Field(None, alias="blockHash")
    from_addr: str               = Field(alias="from")
    to_addr: Optional[str]       = Field(None, alias="to")
    value: int                   # wei
    gas: int
    gas_price: Optional[int]    = Field(None, alias="gasPrice")
    max_fee: Optional[int]      = Field(None, alias="maxFeePerGas")
    max_priority_fee: Optional[int] = Field(None, alias="maxPriorityFeePerGas")
    nonce: int
    input: str
    tx_type: int                = Field(alias="type")

    model_config = {"populate_by_name": True}


class Receipt(BaseModel):
    transaction_hash: str         = Field(alias="transactionHash")
    block_number: int             = Field(alias="blockNumber")
    block_hash: str               = Field(alias="blockHash")
    from_addr: str                = Field(alias="from")
    to_addr: Optional[str]        = Field(None, alias="to")
    gas_used: int                 = Field(alias="gasUsed")
    cumulative_gas_used: int      = Field(alias="cumulativeGasUsed")
    status: bool                  # True = success
    logs: List[dict] = []
    contract_address: Optional[str] = Field(None, alias="contractAddress")

    model_config = {"populate_by_name": True}


class Account(BaseModel):
    address: str
    balance: int          # wei
    nonce: int
    code_hash: str
    is_contract: bool


class Validator(BaseModel):
    address: str
    pub_key: str
    stake: int
    delegated_stake: int
    commission: float
    status: str
    uptime_pct: float
    blocks_produced: int
    epoch_joined: int


class GasPrice(BaseModel):
    base_fee: int
    safe: int
    fast: int
    rapid: int


class NetworkInfo(BaseModel):
    chain_id: int
    chain_name: str
    network: str
    latest_block: int
    finalized_block: int
    peer_count: int
    sync_status: str
    node_version: str
