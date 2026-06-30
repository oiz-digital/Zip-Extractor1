"""Zebvix SDK exception hierarchy."""


class ZbxError(Exception):
    """Base exception for all Zebvix SDK errors."""


class RpcError(ZbxError):
    """JSON-RPC protocol error returned by the node."""

    def __init__(self, code: int, message: str) -> None:
        self.code = code
        self.message = message
        super().__init__(f"RPC error {code}: {message}")


class WalletError(ZbxError):
    """Error in wallet operations (key derivation, signing, import)."""


class TransactionError(ZbxError):
    """Error constructing or broadcasting a transaction."""


class ValidationError(ZbxError):
    """Input validation failed (bad address, hash, amount)."""
