"""Utility functions for Zebvix SDK."""

from __future__ import annotations
import re
from .constants import WEI_PER_ZBX
from .exceptions import ValidationError

_HEX_ADDRESS_RE = re.compile(r"^0x[0-9a-fA-F]{40}$")
_HEX_HASH_RE    = re.compile(r"^0x[0-9a-fA-F]{64}$")


def is_valid_address(addr: str) -> bool:
    """Return True if *addr* is a valid 20-byte hex address."""
    return bool(_HEX_ADDRESS_RE.match(addr))


def is_valid_hash(h: str) -> bool:
    """Return True if *h* is a valid 32-byte hex hash."""
    return bool(_HEX_HASH_RE.match(h))


def checksum_address(addr: str) -> str:
    """Return the EIP-55 checksummed version of *addr*.

    Uses eth-account's implementation when available, otherwise
    returns the input lowercased.
    """
    try:
        from eth_utils import to_checksum_address  # type: ignore
        return to_checksum_address(addr)
    except ImportError:
        return addr.lower()


def to_wei(zbx: float | int | str) -> int:
    """Convert ZBX to wei.

    >>> to_wei(1)
    1000000000000000000
    >>> to_wei("0.5")
    500000000000000000
    """
    if isinstance(zbx, str):
        zbx = float(zbx)
    return int(zbx * WEI_PER_ZBX)


def from_wei(wei: int) -> float:
    """Convert wei to ZBX.

    >>> from_wei(1_000_000_000_000_000_000)
    1.0
    """
    return wei / WEI_PER_ZBX


def hex_to_int(h: str) -> int:
    """Convert a 0x-prefixed hex string to int."""
    return int(h, 16)


def int_to_hex(n: int) -> str:
    """Convert int to 0x-prefixed hex string."""
    return hex(n)


def require_valid_address(addr: str, name: str = "address") -> None:
    if not is_valid_address(addr):
        raise ValidationError(f"invalid {name}: '{addr}' must be 0x + 20 hex bytes")


def require_valid_hash(h: str, name: str = "hash") -> None:
    if not is_valid_hash(h):
        raise ValidationError(f"invalid {name}: '{h}' must be 0x + 32 hex bytes")
