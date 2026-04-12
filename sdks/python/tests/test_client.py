"""Tests for LLMClient and AsyncLLMClient (all network calls mocked)."""

import base64
import json
from unittest.mock import MagicMock, patch

import httpx
import pytest

from solvela.client import (
    AsyncLLMClient,
    BudgetExceededError,
    LLMClient,
    PaymentError,
)
from solvela.config import DEFAULT_API_URL
from solvela.types import (
    ChatMessage,
    CostBreakdown,
    PaymentAccept,
    PaymentRequired,
    Role,
)
from solvela.x402 import decode_payment_header, encode_payment_header


# ---------------------------------------------------------------------------
# Fixtures & helpers
# ---------------------------------------------------------------------------

MOCK_CHAT_RESPONSE = {
    "id": "chatcmpl-test123",
    "object": "chat.completion",
    "created": 1700000000,
    "model": "gpt-4o",
    "choices": [
        {
            "index": 0,
            "message": {"role": "assistant", "content": "Hello there!"},
            "finish_reason": "stop",
        }
    ],
    "usage": {
        "prompt_tokens": 10,
        "completion_tokens": 5,
        "total_tokens": 15,
    },
}

MOCK_PAYMENT_REQUIRED = {
    "x402_version": 2,
    "accepts": [
        {
            "scheme": "exact",
            "network": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
            "amount": "2625",
            "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            "pay_to": "RecipientPubkey",
            "max_timeout_seconds": 300,
        }
    ],
    "cost_breakdown": {
        "provider_cost": "0.002500",
        "platform_fee": "0.000125",
        "total": "0.002625",
        "currency": "USDC",
        "fee_percent": 5,
    },
    "error": "Payment required",
}


def make_402_body() -> dict:
    """Build the 402 response body the gateway would return."""
    return {"error": {"message": json.dumps(MOCK_PAYMENT_REQUIRED)}}


def mock_response(status_code: int, json_data: dict) -> httpx.Response:
    """Create a mock httpx.Response."""
    return httpx.Response(
        status_code=status_code,
        json=json_data,
        request=httpx.Request("POST", "http://test"),
    )


# ---------------------------------------------------------------------------
# LLMClient initialisation
# ---------------------------------------------------------------------------


class TestLLMClientInit:
    def test_default_api_url(self):
        client = LLMClient()
        assert client.api_url == DEFAULT_API_URL
        client.close()

    def test_custom_api_url(self):
        client = LLMClient(api_url="http://localhost:8402")
        assert client.api_url == "http://localhost:8402"
        client.close()

    def test_trailing_slash_stripped(self):
        client = LLMClient(api_url="http://localhost:8402/")
        assert client.api_url == "http://localhost:8402"
        client.close()

    def test_session_budget(self):
        client = LLMClient(session_budget=1.0)
        assert client.session_budget == 1.0
        assert client.session_spent == 0.0
        client.close()

    def test_context_manager(self):
        with LLMClient() as client:
            assert client.api_url == DEFAULT_API_URL


# ---------------------------------------------------------------------------
# URL construction
# ---------------------------------------------------------------------------


class TestURLConstruction:
    def test_chat_completions_url(self):
        client = LLMClient(api_url="http://localhost:8402")
        url = f"{client.api_url}/v1/chat/completions"
        assert url == "http://localhost:8402/v1/chat/completions"
        client.close()

    def test_models_url(self):
        client = LLMClient(api_url="https://api.solvela.ai")
        url = f"{client.api_url}/v1/models"
        assert url == "https://api.solvela.ai/v1/models"
        client.close()

    def test_health_url(self):
        client = LLMClient(api_url="http://example.com/")
        url = f"{client.api_url}/health"
        assert url == "http://example.com/health"
        client.close()


# ---------------------------------------------------------------------------
# Payment header creation
# ---------------------------------------------------------------------------


class TestPaymentHeader:
    def test_encode_decode_roundtrip(self):
        accept = PaymentAccept(
            scheme="exact",
            network="solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
            amount="2625",
            asset="EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            pay_to="RecipientPubkey",
            max_timeout_seconds=300,
        )
        encoded = encode_payment_header(accept, "http://test/v1/chat/completions")
        decoded = decode_payment_header(encoded)

        assert decoded["x402_version"] == 2
        assert decoded["resource"]["url"] == "http://test/v1/chat/completions"
        assert decoded["resource"]["method"] == "POST"
        assert decoded["accepted"]["scheme"] == "exact"
        assert decoded["payload"]["transaction"] == "STUB_BASE64_TX"

    def test_header_is_valid_base64(self):
        accept = PaymentAccept(
            scheme="exact",
            network="solana:test",
            amount="100",
            asset="USDC",
            pay_to="wallet",
            max_timeout_seconds=60,
        )
        encoded = encode_payment_header(accept, "http://test")
        # Should not raise
        raw = base64.b64decode(encoded)
        data = json.loads(raw)
        assert "x402_version" in data


# ---------------------------------------------------------------------------
# 402 payment flow (mocked)
# ---------------------------------------------------------------------------


class TestPaymentFlow:
    def test_successful_chat_no_payment(self):
        """Direct 200 response — no payment needed."""
        client = LLMClient(api_url="http://test")

        with patch.object(
            client._client,
            "post",
            return_value=mock_response(200, MOCK_CHAT_RESPONSE),
        ):
            result = client.chat("gpt-4o", "Hello")
            assert result == "Hello there!"

        client.close()

    @patch("solvela.x402.build_solana_transfer_checked", return_value="MOCK_SIGNED_TX")
    def test_402_then_200_payment_flow(self, _mock_sign):
        """402 → payment → retry → 200."""
        client = LLMClient(api_url="http://test", private_key="TestKey")

        responses = [
            mock_response(402, make_402_body()),
            mock_response(200, MOCK_CHAT_RESPONSE),
        ]
        call_count = 0

        def mock_post(*args, **kwargs):
            nonlocal call_count
            resp = responses[call_count]
            call_count += 1
            return resp

        with patch.object(client._client, "post", side_effect=mock_post):
            result = client.chat("gpt-4o", "Hello")
            assert result == "Hello there!"
            assert call_count == 2

        # Session spent should be updated
        assert client.session_spent == pytest.approx(0.002625)
        client.close()

    @patch("solvela.x402.build_solana_transfer_checked", return_value="MOCK_SIGNED_TX")
    def test_402_with_payment_header_sent(self, _mock_sign):
        """Verify the retry includes the payment-signature header."""
        client = LLMClient(api_url="http://test", private_key="TestKey")

        call_args_list = []

        def mock_post(*args, **kwargs):
            call_args_list.append(kwargs)
            if len(call_args_list) == 1:
                return mock_response(402, make_402_body())
            return mock_response(200, MOCK_CHAT_RESPONSE)

        with patch.object(client._client, "post", side_effect=mock_post):
            client.chat("gpt-4o", "Hello")

        assert len(call_args_list) == 2
        # First call: no payment header
        assert (
            "headers" not in call_args_list[0]
            or call_args_list[0].get("headers") is None
        )
        # Second call: has payment-signature header
        assert "payment-signature" in call_args_list[1].get("headers", {})

        client.close()

    def test_402_unparseable_raises_payment_error(self):
        """If 402 body can't be parsed, raise PaymentError."""
        client = LLMClient(api_url="http://test")

        bad_402 = mock_response(402, {"error": {"message": "not json"}})
        with patch.object(client._client, "post", return_value=bad_402):
            with pytest.raises(PaymentError, match="Failed to parse"):
                client.chat("gpt-4o", "Hello")

        client.close()


# ---------------------------------------------------------------------------
# Session budget
# ---------------------------------------------------------------------------


class TestSessionBudget:
    def test_budget_exceeded_raises(self):
        """If the cost would exceed the budget, raise BudgetExceededError."""
        client = LLMClient(
            api_url="http://test",
            session_budget=0.001,  # Very small budget
        )

        with patch.object(
            client._client,
            "post",
            return_value=mock_response(402, make_402_body()),
        ):
            with pytest.raises(BudgetExceededError, match="budget"):
                client.chat("gpt-4o", "Hello")

        client.close()

    def test_budget_accumulates(self):
        """Multiple requests accumulate session spend."""
        client = LLMClient(api_url="http://test", session_budget=1.0)

        def mock_post(*args, **kwargs):
            if "headers" not in kwargs or kwargs.get("headers") is None:
                return mock_response(402, make_402_body())
            return mock_response(200, MOCK_CHAT_RESPONSE)

        with patch.object(client._client, "post", side_effect=mock_post):
            client.chat("gpt-4o", "Hello")
            client.chat("gpt-4o", "World")

        assert client.session_spent == pytest.approx(0.002625 * 2)
        client.close()

    def test_no_budget_no_limit(self):
        """Without a budget set, no limit is enforced."""
        client = LLMClient(api_url="http://test")
        assert client.session_budget is None

        def mock_post(*args, **kwargs):
            if "headers" not in kwargs or kwargs.get("headers") is None:
                return mock_response(402, make_402_body())
            return mock_response(200, MOCK_CHAT_RESPONSE)

        with patch.object(client._client, "post", side_effect=mock_post):
            client.chat("gpt-4o", "Hello")

        assert client.session_spent == pytest.approx(0.002625)
        client.close()


# ---------------------------------------------------------------------------
# list_models, health
# ---------------------------------------------------------------------------


class TestListModelsAndHealth:
    def test_list_models(self):
        client = LLMClient(api_url="http://test")
        mock_models = {"data": [{"id": "gpt-4o"}, {"id": "claude-sonnet-4"}]}

        with patch.object(
            client._client,
            "get",
            return_value=mock_response(200, mock_models),
        ):
            result = client.list_models()
            assert len(result["data"]) == 2

        client.close()

    def test_health(self):
        client = LLMClient(api_url="http://test")
        mock_health = {"status": "ok", "solana_rpc": "connected"}

        with patch.object(
            client._client,
            "get",
            return_value=mock_response(200, mock_health),
        ):
            result = client.health()
            assert result["status"] == "ok"

        client.close()


# ---------------------------------------------------------------------------
# AsyncLLMClient
# ---------------------------------------------------------------------------


class TestAsyncLLMClientInit:
    def test_default_api_url(self):
        client = AsyncLLMClient()
        assert client.api_url == DEFAULT_API_URL

    def test_custom_api_url(self):
        client = AsyncLLMClient(api_url="http://localhost:8402/")
        assert client.api_url == "http://localhost:8402"

    def test_session_budget(self):
        client = AsyncLLMClient(session_budget=5.0)
        assert client.session_budget == 5.0
        assert client.session_spent == 0.0


@pytest.mark.asyncio
class TestAsyncPaymentFlow:
    async def test_successful_chat_no_payment(self):
        client = AsyncLLMClient(api_url="http://test")

        with patch.object(
            client._client,
            "post",
            return_value=mock_response(200, MOCK_CHAT_RESPONSE),
        ):
            result = await client.chat("gpt-4o", "Hello")
            assert result == "Hello there!"

        await client.close()

    @patch("solvela.x402.build_solana_transfer_checked", return_value="MOCK_SIGNED_TX")
    async def test_402_then_200(self, _mock_sign):
        client = AsyncLLMClient(api_url="http://test", private_key="TestKey")

        responses = [
            mock_response(402, make_402_body()),
            mock_response(200, MOCK_CHAT_RESPONSE),
        ]
        call_count = 0

        async def mock_post(*args, **kwargs):
            nonlocal call_count
            resp = responses[call_count]
            call_count += 1
            return resp

        with patch.object(client._client, "post", side_effect=mock_post):
            result = await client.chat("gpt-4o", "Hello")
            assert result == "Hello there!"

        assert client.session_spent == pytest.approx(0.002625)
        await client.close()

    async def test_budget_exceeded(self):
        client = AsyncLLMClient(api_url="http://test", session_budget=0.001)

        async def mock_post(*args, **kwargs):
            return mock_response(402, make_402_body())

        with patch.object(client._client, "post", side_effect=mock_post):
            with pytest.raises(BudgetExceededError):
                await client.chat("gpt-4o", "Hello")

        await client.close()
