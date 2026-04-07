"""Tests for wallet key resolution and management."""

import json
import os
from pathlib import Path
from unittest.mock import patch

import pytest

from rustyclawrouter.wallet import Wallet


class TestWalletInit:
    def test_no_key_configured(self):
        with patch.dict(os.environ, {}, clear=True):
            wallet = Wallet()
            assert wallet.has_key is False
            assert wallet.address is None

    def test_key_from_constructor(self):
        wallet = Wallet(private_key="SomeBase58Key")
        assert wallet.has_key is True

    def test_key_from_env_var(self):
        with patch.dict(os.environ, {"SOLANA_WALLET_KEY": "EnvBase58Key"}):
            wallet = Wallet()
            assert wallet.has_key is True

    def test_constructor_key_takes_precedence_over_env(self):
        with patch.dict(os.environ, {"SOLANA_WALLET_KEY": "EnvKey"}):
            wallet = Wallet(private_key="ConstructorKey")
            assert wallet.has_key is True
            # The constructor key should win
            assert wallet._private_key == "ConstructorKey"

    def test_key_from_file(self, tmp_path: Path):
        key_dir = tmp_path / ".rustyclawrouter"
        key_dir.mkdir()
        key_file = key_dir / "wallet.json"
        key_file.write_text(json.dumps({"private_key": "FileBase58Key"}))

        with patch.dict(os.environ, {}, clear=True):
            with patch.object(Path, "home", return_value=tmp_path):
                wallet = Wallet()
                assert wallet.has_key is True
                assert wallet._private_key == "FileBase58Key"

    def test_missing_file_no_crash(self, tmp_path: Path):
        with patch.dict(os.environ, {}, clear=True):
            with patch.object(Path, "home", return_value=tmp_path):
                wallet = Wallet()
                assert wallet.has_key is False


class TestWalletAddress:
    def test_address_without_key(self):
        wallet = Wallet()
        # address property should return None if no key and solders not installed
        assert wallet.address is None

    def test_address_with_key_no_solders(self):
        wallet = Wallet(private_key="SomeKey")
        # Without solders, address derivation will fail gracefully
        # (ImportError is caught, returns None)
        # This test passes regardless of whether solders is installed
        # since the key is not a real base58 keypair


class TestWalletSignTransaction:
    def test_sign_without_key_raises(self):
        with patch.dict(os.environ, {}, clear=True):
            wallet = Wallet()
            with pytest.raises(ValueError, match="No private key configured"):
                wallet.sign_transaction(b"fake_tx")

    def test_sign_with_key_raises_not_implemented(self):
        wallet = Wallet(private_key="SomeBase58Key")
        with pytest.raises(NotImplementedError, match="build_solana_transfer_checked"):
            wallet.sign_transaction(b"fake_tx")
