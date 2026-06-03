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

    def test_from_keypair_bytes_invalid_does_not_leak_input(self) -> None:
        # Regression guard (M-1): the raw bytes ARE secret key material, so the
        # error message must be a generic static string with no fragment of the
        # offending input or the underlying solders exception interpolated.
        bad = bytes(range(40))  # wrong length -> rejected
        with pytest.raises(WalletError) as exc_info:
            Wallet.from_keypair_bytes(bad)
        msg = str(exc_info.value)
        assert bad.hex() not in msg
        assert "40" not in msg  # no echoed length detail
        assert exc_info.value.__cause__ is None  # chain dropped (from None)


class TestWalletKeypairB58:
    def test_from_keypair_b58(self) -> None:
        wallet, _ = Wallet.create()
        b58 = wallet.to_keypair_b58()
        restored = Wallet.from_keypair_b58(b58)
        assert restored.address() == wallet.address()

    def test_from_keypair_b58_invalid_does_not_leak_input(self) -> None:
        # Regression guard (M-1): the base58 string IS secret key material, so
        # the error message must be a generic static string with no fragment of
        # the offending input or the underlying solders exception interpolated.
        bad_b58 = "5KQwrPbwdL6PhXujxW37FSSQ" + "Z" * 60  # not a valid keypair
        with pytest.raises(WalletError) as exc_info:
            Wallet.from_keypair_b58(bad_b58)
        msg = str(exc_info.value)
        assert bad_b58 not in msg
        # No long substring of the input should survive into the message.
        assert bad_b58[:16] not in msg
        assert exc_info.value.__cause__ is None  # chain dropped (from None)


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
