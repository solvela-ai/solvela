"""x402 protocol helpers — payment signing and Solana transaction creation."""

import base64
import hashlib
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

    Requires: ``pip install rustyclawrouter[solana]``

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
            "Solana signing requires: pip install rustyclawrouter[solana]"
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


def build_escrow_deposit(
    escrow_program_id: str,
    pay_to: str,
    amount: int,
    service_id: bytes,
    private_key: str,
    max_timeout_seconds: int = 120,
) -> str:
    """Build and sign an Anchor escrow deposit transaction.

    Requires: ``pip install rustyclawrouter[solana]``

    Environment Variables:
        SOLANA_RPC_URL: Solana RPC endpoint URL (required).

    Args:
        escrow_program_id: The Anchor escrow program public key (base58).
        pay_to: Gateway recipient wallet address (base58) — used as the service authority.
        amount: Amount in atomic USDC (6 decimals).
        service_id: 32-byte service identifier for PDA derivation.
        private_key: Base58-encoded Solana keypair.

    Returns:
        Base64-encoded serialized VersionedTransaction.

    Raises:
        ImportError: If solders/solana packages not installed.
        SigningError: If SOLANA_RPC_URL not set, amount invalid, or signing fails.
    """
    try:
        from solana.rpc.api import Client as SolanaClient
        from solders.instruction import AccountMeta, Instruction
        from solders.keypair import Keypair
        from solders.message import MessageV0
        from solders.pubkey import Pubkey
        from solders.system_program import ID as SYSTEM_PROGRAM_ID
        from solders.transaction import VersionedTransaction
        from spl.token.constants import (
            ASSOCIATED_TOKEN_PROGRAM_ID,
            TOKEN_PROGRAM_ID,
        )
    except ImportError:
        raise ImportError(
            "Solana signing requires: pip install rustyclawrouter[solana]"
        )

    rpc_url = os.environ.get("SOLANA_RPC_URL")
    if not rpc_url:
        raise SigningError(
            "SOLANA_RPC_URL environment variable required for on-chain signing."
        )

    if amount <= 0:
        raise SigningError(f"Payment amount must be positive, got: {amount}")

    try:
        kp = Keypair.from_base58_string(private_key)
        program_id = Pubkey.from_string(escrow_program_id)
        usdc_mint = Pubkey.from_string(USDC_MINT)
        recipient = Pubkey.from_string(pay_to)

        # Derive escrow PDA: ["escrow", agent_pubkey, service_id]
        escrow_pda, _ = Pubkey.find_program_address(
            [b"escrow", bytes(kp.pubkey()), service_id],
            program_id,
        )

        # Derive ATAs
        agent_ata, _ = Pubkey.find_program_address(
            [bytes(kp.pubkey()), bytes(TOKEN_PROGRAM_ID), bytes(usdc_mint)],
            ASSOCIATED_TOKEN_PROGRAM_ID,
        )
        escrow_ata, _ = Pubkey.find_program_address(
            [bytes(escrow_pda), bytes(TOKEN_PROGRAM_ID), bytes(usdc_mint)],
            ASSOCIATED_TOKEN_PROGRAM_ID,
        )

        # Fetch slot for expiry and recent blockhash
        client = SolanaClient(rpc_url)
        blockhash_resp = client.get_latest_blockhash()
        blockhash = blockhash_resp.value.blockhash
        slot_resp = client.get_slot()
        current_slot = slot_resp.value
        timeout_slots = max((max_timeout_seconds * 1000) // 400, 10)
        expiry_slot = current_slot + timeout_slots

        # Build Anchor deposit discriminator: sha256("global:deposit")[:8]
        discriminator = hashlib.sha256(b"global:deposit").digest()[:8]

        # Encode instruction data: discriminator + amount (u64 le) + service_id (32 bytes) + expiry_slot (u64 le)
        ix_data = (
            discriminator
            + amount.to_bytes(8, "little")
            + service_id[:32].ljust(32, b"\x00")[:32]
            + expiry_slot.to_bytes(8, "little")
        )

        ix = Instruction(
            program_id=program_id,
            data=bytes(ix_data),
            accounts=[
                AccountMeta(pubkey=kp.pubkey(), is_signer=True, is_writable=True),   # 0: agent
                AccountMeta(pubkey=recipient, is_signer=False, is_writable=False),    # 1: provider
                AccountMeta(pubkey=usdc_mint, is_signer=False, is_writable=False),    # 2: mint
                AccountMeta(pubkey=escrow_pda, is_signer=False, is_writable=True),    # 3: escrow PDA
                AccountMeta(pubkey=agent_ata, is_signer=False, is_writable=True),     # 4: agent ATA
                AccountMeta(pubkey=escrow_ata, is_signer=False, is_writable=True),    # 5: vault ATA
                AccountMeta(pubkey=TOKEN_PROGRAM_ID, is_signer=False, is_writable=False),          # 6
                AccountMeta(pubkey=ASSOCIATED_TOKEN_PROGRAM_ID, is_signer=False, is_writable=False),  # 7
                AccountMeta(pubkey=SYSTEM_PROGRAM_ID, is_signer=False, is_writable=False),            # 8
            ],
        )

        msg = MessageV0.try_compile(
            payer=kp.pubkey(),
            instructions=[ix],
            address_lookup_table_accounts=[],
            recent_blockhash=blockhash,
        )
        tx = VersionedTransaction(msg, [kp])
        return base64.b64encode(bytes(tx)).decode()

    except SigningError:
        raise
    except (ValueError, TypeError, AttributeError) as e:
        raise SigningError(f"Failed to build escrow deposit transaction: {e}") from e


def build_escrow_payment_payload(
    deposit_tx: str,
    service_id: bytes,
    agent_pubkey: str,
) -> Dict[str, Any]:
    """Build the escrow payment payload dict.

    Args:
        deposit_tx: Base64-encoded signed escrow deposit transaction.
        service_id: 32-byte service identifier (will be hex-encoded).
        agent_pubkey: Agent's Solana public key (base58).

    Returns:
        A dict with deposit_tx, service_id, and agent_pubkey.
    """
    return {
        "deposit_tx": deposit_tx,
        "service_id": base64.b64encode(service_id).decode(),
        "agent_pubkey": agent_pubkey,
    }


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
    request_body: Optional[bytes] = None,
) -> str:
    """Create the base64-encoded ``PAYMENT-SIGNATURE`` header value.

    When a ``private_key`` is provided and Solana dependencies are installed,
    builds and signs a real transaction. For the ``escrow`` scheme, builds an
    Anchor escrow deposit transaction. For the ``exact`` scheme, builds a
    USDC-SPL TransferChecked transaction. Falls back to stub values when no
    private key is provided (development/testing mode).

    Args:
        accept: The accepted payment terms from the 402 response.
        resource_url: The URL of the resource being purchased.
        resource_method: The HTTP method (default ``POST``).
        private_key: Base58-encoded Solana private key. If None, uses stub tx.
        request_body: Raw request body bytes used for service_id generation
            in the escrow scheme. If None, a zero-length body is used.

    Returns:
        Base64-encoded JSON string suitable for the PAYMENT-SIGNATURE header.

    Raises:
        SigningError: If private key is provided but signing fails.
    """
    if accept.scheme == "escrow" and accept.escrow_program_id:
        body = request_body or b""
        service_id = hashlib.sha256(body + os.urandom(8)).digest()

        if private_key:
            try:
                amount = int(accept.amount)
            except (ValueError, TypeError) as e:
                raise SigningError(
                    f"Payment amount '{accept.amount}' must be an integer (atomic USDC units): {e}"
                ) from e
            try:
                from solders.keypair import Keypair

                kp = Keypair.from_base58_string(private_key)
                agent_pubkey = str(kp.pubkey())
                deposit_tx = build_escrow_deposit(
                    escrow_program_id=accept.escrow_program_id,
                    pay_to=accept.pay_to,
                    amount=amount,
                    service_id=service_id,
                    private_key=private_key,
                    max_timeout_seconds=accept.max_timeout_seconds,
                )
            except ImportError:
                raise ImportError(
                    "Private key was provided but Solana signing packages are not installed. "
                    "Install with: pip install rustyclawrouter[solana]"
                )
            # SigningError propagates — caller must handle
        else:
            logger.warning(
                "No private key provided — using stub escrow payload. "
                "Payments will be rejected by the gateway."
            )
            deposit_tx = "STUB_ESCROW_DEPOSIT_TX"
            agent_pubkey = "STUB_AGENT_PUBKEY"

        escrow_payload = build_escrow_payment_payload(
            deposit_tx=deposit_tx,
            service_id=service_id,
            agent_pubkey=agent_pubkey,
        )
        outer = {
            "x402_version": X402_VERSION,
            "resource": {"url": resource_url, "method": resource_method},
            "accepted": accept.model_dump(),
            "payload": escrow_payload,
        }
        return base64.b64encode(json.dumps(outer).encode()).decode()

    # --- exact scheme (default) ---
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
                "Install with: pip install rustyclawrouter[solana]"
            )
        # SigningError propagates — caller must handle
    else:
        logger.warning(
            "No private key provided — using stub transaction. "
            "Payments will be rejected by the gateway."
        )

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
