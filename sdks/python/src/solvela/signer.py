"""Solvela signer — pluggable payment signing interface."""

from __future__ import annotations

import base64
import json
import secrets
from abc import ABC, abstractmethod
from typing import TYPE_CHECKING

import httpx

from solvela.constants import USDC_MINT, X402_VERSION
from solvela.errors import SignerError
from solvela.escrow import DepositParams, build_deposit_tx
from solvela.types import (
    AtomicUsdc,
    EscrowPayload,
    PaymentAccept,
    PaymentPayload,
    Resource,
    SolanaPayload,
)

# --- Escrow expiry-slot bounds (mirror the Rust SDK's signer.rs) ---
#
# Solana slots are ~400 ms. These bounds are ported verbatim from
# sdks/rust/crates/solvela-client/src/signer.rs so the Python and Rust SDKs
# choose compatible expiry slots and neither is bounced by the gateway.

# Upper bound on how far ahead an escrow may expire (~66 min). Rejects a
# malicious/misconfigured gateway from pushing expiry to "never".
_MAX_ESCROW_EXPIRY_SLOTS_AHEAD = 10_000

# Lower bound on expiry distance. Mirrors AND exceeds the gateway's
# MIN_EXPIRY_BUFFER_SLOTS = 50 (crates/x402/src/escrow/verifier.rs); the extra
# headroom (150 vs 50) absorbs slot-skew between the slot we read and the slot
# the gateway verifies against. ~150 slots ≈ 60 s.
_MIN_ESCROW_EXPIRY_SLOTS_AHEAD = 150

# Floor below which a getSlot result is implausible and rejected. A stub /
# genesis / freshly-reset validator can answer getSlot with a near-zero slot;
# computing an escrow expiry from that base yields an already-expired (or
# trivially-near-expiry) deposit the gateway would bounce. Mainnet/devnet have
# been well past this for years, so a real node never trips it. Fail closed
# rather than silently signing a dead-on-arrival escrow deposit.
_MIN_PLAUSIBLE_SLOT = 1_000_000

if TYPE_CHECKING:
    from solders.pubkey import Pubkey

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
        """Build and sign a payment transaction, branching on the scheme.

        ``exact``  -> SPL-token transfer to the gateway's pay_to ATA
                      (returns a ``SolanaPayload``).
        ``escrow`` -> on-chain deposit into a per-request escrow PDA
                      (returns an ``EscrowPayload``).

        There is NO silent fallback: an ``escrow``-selected payment always
        produces an escrow deposit, never an ``exact`` transfer, and an unknown
        scheme is rejected. Routing an escrow-selected payment as an exact
        transfer is the scheme-mismatch money-path bug this method guards
        against (per the ``solvela-x402`` skill).
        """
        if accepted.scheme == "exact":
            return await self._sign_exact_payment(amount_atomic, recipient, resource, accepted)
        if accepted.scheme == "escrow":
            return await self._sign_escrow_payment(amount_atomic, resource, accepted)
        # `Scheme` is a closed Literal; PaymentAccept.from_dict already rejects
        # unknown wire schemes. This branch fails closed if a new scheme is
        # added to the enum without a financial branch here.
        raise SignerError(f"Unsupported payment scheme: {accepted.scheme!r}")

    async def _sign_exact_payment(
        self,
        amount_atomic: AtomicUsdc,
        recipient: str,
        resource: Resource,
        accepted: PaymentAccept,
    ) -> PaymentPayload:
        """Build and sign a USDC-SPL transfer transaction (``exact`` scheme)."""
        try:
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
            blockhash_str = await self._fetch_latest_blockhash()
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

    async def _sign_escrow_payment(
        self,
        amount_atomic: AtomicUsdc,
        resource: Resource,
        accepted: PaymentAccept,
    ) -> PaymentPayload:
        """Build and sign an escrow ``deposit`` transaction (``escrow`` scheme).

        Generates a fresh CSPRNG ``service_id``, computes the expiry slot from
        the current slot and ``accepted.max_timeout_seconds`` (mirroring the
        Rust SDK), fetches a recent blockhash, and delegates the byte-exact
        transaction construction to the shared, golden-vector-pinned
        ``solvela.escrow.build_deposit_tx``.
        """
        # Fail clearly if escrow was selected but no program ID was offered —
        # never silently fall back to an exact transfer.
        program_id = accepted.escrow_program_id
        if not program_id:
            raise SignerError("escrow scheme selected but escrow_program_id is missing")

        # Reject a non-integer (float/bool) amount BEFORE it reaches the on-chain
        # u64 amount field. ``int(2.9)`` would silently truncate the deposit and
        # mis-charge the agent; ``bool`` is an ``int`` subclass that would
        # deposit 1 atomic unit. Fail closed at the boundary.
        if not (isinstance(amount_atomic, int) and not isinstance(amount_atomic, bool)):
            raise SignerError(
                "escrow deposit amount must be an integer number of atomic USDC units"
            )
        if amount_atomic <= 0:
            raise SignerError("escrow deposit amount must be greater than zero")

        try:
            # Per-request CSPRNG service_id (#118 invariant): two identical
            # requests must never share a service_id -> escrow PDA -> vault ATA.
            service_id = secrets.token_bytes(32)

            current_slot = await self._fetch_current_slot()
            expiry_slot = self._escrow_expiry_slot(current_slot, accepted.max_timeout_seconds)

            from solders.hash import Hash as Blockhash

            blockhash_str = await self._fetch_latest_blockhash()
            recent_blockhash = bytes(Blockhash.from_string(blockhash_str))

            deposit_tx = build_deposit_tx(
                DepositParams(
                    agent_keypair_b58=self._wallet.to_keypair_b58(),
                    provider_wallet_b58=accepted.pay_to,
                    usdc_mint_b58=USDC_MINT,
                    escrow_program_id_b58=program_id,
                    amount=int(amount_atomic),
                    service_id=service_id,
                    expiry_slot=expiry_slot,
                    recent_blockhash=recent_blockhash,
                )
            )

            payload = EscrowPayload(
                deposit_tx=deposit_tx,
                service_id=base64.b64encode(service_id).decode(),
                agent_pubkey=self._wallet.address(),
            )
            return PaymentPayload(
                x402_version=X402_VERSION,
                resource=resource,
                accepted=accepted,
                payload=payload,
            )
        except SignerError:
            raise
        except Exception as e:
            raise SignerError(f"Failed to sign escrow deposit: {e}") from e

    @staticmethod
    def _escrow_expiry_slot(current_slot: int, max_timeout_seconds: int) -> int:
        """Compute the slot at which a now-created escrow PDA should expire.

        Ported from the Rust SDK's ``escrow_expiry_slot``: ~2.5 slots/s
        (1000 ms / 400 ms), clamped into
        ``[_MIN_ESCROW_EXPIRY_SLOTS_AHEAD, _MAX_ESCROW_EXPIRY_SLOTS_AHEAD]``.
        The floor mirrors and exceeds the gateway's MIN_EXPIRY_BUFFER_SLOTS so a
        too-near expiry is never bounced; the cap rejects an unbounded-future
        expiry. ``current_slot`` and the timeout are non-negative here (slot
        comes from getSlot; timeout from a validated PaymentAccept).
        """
        timeout_slots = (max(max_timeout_seconds, 0) * 1000) // 400
        effective = max(
            _MIN_ESCROW_EXPIRY_SLOTS_AHEAD,
            min(timeout_slots, _MAX_ESCROW_EXPIRY_SLOTS_AHEAD),
        )
        return current_slot + effective

    async def _fetch_latest_blockhash(self) -> str:
        """Fetch a recent blockhash string via JSON-RPC ``getLatestBlockhash``.

        Surfaces HTTP-level failures, malformed JSON, and RPC error responses as
        ``SignerError`` *before* any blind indexing — so a 429/5xx HTML body or
        an error payload never leaks node URLs / internals into a traceback.
        """
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
            if resp.status_code != 200:
                raise SignerError(f"Blockhash RPC HTTP {resp.status_code}")
            try:
                data = resp.json()
            except json.JSONDecodeError as err:
                raise SignerError("Blockhash RPC: malformed JSON body") from err
            result = data.get("result", {}).get("value", {}) if isinstance(data, dict) else {}
            blockhash_str = result.get("blockhash") if isinstance(result, dict) else None
            if not blockhash_str:
                rpc_err = data.get("error") if isinstance(data, dict) else None
                err_code = rpc_err.get("code") if isinstance(rpc_err, dict) else None
                raise SignerError(f"RPC did not return a blockhash (code: {err_code})")
            return str(blockhash_str)

    async def _fetch_current_slot(self) -> int:
        """Fetch the current slot via JSON-RPC ``getSlot`` (commitment confirmed).

        Mirrors the Rust SDK's ``get_current_slot``. Fails closed: any HTTP
        failure, malformed body, RPC error, or missing/invalid ``result`` is a
        ``SignerError`` — never a silent default slot, which would let the
        escrow expiry be computed from a bogus base and bounced or mis-set.
        Also rejects an implausibly-low slot (stub/genesis node) so the escrow
        expiry is never computed from a near-zero base.
        """
        async with httpx.AsyncClient(timeout=30) as client:
            resp = await client.post(
                self._rpc_url,
                json={
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "getSlot",
                    "params": [{"commitment": "confirmed"}],
                },
            )
            if resp.status_code != 200:
                raise SignerError(f"getSlot RPC HTTP {resp.status_code}")
            try:
                data = resp.json()
            except json.JSONDecodeError as err:
                raise SignerError("getSlot RPC: malformed JSON body") from err
            rpc_err = data.get("error") if isinstance(data, dict) else None
            if rpc_err is not None:
                err_code = rpc_err.get("code") if isinstance(rpc_err, dict) else None
                raise SignerError(f"getSlot RPC error (code: {err_code})")
            slot = data.get("result") if isinstance(data, dict) else None
            if not isinstance(slot, int) or isinstance(slot, bool) or slot < 0:
                raise SignerError("getSlot RPC: missing or invalid result")
            if slot < _MIN_PLAUSIBLE_SLOT:
                raise SignerError(
                    f"getSlot RPC returned implausibly low slot {slot} "
                    f"(< {_MIN_PLAUSIBLE_SLOT}); refusing to build an escrow deposit"
                )
            return slot

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
