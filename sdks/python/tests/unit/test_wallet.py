"""Tests for Wallet."""

from __future__ import annotations

import pytest

from solvela.errors import WalletError
from solvela.wallet import Wallet


class TestWalletCreate:
    def test_create_returns_wallet_and_mnemonic(self) -> None:
        wallet, mnemonic = Wallet.create()
        assert isinstance(wallet, Wallet)
        assert wallet.address()  # non-empty
        words = mnemonic.split()
        assert len(words) == 12

    def test_from_mnemonic_roundtrip(self) -> None:
        wallet, mnemonic = Wallet.create()
        restored = Wallet.from_mnemonic(mnemonic)
        assert restored.address() == wallet.address()

    def test_from_mnemonic_invalid(self) -> None:
        with pytest.raises(WalletError):
            Wallet.from_mnemonic("not a valid mnemonic phrase at all")

    def test_from_mnemonic_invalid_does_not_leak_phrase(self) -> None:
        # Regression guard: invalid mnemonics must NEVER appear in the
        # exception message — they would otherwise be captured by tracebacks,
        # logger handlers, and Sentry events as plaintext seed material.
        bad_phrase = "ribbon canyon extra zebra obvious banana lurid wood ghost orbit melt vast"
        with pytest.raises(WalletError) as exc_info:
            Wallet.from_mnemonic(bad_phrase)
        msg = str(exc_info.value)
        assert bad_phrase not in msg
        for word in bad_phrase.split():
            assert word not in msg


class TestWalletKeypairBytes:
    def test_from_keypair_bytes(self) -> None:
        wallet, _ = Wallet.create()
        raw = wallet.to_keypair_bytes()
        assert len(raw) == 64
        restored = Wallet.from_keypair_bytes(raw)
        assert restored.address() == wallet.address()


class TestWalletKeypairB58:
    def test_from_keypair_b58(self) -> None:
        wallet, _ = Wallet.create()
        b58 = wallet.to_keypair_b58()
        restored = Wallet.from_keypair_b58(b58)
        assert restored.address() == wallet.address()


class TestWalletEnv:
    def test_from_env(self, monkeypatch: pytest.MonkeyPatch) -> None:
        wallet, _ = Wallet.create()
        monkeypatch.setenv("TEST_WALLET_KEY", wallet.to_keypair_b58())
        loaded = Wallet.from_env("TEST_WALLET_KEY")
        assert loaded.address() == wallet.address()

    def test_from_env_missing(self) -> None:
        with pytest.raises(WalletError):
            Wallet.from_env("DEFINITELY_NOT_SET_WALLET_VAR")


class TestWalletRepr:
    def test_debug_redacts_secrets(self) -> None:
        wallet, _ = Wallet.create()
        r = repr(wallet)
        assert "REDACTED" in r
        # Ensure the full keypair bytes are not in the repr
        b58 = wallet.to_keypair_b58()
        assert b58 not in r
