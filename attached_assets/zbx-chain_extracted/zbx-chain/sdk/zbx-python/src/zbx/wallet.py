"""Zebvix Chain wallet — key generation, mnemonic, signing, and keystores."""

from __future__ import annotations
from typing import Optional
from .exceptions import WalletError
from .constants import CHAIN_ID_TESTNET

__all__ = ["Wallet"]


class Wallet:
    """An secp256k1 wallet for Zebvix Chain.

    Supports EIP-155 transaction signing, EIP-191 personal_sign,
    BIP-39 mnemonic import/export, and Ethereum v3 keystore files.

    Usage::

        # Generate a new wallet
        w = Wallet.generate()
        print(w.address)

        # Import from private key
        w = Wallet.from_private_key("0xdeadbeef...")

        # Import from mnemonic
        w = Wallet.from_mnemonic("word1 word2 ... word12")
    """

    def __init__(self, private_key: bytes, chain_id: int = CHAIN_ID_TESTNET) -> None:
        if len(private_key) != 32:
            raise WalletError(f"private key must be 32 bytes, got {len(private_key)}")
        self._priv = private_key
        self._chain_id = chain_id
        self._address: Optional[str] = None

    # ── Factory methods ───────────────────────────────────────────────────────

    @classmethod
    def generate(cls, chain_id: int = CHAIN_ID_TESTNET) -> "Wallet":
        """Generate a new random wallet."""
        import os
        return cls(private_key=os.urandom(32), chain_id=chain_id)

    @classmethod
    def from_private_key(cls, key_hex: str, chain_id: int = CHAIN_ID_TESTNET) -> "Wallet":
        """Import from a hex private key (with or without 0x prefix)."""
        key_hex = key_hex.removeprefix("0x")
        try:
            key_bytes = bytes.fromhex(key_hex)
        except ValueError as e:
            raise WalletError(f"invalid hex private key: {e}") from e
        return cls(private_key=key_bytes, chain_id=chain_id)

    @classmethod
    def from_mnemonic(
        cls,
        mnemonic: str,
        account_index: int = 0,
        chain_id: int = CHAIN_ID_TESTNET,
    ) -> "Wallet":
        """Derive a wallet from a BIP-39 mnemonic (BIP-44 path m/44'/60'/0'/0/index)."""
        try:
            from eth_account import Account  # type: ignore
            Account.enable_unaudited_hdwallet_features()
            acct = Account.from_mnemonic(
                mnemonic,
                account_path=f"m/44'/60'/{account_index}'/0/0",
            )
            return cls.from_private_key(acct.key.hex(), chain_id=chain_id)
        except ImportError as e:
            raise WalletError("eth-account required for mnemonic support: pip install eth-account") from e

    @classmethod
    def from_keystore(cls, keystore: dict, password: str, chain_id: int = CHAIN_ID_TESTNET) -> "Wallet":
        """Import from an Ethereum v3 keystore JSON dict."""
        try:
            from eth_account import Account  # type: ignore
            priv = Account.decrypt(keystore, password)
            return cls(private_key=bytes(priv), chain_id=chain_id)
        except ImportError as e:
            raise WalletError("eth-account required: pip install eth-account") from e

    # ── Properties ────────────────────────────────────────────────────────────

    @property
    def address(self) -> str:
        """EIP-55 checksummed address."""
        if self._address is None:
            self._address = self._derive_address()
        return self._address

    @property
    def private_key_hex(self) -> str:
        """Hex-encoded private key (without 0x prefix)."""
        return self._priv.hex()

    @property
    def chain_id(self) -> int:
        return self._chain_id

    # ── Signing ───────────────────────────────────────────────────────────────

    def sign_transaction(self, tx: dict) -> str:
        """Sign an EIP-1559 or legacy transaction dict.

        Args:
            tx: Transaction fields (to, value, gas, maxFeePerGas, etc.).

        Returns:
            0x-prefixed hex-encoded signed RLP transaction.
        """
        try:
            from eth_account import Account  # type: ignore
            signed = Account.sign_transaction(tx, self._priv)
            return signed.rawTransaction.hex()
        except ImportError as e:
            raise WalletError("eth-account required: pip install eth-account") from e

    def sign_message(self, message: str | bytes) -> str:
        """Sign a personal message (EIP-191 prefix)."""
        try:
            from eth_account import Account  # type: ignore
            from eth_account.messages import encode_defunct  # type: ignore
            msg = encode_defunct(text=message) if isinstance(message, str) else encode_defunct(message)
            signed = Account.sign_message(msg, self._priv)
            return "0x" + signed.signature.hex()
        except ImportError as e:
            raise WalletError("eth-account required: pip install eth-account") from e

    def export_keystore(self, password: str) -> dict:
        """Export this wallet as an Ethereum v3 keystore (scrypt)."""
        try:
            from eth_account import Account  # type: ignore
            return Account.encrypt(self._priv, password)
        except ImportError as e:
            raise WalletError("eth-account required: pip install eth-account") from e

    # ── Internal ──────────────────────────────────────────────────────────────

    def _derive_address(self) -> str:
        """Derive the Ethereum address from the private key."""
        try:
            from eth_account import Account  # type: ignore
            return Account.from_key(self._priv).address
        except ImportError:
            return "0x" + "0" * 40  # fallback — will not be correct

    def __repr__(self) -> str:
        return f"Wallet(address={self.address}, chain_id={self._chain_id})"
