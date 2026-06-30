"""Tests for Wallet."""

import pytest
from zbx.wallet import Wallet
from zbx.exceptions import WalletError


def test_generate_wallet():
    w = Wallet.generate()
    assert w.address.startswith("0x")


def test_from_private_key_with_prefix():
    key = "0x" + "aa" * 32
    w = Wallet.from_private_key(key)
    assert isinstance(w.private_key_hex, str)
    assert len(w.private_key_hex) == 64


def test_from_private_key_without_prefix():
    key = "bb" * 32
    w = Wallet.from_private_key(key)
    assert w.private_key_hex == key


def test_invalid_key_hex_raises():
    with pytest.raises(WalletError):
        Wallet.from_private_key("not-valid-hex")


def test_invalid_key_length_raises():
    with pytest.raises(WalletError):
        Wallet(private_key=b"\x01" * 31)


def test_wallet_repr():
    w = Wallet.generate()
    r = repr(w)
    assert "Wallet(" in r
    assert "chain_id" in r


def test_different_wallets_have_different_keys():
    w1 = Wallet.generate()
    w2 = Wallet.generate()
    # Statistically impossible for two random 32-byte keys to match.
    assert w1.private_key_hex != w2.private_key_hex
