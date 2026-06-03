"""Solvela wallet — Solana keypair management."""

from __future__ import annotations

import os
from typing import TYPE_CHECKING

from mnemonic import Mnemonic
from solders.keypair import Keypair

from solvela.errors import WalletError

if TYPE_CHECKING:
    from solders.pubkey import Pubkey

_BIP39 = Mnemonic("english")


class Wallet:
    """Thin wrapper around a Solana keypair with BIP39 mnemonic support."""

    def __init__(self, keypair: Keypair) -> None:
        self._keypair = keypair

    # ------------------------------------------------------------------
    # Construction helpers
    # ------------------------------------------------------------------

    @classmethod
    def create(cls) -> tuple[Wallet, str]:
        """Generate a new wallet with a 12-word BIP39 mnemonic.

        Returns:
            A tuple of (Wallet, mnemonic_phrase).
        """
        phrase = _BIP39.generate(128)
        wallet = cls._from_mnemonic_unchecked(phrase)
        return wallet, phrase

    @classmethod
    def from_mnemonic(cls, phrase: str) -> Wallet:
        """Restore a wallet from a BIP39 mnemonic phrase.

        Raises:
            WalletError: If the mnemonic is invalid.
        """
        if not _BIP39.check(phrase):
            # Do NOT include the phrase in the error — it would leak the seed
            # to any logger, traceback dump, or Sentry event that captures it.
            raise WalletError("Invalid BIP39 mnemonic — check word count and spelling")
        return cls._from_mnemonic_unchecked(phrase)

    @classmethod
    def from_keypair_bytes(cls, raw: bytes) -> Wallet:
        """Import from raw 64-byte keypair.

        Raises:
            WalletError: If the bytes are not a valid keypair.
        """
        try:
            return cls(Keypair.from_bytes(raw))
        except Exception:
            # The raw input IS secret key material, and some solders error
            # variants echo the offending input. Surface a generic static
            # message only and drop the chain (`from None`) — never interpolate
            # the exception or the input, which a logger / Sentry would capture
            # as plaintext key bytes. Mirrors build_deposit_tx's keypair-parse fix.
            raise WalletError("Invalid keypair bytes: bad length or encoding") from None

    @classmethod
    def from_keypair_b58(cls, b58: str) -> Wallet:
        """Import from a base58-encoded keypair string.

        Raises:
            WalletError: If the string is not a valid base58 keypair.
        """
        try:
            return cls(Keypair.from_base58_string(b58))
        except Exception:
            # The base58 string IS secret key material, and some solders error
            # variants echo the offending input. Surface a generic static
            # message only and drop the chain (`from None`) — never interpolate
            # the exception or the input, which a logger / Sentry would capture
            # as a plaintext key. Mirrors build_deposit_tx's keypair-parse fix.
            raise WalletError("Invalid base58 keypair: bad base58 or wrong length") from None

    @classmethod
    def from_env(cls, var: str) -> Wallet:
        """Load a keypair from an environment variable (base58-encoded).

        Raises:
            WalletError: If the variable is not set or contains an invalid keypair.
        """
        value = os.environ.get(var)
        if value is None:
            raise WalletError(f"Environment variable {var!r} is not set")
        return cls.from_keypair_b58(value)

    # ------------------------------------------------------------------
    # Accessors
    # ------------------------------------------------------------------

    def address(self) -> str:
        """Return the base58-encoded public key."""
        return str(self._keypair.pubkey())

    def pubkey(self) -> Pubkey:
        """Return the Solana ``Pubkey`` object."""
        return self._keypair.pubkey()

    def to_keypair_bytes(self) -> bytes:
        """Return the raw 64-byte keypair (secret + public)."""
        return bytes(self._keypair)

    def to_keypair_b58(self) -> str:
        """Return the base58-encoded keypair string."""
        return str(self._keypair)

    # ------------------------------------------------------------------
    # Internals
    # ------------------------------------------------------------------

    @classmethod
    def _from_mnemonic_unchecked(cls, phrase: str) -> Wallet:
        """Derive a keypair from a mnemonic without validation."""
        seed = _BIP39.to_seed(phrase, "")
        keypair = Keypair.from_seed(seed[:32])
        return cls(keypair)

    def __repr__(self) -> str:
        return f"Wallet(address={self.address()}, secret=REDACTED)"
