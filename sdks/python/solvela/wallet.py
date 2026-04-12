"""Wallet management for Solana keypairs."""

import json
import os
from pathlib import Path
from typing import Optional


class Wallet:
    """Manages a Solana keypair for x402 payments.

    Key resolution order: constructor parameter -> SOLANA_WALLET_KEY env var
    -> ~/.solvela/wallet.json file -> no key (read-only mode).
    """

    def __init__(self, private_key: Optional[str] = None):
        """Initialize wallet.

        Args:
            private_key: Base58-encoded Solana private key.
                         Falls back to env var or key file if not provided.
        """
        self._private_key = private_key or os.environ.get("SOLANA_WALLET_KEY")
        if not self._private_key:
            key_file = Path.home() / ".solvela" / "wallet.json"
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
        ``pip install solvela[solana]``).
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
        """Sign a serialized Solana transaction.

        .. deprecated::
            Use ``build_solana_transfer_checked()`` from ``x402`` module instead.

        Args:
            transaction_bytes: The raw transaction bytes to sign.

        Raises:
            ValueError: If no private key is configured.
            NotImplementedError: Always — direct transaction signing is not supported.
        """
        if not self._private_key:
            raise ValueError("No private key configured")
        raise NotImplementedError(
            "Direct transaction signing is not supported. "
            "Use solvela.x402.build_solana_transfer_checked() instead."
        )
