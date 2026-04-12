"""x402 protocol helpers — payment signing and Solana transaction creation."""

import base64
import json
import logging
import os
from typing import Any, Dict, Optional

from .config import USDC_MINT, X402_VERSION
from .types import PaymentAccept

logger = logging.getLogger(__name__)

USDC_DECIMALS = 6


class SigningError(Exception):
    """Raised when Solana transaction signing fails."""

    pass


def build_solana_transfer_checked(
    pay_to: str,
    amount: int,
    private_key: str,
) -> str:
    """Build and sign a USDC-SPL TransferChecked versioned transaction.

    Requires: ``pip install solvela[solana]``

    Environment Variables:
        SOLANA_RPC_URL: Solana RPC endpoint URL (required, e.g. https://api.mainnet-beta.solana.com).

    Args:
        pay_to: Gateway recipient wallet address (base58).
        amount: Amount in atomic USDC (6 decimals), e.g. 2625 = $0.002625.
        private_key: Base58-encoded Solana keypair (64-byte seed+pubkey or 32-byte secret key).

    Returns:
        Base64-encoded serialized VersionedTransaction.

    Raises:
        ImportError: If solders/solana packages not installed.
        SigningError: If SOLANA_RPC_URL not set, amount invalid, or signing fails.
    """
    try:
        from solana.rpc.api import Client as SolanaClient
        from solders.hash import Hash  # noqa: F401
        from solders.instruction import AccountMeta, Instruction  # noqa: F401
        from solders.keypair import Keypair
        from solders.message import MessageV0
        from solders.pubkey import Pubkey
        from solders.transaction import VersionedTransaction
        from spl.token.constants import (
            ASSOCIATED_TOKEN_PROGRAM_ID,
            TOKEN_PROGRAM_ID,
        )
        from spl.token.instructions import (
            TransferCheckedParams,
            transfer_checked,
        )
    except ImportError:
        raise ImportError(
            "Solana signing requires: pip install solvela[solana]"
        )

    rpc_url = os.environ.get("SOLANA_RPC_URL")
    if not rpc_url:
        raise SigningError(
            "SOLANA_RPC_URL environment variable required for on-chain signing. "
            "Set it to your RPC endpoint (e.g. https://api.mainnet-beta.solana.com)"
        )

    if amount <= 0:
        raise SigningError(f"Payment amount must be positive, got: {amount}")

    try:
        kp = Keypair.from_base58_string(private_key)
        usdc_mint = Pubkey.from_string(USDC_MINT)
        recipient = Pubkey.from_string(pay_to)

        # Derive Associated Token Accounts
        sender_ata, _ = Pubkey.find_program_address(
            [bytes(kp.pubkey()), bytes(TOKEN_PROGRAM_ID), bytes(usdc_mint)],
            ASSOCIATED_TOKEN_PROGRAM_ID,
        )
        recipient_ata, _ = Pubkey.find_program_address(
            [bytes(recipient), bytes(TOKEN_PROGRAM_ID), bytes(usdc_mint)],
            ASSOCIATED_TOKEN_PROGRAM_ID,
        )

        # Fetch recent blockhash
        client = SolanaClient(rpc_url)
        blockhash_resp = client.get_latest_blockhash()
        blockhash = blockhash_resp.value.blockhash

        # Build TransferChecked instruction
        ix = transfer_checked(
            TransferCheckedParams(
                program_id=TOKEN_PROGRAM_ID,
                source=sender_ata,
                mint=usdc_mint,
                dest=recipient_ata,
                owner=kp.pubkey(),
                amount=amount,
                decimals=USDC_DECIMALS,
            )
        )

        # Build versioned transaction (v0)
        msg = MessageV0.try_compile(
            payer=kp.pubkey(),
            instructions=[ix],
            address_lookup_table_accounts=[],
            recent_blockhash=blockhash,
        )
        tx = VersionedTransaction(msg, [kp])

        # Serialize to base64
        serialized = bytes(tx)
        return base64.b64encode(serialized).decode()

    except SigningError:
        raise
    except Exception as e:
        raise SigningError(f"Failed to build Solana payment transaction: {e}") from e


def build_payment_payload(
    accept: PaymentAccept,
    resource_url: str,
    *,
    transaction_b64: str,
    resource_method: str = "POST",
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
    private_key: Optional[str] = None,
) -> str:
    """Create the base64-encoded ``PAYMENT-SIGNATURE`` header value.

    When a ``private_key`` is provided and Solana dependencies are installed,
    builds and signs a real USDC-SPL TransferChecked transaction. Otherwise
    falls back to a stub transaction for development/testing.

    Args:
        accept: The accepted payment terms from the 402 response.
        resource_url: The URL of the resource being purchased.
        resource_method: The HTTP method (default ``POST``).
        private_key: Base58-encoded Solana private key. If None, uses stub tx.

    Returns:
        Base64-encoded JSON string suitable for the PAYMENT-SIGNATURE header.

    Raises:
        SigningError: If private key is provided but signing fails.
    """
    transaction_b64 = "STUB_BASE64_TX"

    if private_key:
        try:
            amount = int(accept.amount)
        except (ValueError, TypeError) as e:
            raise SigningError(
                f"Payment amount '{accept.amount}' must be an integer (atomic USDC units): {e}"
            ) from e
        try:
            transaction_b64 = build_solana_transfer_checked(
                pay_to=accept.pay_to,
                amount=amount,
                private_key=private_key,
            )
        except ImportError:
            raise ImportError(
                "Private key was provided but Solana signing packages are not installed. "
                "Install with: pip install solvela[solana]"
            )
        # SigningError propagates — caller must handle

    payload = build_payment_payload(
        accept, resource_url, transaction_b64=transaction_b64, resource_method=resource_method
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
