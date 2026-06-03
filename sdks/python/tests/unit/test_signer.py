"""Unit tests for Signer — interface contract, ATA derivation, sign_payment."""

from __future__ import annotations

import base64

import pytest
from solders.pubkey import Pubkey  # type: ignore[import-untyped]

from solvela.constants import SOLANA_NETWORK, USDC_MINT
from solvela.errors import SignerError
from solvela.signer import KeypairSigner, Signer
from solvela.types import EscrowPayload, PaymentAccept, Resource, SolanaPayload
from solvela.wallet import Wallet

ESCROW_PROGRAM = "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU"


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
        # result must be a plausibly-shaped signed Solana transaction.
        tx_bytes = base64.b64decode(payload.payload.transaction)
        assert tx_bytes[0] == 0x01  # compact-u16 signature count = 1
        assert len(tx_bytes) > 100  # sig(64) + message header/keys/blockhash/ix
        # The SPL transfer amount (1 USDC = 1_000_000 atomic) must appear as a
        # little-endian u64 inside the serialized instruction data.
        assert (1_000_000).to_bytes(8, "little") in tx_bytes

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


def _escrow_accept(escrow_program_id: str | None = ESCROW_PROGRAM) -> PaymentAccept:
    return PaymentAccept(
        scheme="escrow",
        network=SOLANA_NETWORK,
        amount="2625",
        asset=USDC_MINT,
        pay_to="9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
        max_timeout_seconds=300,
        escrow_program_id=escrow_program_id,
    )


def _mock_slot_and_blockhash(httpx_mock, slot: int = 1_000_000) -> None:  # type: ignore[no-untyped-def]
    """Queue getSlot then getLatestBlockhash responses (escrow path order)."""
    httpx_mock.add_response(
        url=_RPC_URL,
        json={"jsonrpc": "2.0", "id": 1, "result": slot},
    )
    httpx_mock.add_response(
        url=_RPC_URL,
        json={
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "context": {"slot": slot},
                "value": {"blockhash": _FAKE_BLOCKHASH, "lastValidBlockHeight": slot},
            },
        },
    )


class TestSchemeBranching:
    """`sign_payment` must branch on `accepted.scheme` with no silent fallback."""

    @pytest.mark.asyncio
    async def test_escrow_scheme_returns_escrow_payload(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        wallet, _ = Wallet.create()
        signer = KeypairSigner(wallet, rpc_url=_RPC_URL)
        _mock_slot_and_blockhash(httpx_mock)

        payload = await signer.sign_payment(
            amount_atomic=2625,
            recipient="9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
            resource=_resource(),
            accepted=_escrow_accept(),
        )

        assert payload.accepted.scheme == "escrow"
        assert isinstance(payload.payload, EscrowPayload)
        # deposit_tx is a non-empty signed tx
        tx_bytes = base64.b64decode(payload.payload.deposit_tx)
        assert tx_bytes[0] == 0x01  # 1 signature
        # service_id is base64 of exactly 32 bytes
        assert len(base64.b64decode(payload.payload.service_id)) == 32
        # agent_pubkey is the wallet's base58 address
        assert payload.payload.agent_pubkey == wallet.address()

    @pytest.mark.asyncio
    async def test_escrow_service_id_is_unique_per_call(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        wallet, _ = Wallet.create()
        signer = KeypairSigner(wallet, rpc_url=_RPC_URL)
        _mock_slot_and_blockhash(httpx_mock)
        _mock_slot_and_blockhash(httpx_mock)

        p1 = await signer.sign_payment(
            amount_atomic=2625,
            recipient="9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
            resource=_resource(),
            accepted=_escrow_accept(),
        )
        p2 = await signer.sign_payment(
            amount_atomic=2625,
            recipient="9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
            resource=_resource(),
            accepted=_escrow_accept(),
        )
        assert isinstance(p1.payload, EscrowPayload)
        assert isinstance(p2.payload, EscrowPayload)
        assert p1.payload.service_id != p2.payload.service_id

    @pytest.mark.asyncio
    async def test_exact_scheme_still_returns_solana_payload(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        # Regression: the exact path is unchanged by the escrow branch.
        wallet, _ = Wallet.create()
        signer = KeypairSigner(wallet, rpc_url=_RPC_URL)
        httpx_mock.add_response(
            url=_RPC_URL,
            json={
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "context": {"slot": 1},
                    "value": {"blockhash": _FAKE_BLOCKHASH, "lastValidBlockHeight": 1},
                },
            },
        )
        payload = await signer.sign_payment(
            amount_atomic=1_000_000,
            recipient="11111111111111111111111111111112",
            resource=_resource(),
            accepted=_accept(),
        )
        assert isinstance(payload.payload, SolanaPayload)
        assert payload.accepted.scheme == "exact"

    @pytest.mark.asyncio
    async def test_escrow_missing_program_id_rejected(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        # No RPC should be hit — rejection happens before any network call.
        wallet, _ = Wallet.create()
        signer = KeypairSigner(wallet, rpc_url=_RPC_URL)
        with pytest.raises(SignerError, match="escrow_program_id is missing"):
            await signer.sign_payment(
                amount_atomic=2625,
                recipient="9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
                resource=_resource(),
                accepted=_escrow_accept(escrow_program_id=None),
            )
        # Rejection must happen before any network call — no getSlot / blockhash
        # RPC may be issued for an escrow payment we can't build.
        assert len(httpx_mock.get_requests()) == 0

    @pytest.mark.asyncio
    async def test_escrow_zero_amount_rejected(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        wallet, _ = Wallet.create()
        signer = KeypairSigner(wallet, rpc_url=_RPC_URL)
        with pytest.raises(SignerError, match="greater than zero"):
            await signer.sign_payment(
                amount_atomic=0,
                recipient="9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
                resource=_resource(),
                accepted=_escrow_accept(),
            )
        # No network call for an amount we reject before building.
        assert len(httpx_mock.get_requests()) == 0

    @pytest.mark.asyncio
    async def test_escrow_negative_amount_rejected(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        wallet, _ = Wallet.create()
        signer = KeypairSigner(wallet, rpc_url=_RPC_URL)
        with pytest.raises(SignerError, match="greater than zero"):
            await signer.sign_payment(
                amount_atomic=-1,
                recipient="9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
                resource=_resource(),
                accepted=_escrow_accept(),
            )
        assert len(httpx_mock.get_requests()) == 0

    @pytest.mark.asyncio
    async def test_escrow_float_amount_rejected(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        # A float amount would be silently truncated into the on-chain u64.
        wallet, _ = Wallet.create()
        signer = KeypairSigner(wallet, rpc_url=_RPC_URL)
        with pytest.raises(SignerError, match="integer"):
            await signer.sign_payment(
                amount_atomic=2625.0,  # type: ignore[arg-type]
                recipient="9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
                resource=_resource(),
                accepted=_escrow_accept(),
            )
        assert len(httpx_mock.get_requests()) == 0

    @pytest.mark.asyncio
    async def test_escrow_stub_low_slot_rejected(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        # A stub/genesis node returning a near-zero slot must be refused, not
        # used to compute an instantly-expired escrow deposit.
        wallet, _ = Wallet.create()
        signer = KeypairSigner(wallet, rpc_url=_RPC_URL)
        httpx_mock.add_response(url=_RPC_URL, json={"jsonrpc": "2.0", "id": 1, "result": 0})
        with pytest.raises(SignerError, match="implausibly low slot"):
            await signer.sign_payment(
                amount_atomic=2625,
                recipient="9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
                resource=_resource(),
                accepted=_escrow_accept(),
            )

    @pytest.mark.asyncio
    async def test_escrow_getslot_429_rejected(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        wallet, _ = Wallet.create()
        signer = KeypairSigner(wallet, rpc_url=_RPC_URL)
        httpx_mock.add_response(url=_RPC_URL, status_code=429, text="<html>Rate limited</html>")
        with pytest.raises(SignerError, match="getSlot RPC HTTP 429"):
            await signer.sign_payment(
                amount_atomic=2625,
                recipient="9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
                resource=_resource(),
                accepted=_escrow_accept(),
            )

    @pytest.mark.asyncio
    async def test_escrow_getslot_malformed_json_rejected(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        wallet, _ = Wallet.create()
        signer = KeypairSigner(wallet, rpc_url=_RPC_URL)
        httpx_mock.add_response(url=_RPC_URL, text="not valid json")
        with pytest.raises(SignerError, match="getSlot RPC: malformed JSON"):
            await signer.sign_payment(
                amount_atomic=2625,
                recipient="9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
                resource=_resource(),
                accepted=_escrow_accept(),
            )

    @pytest.mark.asyncio
    async def test_escrow_getslot_jsonrpc_error_rejected(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        # A JSON-RPC {"error": {...}} body must surface as SignerError carrying
        # only the numeric code, never a silent default slot.
        wallet, _ = Wallet.create()
        signer = KeypairSigner(wallet, rpc_url=_RPC_URL)
        httpx_mock.add_response(
            url=_RPC_URL,
            json={"jsonrpc": "2.0", "id": 1, "error": {"code": -32000, "message": "node behind"}},
        )
        with pytest.raises(SignerError, match="getSlot RPC error"):
            await signer.sign_payment(
                amount_atomic=2625,
                recipient="9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
                resource=_resource(),
                accepted=_escrow_accept(),
            )

    @pytest.mark.asyncio
    async def test_unknown_scheme_rejected(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        wallet, _ = Wallet.create()
        signer = KeypairSigner(wallet, rpc_url=_RPC_URL)
        accept = _accept()
        # Force an out-of-domain scheme past the type system to prove the
        # signer fails closed rather than defaulting to a transfer.
        object.__setattr__(accept, "scheme", "upto")
        with pytest.raises(SignerError, match="Unsupported payment scheme"):
            await signer.sign_payment(
                amount_atomic=1_000_000,
                recipient="11111111111111111111111111111112",
                resource=_resource(),
                accepted=accept,
            )


class TestEscrowExpirySlot:
    def test_typical_300s(self) -> None:
        # 300 s * 2.5 slots/s = 750 slots ahead.
        assert KeypairSigner._escrow_expiry_slot(1_000_000, 300) == 1_000_750

    def test_zero_seconds_applies_min_floor(self) -> None:
        assert KeypairSigner._escrow_expiry_slot(1_000_000, 0) == 1_000_150

    def test_huge_timeout_clamped_to_cap(self) -> None:
        assert KeypairSigner._escrow_expiry_slot(1_000_000, 10**12) == 1_010_000
