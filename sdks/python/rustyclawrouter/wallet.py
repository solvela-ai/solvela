"""Wallet management for Solana keypairs."""

import json
import os
from pathlib import Path
from typing import Optional


class Wallet:
    """Manages a Solana keypair for x402 payments.

    Key resolution order: constructor parameter -> SOLANA_WALLET_KEY env var
    -> ~/.rustyclawrouter/wallet.json file -> no key (read-only mode).
    """

    def __init__(self, private_key: Optional[str] = None):
        """Initialize wallet.

        Args:
            private_key: Base58-encoded Solana private key.
                         Falls back to env var or key file if not provided.
        """
        self._private_key = private_key or os.environ.get("SOLANA_WALLET_KEY")
        if not self._private_key:
            key_file = Path.home() / ".rustyclawrouter" / "wallet.json"
            if key_file.exists():
                data = json.loads(key_file.read_text())
                self._private_key = data.get("private_key")

        self._address: Optional[str] = None

    @property
    def has_key(self) -> bool:
        """Whether this wallet has a private key configured."""
        return self._private_key is not None

    @property
    def address(self) -> Optional[str]:
        """Derive the public address from the private key.

        Requires the ``solders`` package (install with
        ``pip install rustyclawrouter[solana]``).
        """
        if self._address:
            return self._address
        if not self._private_key:
            return None
        try:
            from solders.keypair import Keypair  # type: ignore[import-untyped]

            kp = Keypair.from_base58_string(self._private_key)
            self._address = str(kp.pubkey())
            return self._address
        except ImportError:
            return None

    def sign_transaction(self, transaction_bytes: bytes) -> bytes:
        """Sign a serialized Solana transaction with the wallet's private key.

        This is a stub — full implementation requires constructing proper
        Solana versioned transactions with ``solders``.

        Args:
            transaction_bytes: The raw transaction bytes to sign.

        Returns:
            The signed transaction bytes.

        Raises:
            ValueError: If no private key is configured.
            ImportError: If solders is not installed.
        """
        if not self._private_key:
            raise ValueError("No private key configured")
        try:
            from solders.keypair import Keypair  # type: ignore[import-untyped]  # noqa: F401

            # Stub — real signing requires constructing proper Solana tx
            return transaction_bytes
        except ImportError:
            raise ImportError(
                "Install solana extras: pip install rustyclawrouter[solana]"
            )
