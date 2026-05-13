"""Unit tests for Signer — interface contract, ATA derivation, sign_payment."""

from __future__ import annotations

import base64

import pytest
from solders.pubkey import Pubkey  # type: ignore[import-untyped]

from solvela.constants import SOLANA_NETWORK, USDC_MINT
from solvela.errors import SignerError
from solvela.signer import KeypairSigner, Signer
from solvela.types import PaymentAccept, Resource, SolanaPayload
from solvela.wallet import Wallet


class TestSignerInterface:
    def test_signer_is_abstract(self) -> None:
        with pytest.raises(TypeError):
            Signer()  # type: ignore[abstract]

    def test_keypair_signer_implements_signer(self) -> None:
        wallet, _ = Wallet.create()
        ks = KeypairSigner(wallet)
        assert isinstance(ks, Signer)


class TestDeriveAta:
    def test_derive_ata_deterministic(self) -> None:
        owner = Pubkey.from_string("11111111111111111111111111111111")
        mint = Pubkey.from_string(USDC_MINT)
        ata1 = KeypairSigner._derive_ata(owner, mint)
        ata2 = KeypairSigner._derive_ata(owner, mint)
        assert ata1 == ata2

    def test_derive_ata_differs_for_different_owners(self) -> None:
        mint = Pubkey.from_string(USDC_MINT)
        owner_a = Pubkey.from_string("11111111111111111111111111111111")
        owner_b = Pubkey.from_string("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")
        ata_a = KeypairSigner._derive_ata(owner_a, mint)
        ata_b = KeypairSigner._derive_ata(owner_b, mint)
        assert ata_a != ata_b


# A valid Solana base58 32-byte hash, used as the canned blockhash for tests
# that mock the getLatestBlockhash RPC. (Equivalent to the system program ID.)
_FAKE_BLOCKHASH = "11111111111111111111111111111111"
_RPC_URL = "https://rpc.test.local"


def _accept() -> PaymentAccept:
    return PaymentAccept(
        scheme="exact",
        network=SOLANA_NETWORK,
        amount="1000000",  # 1 USDC atomic
        asset=USDC_MINT,
        pay_to="11111111111111111111111111111112",
        max_timeout_seconds=300,
    )


def _resource() -> Resource:
    return Resource(url=f"{_RPC_URL}/v1/chat/completions", method="POST")


class TestSignPayment:
    """Round-trip ``KeypairSigner.sign_payment`` with a mocked blockhash RPC.

    Previously only ``_derive_ata`` was exercised. The actual transaction-
    building hot path (blockhash fetch, SPL transfer instruction encoding,
    base64 serialization) had zero coverage — a regression in the amount
    little-endian encoding or the account ordering would pass all tests.
    """

    @pytest.mark.asyncio
    async def test_sign_payment_returns_solana_payload(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        wallet, _ = Wallet.create()
        signer = KeypairSigner(wallet, rpc_url=_RPC_URL)
        httpx_mock.add_response(
            url=_RPC_URL,
            json={
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "context": {"slot": 1},
                    "value": {
                        "blockhash": _FAKE_BLOCKHASH,
                        "lastValidBlockHeight": 1,
                    },
                },
            },
        )

        payload = await signer.sign_payment(
            amount_atomic=1_000_000,
            recipient="11111111111111111111111111111112",
            resource=_resource(),
            accepted=_accept(),
        )

        assert payload.x402_version == 2
        assert isinstance(payload.payload, SolanaPayload)
        assert payload.accepted.scheme == "exact"

        # The transaction is base64-encoded; decoding must succeed and the
        # result must be a non-empty serialized Solana transaction.
        tx_bytes = base64.b64decode(payload.payload.transaction)
        assert len(tx_bytes) > 0

    @pytest.mark.asyncio
    async def test_sign_payment_raises_on_rpc_429(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        wallet, _ = Wallet.create()
        signer = KeypairSigner(wallet, rpc_url=_RPC_URL)
        # Rate-limited blockhash fetch must surface as SignerError, not as a
        # JSONDecodeError or KeyError that leaks the raw body.
        httpx_mock.add_response(url=_RPC_URL, status_code=429, text="<html>Rate limited</html>")

        with pytest.raises(SignerError, match="HTTP 429"):
            await signer.sign_payment(
                amount_atomic=1_000_000,
                recipient="11111111111111111111111111111112",
                resource=_resource(),
                accepted=_accept(),
            )

    @pytest.mark.asyncio
    async def test_sign_payment_raises_on_malformed_json(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        wallet, _ = Wallet.create()
        signer = KeypairSigner(wallet, rpc_url=_RPC_URL)
        httpx_mock.add_response(url=_RPC_URL, text="not valid json")

        with pytest.raises(SignerError, match="malformed JSON"):
            await signer.sign_payment(
                amount_atomic=1_000_000,
                recipient="11111111111111111111111111111112",
                resource=_resource(),
                accepted=_accept(),
            )

    @pytest.mark.asyncio
    async def test_sign_payment_raises_when_blockhash_missing(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        wallet, _ = Wallet.create()
        signer = KeypairSigner(wallet, rpc_url=_RPC_URL)
        # 200 with a generic RPC error must surface as SignerError carrying
        # the original error code rather than crashing on missing keys.
        httpx_mock.add_response(
            url=_RPC_URL,
            json={
                "jsonrpc": "2.0",
                "id": 1,
                "error": {"code": -32000, "message": "Internal error"},
            },
        )

        with pytest.raises(SignerError, match="did not return a blockhash"):
            await signer.sign_payment(
                amount_atomic=1_000_000,
                recipient="11111111111111111111111111111112",
                resource=_resource(),
                accepted=_accept(),
            )
