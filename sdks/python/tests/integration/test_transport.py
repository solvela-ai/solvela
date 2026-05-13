"""Integration tests for Transport — HTTP interactions via pytest-httpx."""

from __future__ import annotations

import json

import pytest

from solvela.errors import GatewayError
from solvela.transport import Transport
from solvela.types import (
    ChatMessage,
    ChatRequest,
    ChatResponse,
    PaymentRequired,
    Role,
)

# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

GATEWAY_URL = "https://gw.test.local"


def _chat_request() -> ChatRequest:
    return ChatRequest(
        model="gpt-4o",
        messages=[ChatMessage(role=Role.USER, content="Hello")],
    )


def _chat_response_json() -> dict:
    return {
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "created": 1700000000,
        "model": "gpt-4o",
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": "Hi there"},
                "finish_reason": "stop",
            }
        ],
        "usage": {
            "prompt_tokens": 5,
            "completion_tokens": 3,
            "total_tokens": 8,
        },
    }


def _payment_required_json() -> dict:
    return {
        "x402_version": 2,
        "resource": {"url": "https://gw.test.local/v1/chat/completions", "method": "POST"},
        "accepts": [
            {
                "scheme": "exact",
                "network": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
                "amount": "1000",
                "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                "pay_to": "RecipientPubkey111111111111111111111111111",
                "max_timeout_seconds": 300,
            }
        ],
        "cost_breakdown": {
            "provider_cost": "950",
            "platform_fee": "50",
            "total": "1000",
            "currency": "USDC",
            "fee_percent": 5,
        },
        "error": "Payment required",
    }


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_send_chat_success(httpx_mock) -> None:
    httpx_mock.add_response(
        url=f"{GATEWAY_URL}/v1/chat/completions",
        json=_chat_response_json(),
        status_code=200,
    )
    transport = Transport(GATEWAY_URL)
    result = await transport.send_chat(_chat_request())
    assert isinstance(result, ChatResponse)
    assert result.id == "chatcmpl-1"
    assert result.choices[0].message.content == "Hi there"


@pytest.mark.asyncio
async def test_send_chat_402(httpx_mock) -> None:
    httpx_mock.add_response(
        url=f"{GATEWAY_URL}/v1/chat/completions",
        json=_payment_required_json(),
        status_code=402,
    )
    transport = Transport(GATEWAY_URL)
    result = await transport.send_chat(_chat_request())
    assert isinstance(result, PaymentRequired)
    assert result.x402_version == 2
    assert result.cost_breakdown.total == "1000"


@pytest.mark.asyncio
async def test_send_chat_500(httpx_mock) -> None:
    httpx_mock.add_response(
        url=f"{GATEWAY_URL}/v1/chat/completions",
        json={"error": "Internal server error"},
        status_code=500,
    )
    transport = Transport(GATEWAY_URL)
    with pytest.raises(GatewayError) as exc_info:
        await transport.send_chat(_chat_request())
    assert exc_info.value.status == 500


@pytest.mark.asyncio
async def test_send_chat_sends_stream_false(httpx_mock) -> None:
    """Verify non-streaming request sets stream=false in the body."""
    httpx_mock.add_response(
        url=f"{GATEWAY_URL}/v1/chat/completions",
        json=_chat_response_json(),
        status_code=200,
    )
    transport = Transport(GATEWAY_URL)
    await transport.send_chat(_chat_request())

    request = httpx_mock.get_request()
    body = json.loads(request.content)
    assert body["stream"] is False


@pytest.mark.asyncio
async def test_fetch_models(httpx_mock) -> None:
    httpx_mock.add_response(
        url=f"{GATEWAY_URL}/v1/models",
        json={"data": [{"id": "gpt-4o"}, {"id": "claude-sonnet"}]},
        status_code=200,
    )
    transport = Transport(GATEWAY_URL)
    models = await transport.fetch_models()
    assert len(models) == 2
    assert models[0]["id"] == "gpt-4o"


# ---------------------------------------------------------------------------
# 402 envelope unwrap — the real gateway returns
# ``{"error": {"message": "<stringified PaymentRequired>", "type": "invalid_payment"}}``
# rather than the bare PaymentRequired body. Without the unwrap, the SDK
# could not parse a real 402 at all (KeyError on missing top-level keys).


def _wrapped_402_envelope() -> dict:
    return {
        "error": {
            "message": json.dumps(_payment_required_json()),
            "type": "invalid_payment",
        }
    }


@pytest.mark.asyncio
async def test_send_chat_402_unwraps_gateway_envelope(httpx_mock) -> None:
    """The 402 envelope must be unwrapped before PaymentRequired.from_dict."""
    httpx_mock.add_response(
        url=f"{GATEWAY_URL}/v1/chat/completions",
        json=_wrapped_402_envelope(),
        status_code=402,
    )
    transport = Transport(GATEWAY_URL)
    result = await transport.send_chat(_chat_request())
    assert isinstance(result, PaymentRequired)
    assert result.x402_version == 2
    assert result.cost_breakdown.total == "1000"
    assert result.accepts[0].scheme == "exact"


@pytest.mark.asyncio
async def test_send_chat_stream_402_unwraps_gateway_envelope(httpx_mock) -> None:
    """Streaming path must apply the same envelope unwrap as send_chat."""
    from solvela.errors import PaymentRequiredError

    httpx_mock.add_response(
        url=f"{GATEWAY_URL}/v1/chat/completions",
        json=_wrapped_402_envelope(),
        status_code=402,
    )
    transport = Transport(GATEWAY_URL)
    with pytest.raises(PaymentRequiredError) as exc_info:
        async for _ in transport.send_chat_stream(_chat_request()):
            pass
    assert exc_info.value.payment_required.cost_breakdown.total == "1000"
