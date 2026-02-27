"""x402 protocol helpers — payment signing and Solana transaction creation."""

import base64
import json
from typing import Any, Dict

from .config import X402_VERSION
from .types import PaymentAccept


def build_payment_payload(
    accept: PaymentAccept,
    resource_url: str,
    resource_method: str = "POST",
    transaction_b64: str = "STUB_BASE64_TX",
) -> Dict[str, Any]:
    """Build the x402 PaymentPayload dict.

    Args:
        accept: The accepted payment terms from the 402 response.
        resource_url: The URL of the resource being purchased.
        resource_method: The HTTP method (default ``POST``).
        transaction_b64: Base64-encoded signed Solana transaction.

    Returns:
        A dict representing the PaymentPayload.
    """
    return {
        "x402_version": X402_VERSION,
        "resource": {"url": resource_url, "method": resource_method},
        "accepted": accept.model_dump(),
        "payload": {"transaction": transaction_b64},
    }


def encode_payment_header(
    accept: PaymentAccept,
    resource_url: str,
    resource_method: str = "POST",
    transaction_b64: str = "STUB_BASE64_TX",
) -> str:
    """Create the base64-encoded ``PAYMENT-SIGNATURE`` header value.

    Args:
        accept: The accepted payment terms from the 402 response.
        resource_url: The URL of the resource being purchased.
        resource_method: The HTTP method (default ``POST``).
        transaction_b64: Base64-encoded signed Solana transaction.

    Returns:
        Base64-encoded JSON string suitable for the header.
    """
    payload = build_payment_payload(
        accept, resource_url, resource_method, transaction_b64
    )
    payload_json = json.dumps(payload)
    return base64.b64encode(payload_json.encode()).decode()


def decode_payment_header(header_value: str) -> Dict[str, Any]:
    """Decode a ``PAYMENT-SIGNATURE`` header back to a dict.

    Args:
        header_value: The base64-encoded header value.

    Returns:
        The decoded PaymentPayload dict.
    """
    raw = base64.b64decode(header_value)
    return json.loads(raw)
