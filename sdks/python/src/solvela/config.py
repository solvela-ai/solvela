"""Solvela client configuration and builder."""

from __future__ import annotations

from dataclasses import dataclass, field
from urllib.parse import urlparse

from solvela.constants import MAINNET_ESCROW_PROGRAM_ID
from solvela.errors import ClientError
from solvela.types import AtomicUsdc

# Local-only hosts that are allowed to use http:// without HTTPS enforcement.
# Anything else must use https:// so payment-signing traffic and the wallet
# address are not exposed to passive network observers.
_LOCAL_HOSTS = frozenset({"localhost", "127.0.0.1", "::1"})

# Default cap on a single payment, in USDC atomic units (1 USDC = 1_000_000).
# Set conservatively so a malicious or misconfigured gateway cannot drain a
# wallet without the caller explicitly opting in to a higher limit.
DEFAULT_MAX_PAYMENT_AMOUNT: AtomicUsdc = AtomicUsdc(10_000_000)  # 10 USDC


def _validate_https_url(label: str, value: str) -> None:
    """Reject http:// URLs unless the host is a recognized local loopback.

    Applied to both ``gateway_url`` and ``rpc_url`` so that payment-signing
    traffic, the wallet address, and Solana blockhash fetches are not exposed
    to passive observers or on-path attackers who could tamper with a returned
    blockhash and redirect the signed transaction.

    Raises ``ClientError`` (not ``ValueError``) so configuration failures stay
    inside the SDK's typed error hierarchy and are caught by callers using
    ``except ClientError`` around construction.
    """
    if not value.startswith("http://"):
        return
    host = urlparse(value).hostname or ""
    if host not in _LOCAL_HOSTS:
        raise ClientError(
            f"{label} must use https:// for non-local endpoints (got http:// host {host!r})"
        )


@dataclass
class ClientConfig:
    """Configuration for the Solvela client.

    Notes:
        ``gateway_url`` defaults to the production HTTPS endpoint. Plain
        ``http://`` URLs are only accepted when pointing at localhost / loopback;
        any other ``http://`` value will raise ``ClientError`` at construction.

        ``max_payment_amount`` defaults to 10 USDC (10_000_000 atomic units) so
        a hostile or buggy gateway cannot silently drain the wallet. Callers
        that genuinely need higher limits must set this explicitly.

        ``expected_escrow_program_id`` is pinned to
        ``MAINNET_ESCROW_PROGRAM_ID`` by default so an escrow deposit is never
        signed for an unexpected program (mirrors ``expected_recipient`` for the
        escrow path). Set to ``None`` to disable the pin.
    """

    gateway_url: str = "https://api.solvela.ai"
    rpc_url: str = "https://api.mainnet-beta.solana.com"
    prefer_escrow: bool = False
    timeout: float = 180.0
    expected_recipient: str | None = None
    expected_escrow_program_id: str | None = MAINNET_ESCROW_PROGRAM_ID
    max_payment_amount: AtomicUsdc | None = field(default=DEFAULT_MAX_PAYMENT_AMOUNT)
    enable_cache: bool = False
    enable_sessions: bool = False
    session_ttl: float = 1800.0
    enable_quality_check: bool = False
    max_quality_retries: int = 1
    free_fallback_model: str | None = None

    def __post_init__(self) -> None:
        _validate_https_url("gateway_url", self.gateway_url)
        _validate_https_url("rpc_url", self.rpc_url)


class ClientBuilder:
    """Fluent builder for ClientConfig."""

    def __init__(self) -> None:
        self._config = ClientConfig()

    def gateway_url(self, value: str) -> ClientBuilder:
        _validate_https_url("gateway_url", value)
        self._config.gateway_url = value
        return self

    def rpc_url(self, value: str) -> ClientBuilder:
        _validate_https_url("rpc_url", value)
        self._config.rpc_url = value
        return self

    def prefer_escrow(self, value: bool) -> ClientBuilder:
        self._config.prefer_escrow = value
        return self

    def timeout(self, value: float) -> ClientBuilder:
        self._config.timeout = value
        return self

    def expected_recipient(self, value: str | None) -> ClientBuilder:
        self._config.expected_recipient = value
        return self

    def expected_escrow_program_id(self, value: str | None) -> ClientBuilder:
        """Pin the escrow program ID the client will sign a deposit for.

        A 402 advertising an escrow scheme whose ``escrow_program_id`` differs is
        rejected before signing. Defaults to ``MAINNET_ESCROW_PROGRAM_ID`` (see
        ``ClientConfig``); use this to point at a different deployment.
        """
        self._config.expected_escrow_program_id = value
        return self

    def disable_escrow_program_pin(self) -> ClientBuilder:
        """Clear the escrow-program pin, allowing any advertised escrow program.

        This removes a security guarantee; prefer ``expected_escrow_program_id``
        to point at a non-default deployment instead.
        """
        self._config.expected_escrow_program_id = None
        return self

    def max_payment_amount(self, value: AtomicUsdc | None) -> ClientBuilder:
        self._config.max_payment_amount = value
        return self

    def enable_cache(self, value: bool) -> ClientBuilder:
        self._config.enable_cache = value
        return self

    def enable_sessions(self, value: bool) -> ClientBuilder:
        self._config.enable_sessions = value
        return self

    def session_ttl(self, value: float) -> ClientBuilder:
        self._config.session_ttl = value
        return self

    def enable_quality_check(self, value: bool) -> ClientBuilder:
        self._config.enable_quality_check = value
        return self

    def max_quality_retries(self, value: int) -> ClientBuilder:
        self._config.max_quality_retries = value
        return self

    def free_fallback_model(self, value: str | None) -> ClientBuilder:
        self._config.free_fallback_model = value
        return self

    def build(self) -> ClientConfig:
        """Build and return the configuration."""
        return self._config
