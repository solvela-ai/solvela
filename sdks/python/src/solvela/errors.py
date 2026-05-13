"""Solvela error types."""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from solvela.types import AtomicUsdc, PaymentRequired


class ClientError(Exception):
    """Base error for all Solvela client errors."""


class WalletError(ClientError):
    """Error related to wallet operations."""


class SignerError(ClientError):
    """Error related to transaction signing."""


class InsufficientBalanceError(ClientError):
    """Raised when wallet balance is too low for a payment.

    ``have`` and ``need`` are USDC atomic units (1 USDC = 1_000_000). The
    typed ``AtomicUsdc`` annotation prevents accidental conflation with
    human-readable USDC floats inside the SDK.
    """

    def __init__(self, have: AtomicUsdc, need: AtomicUsdc) -> None:
        self.have = have
        self.need = need
        super().__init__(f"Insufficient balance: have {have}, need {need}")


class GatewayError(ClientError):
    """Error returned by the gateway HTTP API."""

    def __init__(self, status: int, message: str) -> None:
        self.status = status
        self.message = message
        super().__init__(f"Gateway error {status}: {message}")


class PaymentRequiredError(ClientError):
    """Raised when the gateway returns 402 Payment Required."""

    def __init__(self, payment_required: PaymentRequired) -> None:
        self.payment_required = payment_required
        total = payment_required.cost_breakdown.total
        super().__init__(f"Payment required: {total} {payment_required.cost_breakdown.currency}")


class PaymentRejectedError(ClientError):
    """Raised when a payment is rejected by the gateway after signing.

    ``payment_required`` carries the second 402 body (cost breakdown, accepted
    schemes, gateway error message) so callers can inspect *why* payment was
    rejected — replay nonce, expired signature, balance issue, etc. — instead
    of getting a hardcoded reason string with no context. Optional for
    backward-compatibility with callers constructing this error from a string
    only.
    """

    def __init__(
        self,
        reason: str,
        payment_required: PaymentRequired | None = None,
    ) -> None:
        self.reason = reason
        self.payment_required = payment_required
        super().__init__(f"Payment rejected: {reason}")


class RecipientMismatchError(ClientError):
    """Raised when the payment recipient does not match expectations."""

    def __init__(self, expected: str, actual: str) -> None:
        self.expected = expected
        self.actual = actual
        super().__init__(f"Recipient mismatch: expected {expected}, got {actual}")


class AmountExceedsMaxError(ClientError):
    """Raised when a payment amount exceeds the configured maximum.

    Both fields are USDC atomic units (typed ``AtomicUsdc``).
    """

    def __init__(self, amount: AtomicUsdc, max_amount: AtomicUsdc) -> None:
        self.amount = amount
        self.max_amount = max_amount
        super().__init__(f"Amount {amount} exceeds max {max_amount}")


class TimeoutError(ClientError):
    """Raised when an operation times out."""

    def __init__(self, timeout_secs: float) -> None:
        self.timeout_secs = timeout_secs
        super().__init__(f"Operation timed out after {timeout_secs}s")
