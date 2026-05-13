"""Solvela signer — pluggable payment signing interface."""

from __future__ import annotations

from abc import ABC, abstractmethod
from typing import TYPE_CHECKING

from solvela.constants import USDC_MINT, X402_VERSION
from solvela.errors import SignerError
from solvela.types import (
    AtomicUsdc,
    PaymentAccept,
    PaymentPayload,
    Resource,
    SolanaPayload,
)

if TYPE_CHECKING:
    from solders.pubkey import Pubkey  # type: ignore[import-untyped]

    from solvela.wallet import Wallet


class Signer(ABC):
    """Abstract signer interface. Implement this to customize payment signing."""

    @abstractmethod
    async def sign_payment(
        self,
        amount_atomic: AtomicUsdc,
        recipient: str,
        resource: Resource,
        accepted: PaymentAccept,
    ) -> PaymentPayload:
        """Build and sign a payment transaction, return PaymentPayload.

        The returned ``PaymentPayload`` is base64-JSON-encoded by the caller
        and sent to the gateway in the ``Payment-Signature`` header.

        Args:
            amount_atomic: USDC atomic units (1 USDC = 1_000_000). The typed
                ``AtomicUsdc`` annotation forces callers to mark the unit
                conversion explicitly, preventing accidental human-USDC
                values from reaching the on-chain transfer instruction.
        """
        ...


class KeypairSigner(Signer):
    """Default signer that builds real Solana USDC-SPL transfer transactions."""

    def __init__(
        self,
        wallet: Wallet,
        rpc_url: str = "https://api.mainnet-beta.solana.com",
    ) -> None:
        self._wallet = wallet
        self._rpc_url = rpc_url

    async def sign_payment(
        self,
        amount_atomic: AtomicUsdc,
        recipient: str,
        resource: Resource,
        accepted: PaymentAccept,
    ) -> PaymentPayload:
        """Build and sign a USDC-SPL transfer transaction."""
        try:
            import base64

            import httpx
            from solders.hash import Hash as Blockhash
            from solders.instruction import AccountMeta, Instruction
            from solders.keypair import Keypair as SoldersKeypair
            from solders.message import Message
            from solders.pubkey import Pubkey
            from solders.transaction import Transaction

            sender = self._wallet.pubkey()
            recipient_pubkey = Pubkey.from_string(recipient)
            mint = Pubkey.from_string(USDC_MINT)

            # SPL Token program
            token_program = Pubkey.from_string("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")

            # Derive ATAs
            sender_ata = self._derive_ata(sender, mint)
            recipient_ata = self._derive_ata(recipient_pubkey, mint)

            # Get recent blockhash
            async with httpx.AsyncClient(timeout=30) as client:
                resp = await client.post(
                    self._rpc_url,
                    json={
                        "jsonrpc": "2.0",
                        "id": 1,
                        "method": "getLatestBlockhash",
                        "params": [{"commitment": "finalized"}],
                    },
                )
                # Surface HTTP-level failures (rate limit, 5xx, etc.) before
                # attempting JSON decode. An HTML/text body from a 429 or
                # gateway error would otherwise raise `json.JSONDecodeError`
                # uncontextualised and embed the raw response in the
                # traceback.
                if resp.status_code != 200:
                    raise SignerError(f"Blockhash RPC HTTP {resp.status_code}")
                import json

                try:
                    data = resp.json()
                except json.JSONDecodeError as err:
                    raise SignerError("Blockhash RPC: malformed JSON body") from err
                # Guard against missing keys / RPC error responses. Indexing
                # blindly with `data["result"]["value"]["blockhash"]` would
                # raise a `KeyError` whose message embeds the raw RPC payload
                # — leaking node URLs, internal error messages, etc. into
                # tracebacks and Sentry events.
                result = data.get("result", {}).get("value", {}) if isinstance(data, dict) else {}
                blockhash_str = result.get("blockhash") if isinstance(result, dict) else None
                if not blockhash_str:
                    rpc_err = data.get("error") if isinstance(data, dict) else None
                    err_code = rpc_err.get("code") if isinstance(rpc_err, dict) else None
                    raise SignerError(f"RPC did not return a blockhash (code: {err_code})")
                blockhash = Blockhash.from_string(blockhash_str)

            # Build SPL Token transfer instruction
            # Transfer instruction = index 3, data = amount as little-endian u64
            transfer_data = bytes([3]) + amount_atomic.to_bytes(8, "little")
            transfer_ix = Instruction(
                program_id=token_program,
                accounts=[
                    AccountMeta(pubkey=sender_ata, is_signer=False, is_writable=True),
                    AccountMeta(pubkey=recipient_ata, is_signer=False, is_writable=True),
                    AccountMeta(pubkey=sender, is_signer=True, is_writable=False),
                ],
                data=transfer_data,
            )

            # Build and sign transaction
            msg = Message.new_with_blockhash([transfer_ix], sender, blockhash)
            kp_bytes = self._wallet.to_keypair_bytes()
            solders_kp = SoldersKeypair.from_bytes(kp_bytes)
            tx = Transaction.new_unsigned(msg)
            tx.sign([solders_kp], blockhash)

            tx_b64 = base64.b64encode(bytes(tx)).decode()

            return PaymentPayload(
                x402_version=X402_VERSION,
                resource=resource,
                accepted=accepted,
                payload=SolanaPayload(transaction=tx_b64),
            )
        except SignerError:
            raise
        except Exception as e:
            raise SignerError(f"Failed to sign payment: {e}") from e

    @staticmethod
    def _derive_ata(owner: Pubkey, mint: Pubkey) -> Pubkey:
        """Derive Associated Token Account address."""
        from solders.pubkey import Pubkey

        ata_program = Pubkey.from_string("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL")
        token_program = Pubkey.from_string("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")
        # PDA: seeds = [owner, token_program, mint], program = ata_program
        derived, _bump = Pubkey.find_program_address(
            [bytes(owner), bytes(token_program), bytes(mint)],
            ata_program,
        )
        return derived
