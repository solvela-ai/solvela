"""Unit tests for SolvelaClient — construction and basic properties."""

from __future__ import annotations

import pytest

from solvela.client import SolvelaClient
from solvela.config import ClientConfig
from solvela.constants import SOLANA_NETWORK, USDC_MINT
from solvela.errors import ClientError
from solvela.types import CostBreakdown, PaymentAccept, PaymentRequired, Resource
from solvela.wallet import Wallet


def _accept(
    *,
    network: str = SOLANA_NETWORK,
    asset: str = USDC_MINT,
    amount: str = "1000",
    pay_to: str = "RecipientPubkey111111111111111111111111111",
) -> PaymentAccept:
    return PaymentAccept(
        scheme="exact",
        network=network,
        amount=amount,
        asset=asset,
        pay_to=pay_to,
        max_timeout_seconds=300,
    )


class TestClientCreation:
    def test_client_creation(self) -> None:
        config = ClientConfig(gateway_url="https://example.com")
        client = SolvelaClient(config=config)
        assert client._config.gateway_url == "https://example.com"
        assert client._wallet is None
        assert client._signer is None

    def test_client_creation_with_wallet(self) -> None:
        wallet, _ = Wallet.create()
        config = ClientConfig()
        client = SolvelaClient(config=config, wallet=wallet)
        assert client._wallet is wallet

    def test_client_last_known_balance_initially_none(self) -> None:
        client = SolvelaClient()
        assert client.last_known_balance() is None

    def test_client_debug_redacts(self) -> None:
        client = SolvelaClient()
        r = repr(client)
        assert "REDACTED" in r
        assert "wallet" not in r.replace("wallet=REDACTED", "")


class TestBalanceStateSetter:
    def test_balance_state_setter_updates_balance(self) -> None:
        client = SolvelaClient()
        setter = client.balance_state_setter()
        assert client.last_known_balance() is None
        setter(42.5)
        assert client.last_known_balance() == 42.5
        setter(0.0)
        assert client.last_known_balance() == 0.0


class TestValidatePayment:
    """`_validate_payment` must catch network/asset spoofing before signing."""

    def _client(self) -> SolvelaClient:
        return SolvelaClient(config=ClientConfig(max_payment_amount=None))

    def test_accepts_canonical_solana_usdc(self) -> None:
        # Should not raise.
        self._client()._validate_payment(_accept())

    def test_rejects_unexpected_network(self) -> None:
        client = self._client()
        with pytest.raises(ClientError, match="payment network"):
            client._validate_payment(_accept(network="ethereum:1"))

    def test_rejects_unexpected_asset(self) -> None:
        client = self._client()
        # Use a fake mint — must be rejected even though all other fields match.
        bogus_mint = "So11111111111111111111111111111111111111112"
        with pytest.raises(ClientError, match="Unexpected payment asset"):
            client._validate_payment(_accept(asset=bogus_mint))

    def test_rejects_unexpected_asset_does_not_echo_value(self) -> None:
        # Defensive: malicious mint addresses must not be echoed back into logs.
        client = self._client()
        bogus_mint = "MaliciousMint1111111111111111111111111111111"
        try:
            client._validate_payment(_accept(asset=bogus_mint))
        except ClientError as exc:
            assert bogus_mint not in str(exc)
        else:
            pytest.fail("expected ClientError")

    def test_returns_parsed_atomic_amount(self) -> None:
        # Single-source-of-truth parse: callers receive the validated value
        # rather than re-parsing the wire string at every sign site.
        amount = self._client()._validate_payment(_accept(amount="50000"))
        assert amount == 50000
        assert isinstance(amount, int)

    def test_rejects_non_integer_amount_as_client_error(self) -> None:
        # Wire boundary: a malformed amount string must surface as ClientError,
        # not bare ValueError. A bare exception escapes the typed hierarchy and
        # bypasses callers' `except ClientError` blocks.
        client = self._client()
        for bad in ("", "NaN", "1.5", "0x10"):
            with pytest.raises(ClientError, match="non-integer payment amount"):
                client._validate_payment(_accept(amount=bad))

    def test_rejects_negative_amount(self) -> None:
        # `int("-1")` succeeds and would otherwise sneak past the cap (cap > 0)
        # straight into solders.to_bytes(8, "little") as an OverflowError.
        client = self._client()
        with pytest.raises(ClientError, match="negative payment amount"):
            client._validate_payment(_accept(amount="-1"))


class TestQueryBalanceRpcDiscrimination:
    """`_query_balance` must distinguish ATA-not-found from real RPC errors.

    Returning 0.0 on every kind of failure silently trips the balance guard
    into the free-fallback path on transient infrastructure issues (rate
    limit, node down, malformed response). Only the explicit "account does
    not exist" signal — an error message matching the canonical Solana phrase
    ``"could not find account"`` (or ``"account not found"``) or a 200 with
    ``result.value == null`` — should map to 0. The broader ``"not found"``
    substring also matches transient failures like ``"Method not found"`` and
    ``"Block not found"`` and must NOT be treated as zero balance.
    """

    _ADDRESS = "Sender1111111111111111111111111111111111111"
    _RPC_URL = "https://rpc.test.local"

    def _client(self) -> SolvelaClient:
        return SolvelaClient(config=ClientConfig(rpc_url=self._RPC_URL))

    @pytest.mark.asyncio
    async def test_zero_balance_for_existing_ata(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        httpx_mock.add_response(
            url=self._RPC_URL,
            json={
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "context": {"slot": 1},
                    "value": {
                        "amount": "0",
                        "decimals": 6,
                        "uiAmount": 0.0,
                        "uiAmountString": "0",
                    },
                },
            },
        )
        assert await self._client()._query_balance(self._ADDRESS) == 0.0

    @pytest.mark.asyncio
    async def test_positive_balance(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        httpx_mock.add_response(
            url=self._RPC_URL,
            json={
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "context": {"slot": 1},
                    "value": {
                        "amount": "5000000",
                        "decimals": 6,
                        "uiAmount": 5.0,
                        "uiAmountString": "5",
                    },
                },
            },
        )
        assert await self._client()._query_balance(self._ADDRESS) == 5.0

    @pytest.mark.asyncio
    async def test_ata_not_found_error_returns_zero(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        httpx_mock.add_response(
            url=self._RPC_URL,
            json={
                "jsonrpc": "2.0",
                "id": 1,
                "error": {
                    "code": -32602,
                    "message": "Invalid param: could not find account",
                },
            },
        )
        assert await self._client()._query_balance(self._ADDRESS) == 0.0

    @pytest.mark.asyncio
    async def test_ata_null_value_returns_zero(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        # Some providers signal absence with `result.value: null` on 200.
        httpx_mock.add_response(
            url=self._RPC_URL,
            json={
                "jsonrpc": "2.0",
                "id": 1,
                "result": {"context": {"slot": 1}, "value": None},
            },
        )
        assert await self._client()._query_balance(self._ADDRESS) == 0.0

    @pytest.mark.asyncio
    async def test_rate_limit_raises_not_zero(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        # 429 must not be silently coerced to "zero balance" — that would
        # transparently switch the caller to the free-fallback model on a
        # transient infra issue.
        httpx_mock.add_response(url=self._RPC_URL, status_code=429, json={})
        with pytest.raises(ClientError, match="HTTP 429"):
            await self._client()._query_balance(self._ADDRESS)

    @pytest.mark.asyncio
    async def test_generic_rpc_error_raises(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        httpx_mock.add_response(
            url=self._RPC_URL,
            json={
                "jsonrpc": "2.0",
                "id": 1,
                "error": {"code": -32000, "message": "Internal error"},
            },
        )
        with pytest.raises(ClientError, match="Internal error"):
            await self._client()._query_balance(self._ADDRESS)

    @pytest.mark.asyncio
    async def test_malformed_response_shape_raises(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        httpx_mock.add_response(
            url=self._RPC_URL,
            json={"jsonrpc": "2.0", "id": 1, "result": "not-an-object"},
        )
        with pytest.raises(ClientError, match="unexpected response shape"):
            await self._client()._query_balance(self._ADDRESS)

    @pytest.mark.asyncio
    async def test_method_not_found_does_not_silently_zero(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        # Misconfigured RPC endpoint that doesn't expose getTokenAccountBalance
        # returns "Method not found" — must NOT match the ATA-absent path.
        httpx_mock.add_response(
            url=self._RPC_URL,
            json={
                "jsonrpc": "2.0",
                "id": 1,
                "error": {"code": -32601, "message": "Method not found"},
            },
        )
        with pytest.raises(ClientError, match="Method not found"):
            await self._client()._query_balance(self._ADDRESS)

    @pytest.mark.asyncio
    async def test_block_not_found_does_not_silently_zero(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        # Transient node sync error — same risk: "not found" substring would
        # have falsely matched and silently routed to free-fallback.
        httpx_mock.add_response(
            url=self._RPC_URL,
            json={
                "jsonrpc": "2.0",
                "id": 1,
                "error": {"code": -32004, "message": "Block not found"},
            },
        )
        with pytest.raises(ClientError, match="Block not found"):
            await self._client()._query_balance(self._ADDRESS)

    @pytest.mark.asyncio
    async def test_scalar_error_value_raises(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        # Some non-canonical RPC implementations encode `error` as a string
        # rather than a dict. The discriminator must still raise rather than
        # let it fall through into the result-extraction branch.
        httpx_mock.add_response(
            url=self._RPC_URL,
            json={"jsonrpc": "2.0", "id": 1, "error": "rate limited"},
        )
        with pytest.raises(ClientError, match="rate limited"):
            await self._client()._query_balance(self._ADDRESS)

    @pytest.mark.asyncio
    async def test_missing_result_key_raises(self, httpx_mock) -> None:  # type: ignore[no-untyped-def]
        # 200 with neither `error` nor `result` — pure KeyError path, distinct
        # from the `result: "not-an-object"` TypeError case.
        httpx_mock.add_response(
            url=self._RPC_URL,
            json={"jsonrpc": "2.0", "id": 1},
        )
        with pytest.raises(ClientError, match="unexpected response shape"):
            await self._client()._query_balance(self._ADDRESS)


class TestFindCompatibleScheme:
    """Scheme preference must honor ``ClientConfig.prefer_escrow``."""

    def _payment_required(self, schemes: list[str]) -> PaymentRequired:
        return PaymentRequired(
            x402_version=2,
            resource=Resource(url="https://gw.test/v1/chat/completions", method="POST"),
            accepts=[_accept() if s == "exact" else _accept_escrow() for s in schemes],
            cost_breakdown=CostBreakdown(
                provider_cost="950",
                platform_fee="50",
                total="1000",
                currency="USDC",
                fee_percent=5,
            ),
            error="Payment required",
        )

    def test_default_prefers_exact_when_both_offered(self) -> None:
        client = SolvelaClient(config=ClientConfig())
        accept = client._find_compatible_scheme(self._payment_required(["escrow", "exact"]))
        assert accept.scheme == "exact"

    def test_prefer_escrow_selects_escrow_when_both_offered(self) -> None:
        client = SolvelaClient(config=ClientConfig(prefer_escrow=True))
        accept = client._find_compatible_scheme(self._payment_required(["exact", "escrow"]))
        assert accept.scheme == "escrow"

    def test_prefer_escrow_falls_back_to_exact_when_only_exact_offered(self) -> None:
        client = SolvelaClient(config=ClientConfig(prefer_escrow=True))
        accept = client._find_compatible_scheme(self._payment_required(["exact"]))
        assert accept.scheme == "exact"

    def test_default_falls_back_to_escrow_when_only_escrow_offered(self) -> None:
        # Symmetric fallback: with the default prefer_escrow=False, an
        # escrow-only gateway must still be usable. Without this, the second
        # loop would only ever fire under prefer_escrow=True.
        client = SolvelaClient(config=ClientConfig())
        accept = client._find_compatible_scheme(self._payment_required(["escrow"]))
        assert accept.scheme == "escrow"

    def test_unhonored_preference_emits_warning(self, caplog) -> None:  # type: ignore[no-untyped-def]
        # Silent fallback hides config bugs. A WARNING is the cheapest signal
        # to operators that the runtime preference wasn't honored.
        import logging

        client = SolvelaClient(config=ClientConfig(prefer_escrow=True))
        with caplog.at_level(logging.WARNING, logger="solvela.client"):
            client._find_compatible_scheme(self._payment_required(["exact"]))
        assert any(
            "prefer_escrow" in r.getMessage() and "falling back" in r.getMessage()
            for r in caplog.records
        )

    def test_no_compatible_scheme_raises(self) -> None:
        client = SolvelaClient(config=ClientConfig())
        pr = PaymentRequired(
            x402_version=2,
            resource=Resource(url="https://gw.test/v1/chat/completions", method="POST"),
            accepts=[],
            cost_breakdown=CostBreakdown(
                provider_cost="0",
                platform_fee="0",
                total="0",
                currency="USDC",
                fee_percent=0,
            ),
            error="Payment required",
        )
        with pytest.raises(ClientError, match="No compatible payment scheme"):
            client._find_compatible_scheme(pr)


def _accept_escrow() -> PaymentAccept:
    return PaymentAccept(
        scheme="escrow",
        network=SOLANA_NETWORK,
        amount="1000",
        asset=USDC_MINT,
        pay_to="RecipientPubkey111111111111111111111111111",
        max_timeout_seconds=300,
        escrow_program_id="EscrowProg11111111111111111111111111111111",
    )
