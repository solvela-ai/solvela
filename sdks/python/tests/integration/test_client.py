"""Integration tests for SolvelaClient — HTTP mocking via pytest-httpx."""

from __future__ import annotations

import base64
import json

import pytest

from solvela.client import SolvelaClient
from solvela.config import ClientConfig
from solvela.errors import ClientError, PaymentRejectedError, PaymentRequiredError
from solvela.signer import Signer
from solvela.types import (
    AtomicUsdc,
    ChatMessage,
    ChatRequest,
    ChatResponse,
    PaymentAccept,
    PaymentPayload,
    Resource,
    Role,
    SolanaPayload,
)

GATEWAY_URL = "https://gw.test.local"


def _chat_request(model: str = "gpt-4o") -> ChatRequest:
    return ChatRequest(
        model=model,
        messages=[ChatMessage(role=Role.USER, content="Hello")],
    )


def _chat_response_json(model: str = "gpt-4o") -> dict:
    return {
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "created": 1700000000,
        "model": model,
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": "Hi there!"},
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
        "resource": {"url": f"{GATEWAY_URL}/v1/chat/completions", "method": "POST"},
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


@pytest.mark.asyncio
async def test_chat_success(httpx_mock) -> None:
    """Mock 200 response, verify returns ChatResponse."""
    httpx_mock.add_response(
        url=f"{GATEWAY_URL}/v1/chat/completions",
        json=_chat_response_json(),
        status_code=200,
    )
    config = ClientConfig(gateway_url=GATEWAY_URL)
    client = SolvelaClient(config=config)
    response = await client.chat(_chat_request())
    assert isinstance(response, ChatResponse)
    assert response.id == "chatcmpl-1"
    assert response.choices[0].message.content == "Hi there!"


@pytest.mark.asyncio
async def test_chat_with_cache(httpx_mock) -> None:
    """Enable cache, send same request twice, verify only 1 HTTP call."""
    httpx_mock.add_response(
        url=f"{GATEWAY_URL}/v1/chat/completions",
        json=_chat_response_json(),
        status_code=200,
    )
    config = ClientConfig(gateway_url=GATEWAY_URL, enable_cache=True)
    client = SolvelaClient(config=config)

    req = _chat_request()
    resp1 = await client.chat(req)
    resp2 = await client.chat(req)

    assert resp1.id == resp2.id
    # Only one HTTP request should have been made
    assert len(httpx_mock.get_requests()) == 1


@pytest.mark.asyncio
async def test_chat_quality_retry(httpx_mock) -> None:
    """First response degraded (empty content), second good, verify retries."""
    degraded_response = {
        "id": "chatcmpl-degraded",
        "object": "chat.completion",
        "created": 1700000000,
        "model": "gpt-4o",
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": ""},
                "finish_reason": "stop",
            }
        ],
    }
    good_response = _chat_response_json()

    httpx_mock.add_response(
        url=f"{GATEWAY_URL}/v1/chat/completions",
        json=degraded_response,
        status_code=200,
    )
    httpx_mock.add_response(
        url=f"{GATEWAY_URL}/v1/chat/completions",
        json=good_response,
        status_code=200,
    )

    config = ClientConfig(
        gateway_url=GATEWAY_URL,
        enable_quality_check=True,
        max_quality_retries=1,
    )
    client = SolvelaClient(config=config)
    response = await client.chat(_chat_request())

    assert response.choices[0].message.content == "Hi there!"
    assert len(httpx_mock.get_requests()) == 2


@pytest.mark.asyncio
async def test_chat_402_without_signer(httpx_mock) -> None:
    """Mock 402, verify raises PaymentRequiredError when no signer configured."""
    httpx_mock.add_response(
        url=f"{GATEWAY_URL}/v1/chat/completions",
        json=_payment_required_json(),
        status_code=402,
    )
    config = ClientConfig(gateway_url=GATEWAY_URL)
    client = SolvelaClient(config=config)

    with pytest.raises(PaymentRequiredError):
        await client.chat(_chat_request())


class _StubSigner(Signer):
    """Test-only signer that returns a canned PaymentPayload without RPC calls."""

    async def sign_payment(
        self,
        amount_atomic: AtomicUsdc,
        recipient: str,
        resource: Resource,
        accepted: PaymentAccept,
    ) -> PaymentPayload:
        return PaymentPayload(
            x402_version=2,
            resource=resource,
            accepted=accepted,
            payload=SolanaPayload(transaction="dGVzdC10cmFuc2FjdGlvbg=="),
        )


@pytest.mark.asyncio
async def test_chat_402_signed_then_200_success(httpx_mock) -> None:
    """End-to-end 402 → sign → 200 happy path with stub signer.

    The audit highlighted that the success branch of `_send_with_payment`
    had no integration coverage. A regression in base64 encoding, header
    name, or PaymentPayload.to_dict() field ordering would pass every
    other test. This exercises the full handshake and asserts the second
    request carries a Payment-Signature header derived from the stub.
    """
    httpx_mock.add_response(
        url=f"{GATEWAY_URL}/v1/chat/completions",
        json=_payment_required_json(),
        status_code=402,
    )
    httpx_mock.add_response(
        url=f"{GATEWAY_URL}/v1/chat/completions",
        json=_chat_response_json(),
        status_code=200,
    )
    config = ClientConfig(gateway_url=GATEWAY_URL, max_payment_amount=None)
    client = SolvelaClient(config=config, signer=_StubSigner())

    response = await client.chat(_chat_request())

    assert isinstance(response, ChatResponse)
    assert response.id == "chatcmpl-1"

    # The second request must carry a Payment-Signature header that base64-
    # decodes back into the canned PaymentPayload — this catches header-name
    # typos, missing payload fields, and wire-encoding regressions.
    requests = httpx_mock.get_requests()
    assert len(requests) == 2
    assert "Payment-Signature" not in requests[0].headers
    sig_b64 = requests[1].headers.get("Payment-Signature")
    assert sig_b64 is not None
    decoded = json.loads(base64.b64decode(sig_b64).decode())
    assert decoded["x402_version"] == 2
    assert decoded["payload"]["transaction"] == "dGVzdC10cmFuc2FjdGlvbg=="


@pytest.mark.asyncio
async def test_chat_402_after_signing_raises_payment_rejected(httpx_mock) -> None:
    """402 → sign → 402 must raise PaymentRejectedError, not bare ClientError."""
    httpx_mock.add_response(
        url=f"{GATEWAY_URL}/v1/chat/completions",
        json=_payment_required_json(),
        status_code=402,
    )
    httpx_mock.add_response(
        url=f"{GATEWAY_URL}/v1/chat/completions",
        json=_payment_required_json(),
        status_code=402,
    )
    config = ClientConfig(gateway_url=GATEWAY_URL, max_payment_amount=None)
    client = SolvelaClient(config=config, signer=_StubSigner())

    with pytest.raises(PaymentRejectedError) as exc_info:
        await client.chat(_chat_request())

    assert len(httpx_mock.get_requests()) == 2
    # Inheritance regression guard: callers using `except ClientError` must
    # still catch this. PaymentRejectedError → ClientError → Exception.
    assert isinstance(exc_info.value, ClientError)
    # Body capture: the second-402 PaymentRequired struct must be reachable
    # from the exception so callers can inspect *why* the gateway rejected.
    assert exc_info.value.payment_required is not None
    assert exc_info.value.payment_required.cost_breakdown.total == "1000"


@pytest.mark.asyncio
async def test_chat_stream_402_after_signing_raises_payment_rejected(
    httpx_mock,
) -> None:
    """Streaming-path symmetry: post-signing 402 surfaces as PaymentRejectedError.

    Without the wrap in chat_stream, send_chat_stream would raise the bare
    PaymentRequiredError it uses for any 402 — callers cannot distinguish
    "needs signing" from "signed and rejected" on the streaming path.
    """
    # First call: preflight probe → 402 (gateway demands payment).
    httpx_mock.add_response(
        url=f"{GATEWAY_URL}/v1/chat/completions",
        json=_payment_required_json(),
        status_code=402,
    )
    # Second call: streaming POST with the signed header → still 402.
    httpx_mock.add_response(
        url=f"{GATEWAY_URL}/v1/chat/completions",
        json=_payment_required_json(),
        status_code=402,
    )
    config = ClientConfig(gateway_url=GATEWAY_URL, max_payment_amount=None)
    client = SolvelaClient(config=config, signer=_StubSigner())

    with pytest.raises(PaymentRejectedError) as exc_info:
        async for _ in client.chat_stream(_chat_request()):
            pass

    assert isinstance(exc_info.value, ClientError)
    assert exc_info.value.payment_required is not None


@pytest.mark.asyncio
async def test_chat_stream_preflight_402_then_sse(httpx_mock) -> None:
    """chat_stream preflight handshake: 402 probe → sign → SSE stream.

    Exercises the streaming code path's preflight: a non-streaming probe
    returns 402, the stub signer produces a payload, and the subsequent
    streaming request carries the resulting Payment-Signature header. The
    SSE body emits two chunks plus the [DONE] terminator.
    """
    # Probe: non-streaming POST returning 402.
    httpx_mock.add_response(
        url=f"{GATEWAY_URL}/v1/chat/completions",
        json=_payment_required_json(),
        status_code=402,
    )
    # Streaming POST: SSE body. pytest-httpx routes by URL; the ordering of
    # add_response calls matches request ordering.
    sse_body = (
        b'data: {"id":"c1","object":"chat.completion.chunk","created":1,'
        b'"model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant",'
        b'"content":"Hi"},"finish_reason":null}]}\n\n'
        b'data: {"id":"c1","object":"chat.completion.chunk","created":1,'
        b'"model":"gpt-4o","choices":[{"index":0,"delta":{"content":" there"},'
        b'"finish_reason":"stop"}]}\n\n'
        b"data: [DONE]\n\n"
    )
    httpx_mock.add_response(
        url=f"{GATEWAY_URL}/v1/chat/completions",
        status_code=200,
        content=sse_body,
        headers={"content-type": "text/event-stream"},
    )

    config = ClientConfig(gateway_url=GATEWAY_URL, max_payment_amount=None)
    client = SolvelaClient(config=config, signer=_StubSigner())

    chunks = [c async for c in client.chat_stream(_chat_request())]

    assert len(chunks) == 2
    assert chunks[0].choices[0].delta.content == "Hi"
    assert chunks[1].choices[0].finish_reason == "stop"

    requests = httpx_mock.get_requests()
    assert len(requests) == 2
    # The probe is non-streaming and unsigned; the streaming request carries
    # the signature derived from the probe's 402.
    probe_body = json.loads(requests[0].content)
    assert probe_body["stream"] is False
    assert "Payment-Signature" not in requests[0].headers
    stream_body = json.loads(requests[1].content)
    assert stream_body["stream"] is True
    assert "Payment-Signature" in requests[1].headers


@pytest.mark.asyncio
async def test_chat_balance_guard_fallback(httpx_mock) -> None:
    """Set _last_balance=0, free_fallback_model set, verify model swapped."""
    httpx_mock.add_response(
        url=f"{GATEWAY_URL}/v1/chat/completions",
        json=_chat_response_json(model="free-model"),
        status_code=200,
    )
    config = ClientConfig(
        gateway_url=GATEWAY_URL,
        free_fallback_model="free-model",
    )
    client = SolvelaClient(config=config)
    client._last_balance = 0.0

    await client.chat(_chat_request(model="gpt-4o"))

    # Verify the request sent to the gateway used the fallback model
    request = httpx_mock.get_requests()[0]
    body = json.loads(request.content)
    assert body["model"] == "free-model"
