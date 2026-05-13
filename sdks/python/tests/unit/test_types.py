"""Tests for solvela.types and solvela.constants."""

from __future__ import annotations

from typing import Any

import pytest

from solvela.constants import (
    MAX_TIMEOUT_SECONDS,
    PLATFORM_FEE_PERCENT,
    SOLANA_NETWORK,
    USDC_MINT,
    X402_VERSION,
)
from solvela.errors import ClientError
from solvela.types import (
    ChatChunk,
    ChatMessage,
    ChatRequest,
    ChatResponse,
    ModelInfo,
    PaymentAccept,
    PaymentPayload,
    PaymentRequired,
    Resource,
    Role,
    SolanaPayload,
)

# --- Constants ---


class TestConstants:
    def test_x402_version(self) -> None:
        assert X402_VERSION == 2

    def test_usdc_mint(self) -> None:
        assert USDC_MINT == "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"

    def test_solana_network(self) -> None:
        assert SOLANA_NETWORK == "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"

    def test_max_timeout(self) -> None:
        assert MAX_TIMEOUT_SECONDS == 300

    def test_platform_fee(self) -> None:
        assert PLATFORM_FEE_PERCENT == 5


# --- Role ---


class TestRole:
    def test_all_values(self) -> None:
        assert Role.SYSTEM == "system"
        assert Role.USER == "user"
        assert Role.ASSISTANT == "assistant"
        assert Role.TOOL == "tool"
        assert Role.DEVELOPER == "developer"

    def test_role_is_str(self) -> None:
        assert isinstance(Role.SYSTEM, str)

    def test_role_count(self) -> None:
        assert len(Role) == 5


# --- ChatMessage ---


class TestChatMessage:
    def test_to_dict_minimal(self) -> None:
        msg = ChatMessage(role=Role.USER, content="hello")
        d = msg.to_dict()
        assert d == {"role": "user", "content": "hello"}

    def test_to_dict_with_optional_fields(self) -> None:
        msg = ChatMessage(role=Role.ASSISTANT, content="hi", name="bot")
        d = msg.to_dict()
        assert d == {"role": "assistant", "content": "hi", "name": "bot"}

    def test_to_dict_omits_none(self) -> None:
        msg = ChatMessage(role=Role.USER, content="test")
        d = msg.to_dict()
        assert "name" not in d
        assert "tool_calls" not in d
        assert "tool_call_id" not in d

    def test_from_dict(self) -> None:
        msg = ChatMessage.from_dict({"role": "user", "content": "hello"})
        assert msg.role == Role.USER
        assert msg.content == "hello"
        assert msg.name is None

    def test_from_dict_with_tool_calls(self) -> None:
        data = {
            "role": "assistant",
            "content": None,
            "tool_calls": [
                {
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "get_weather", "arguments": '{"city":"NYC"}'},
                }
            ],
        }
        msg = ChatMessage.from_dict(data)
        assert msg.tool_calls is not None
        assert len(msg.tool_calls) == 1
        assert msg.tool_calls[0].function.name == "get_weather"


# --- ChatRequest ---


class TestChatRequest:
    def test_to_dict_includes_stream(self) -> None:
        req = ChatRequest(
            model="gpt-4",
            messages=[ChatMessage(role=Role.USER, content="hi")],
        )
        d = req.to_dict()
        assert "stream" in d
        assert d["stream"] is False

    def test_to_dict_stream_true(self) -> None:
        req = ChatRequest(
            model="gpt-4",
            messages=[ChatMessage(role=Role.USER, content="hi")],
            stream=True,
        )
        d = req.to_dict()
        assert d["stream"] is True

    def test_to_dict_omits_none_optional(self) -> None:
        req = ChatRequest(
            model="gpt-4",
            messages=[ChatMessage(role=Role.USER, content="hi")],
        )
        d = req.to_dict()
        assert "max_tokens" not in d
        assert "temperature" not in d
        assert "tools" not in d


# --- ChatResponse ---


class TestChatResponse:
    def test_from_dict_full(self) -> None:
        data = {
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1700000000,
            "model": "gpt-4",
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": "Hello!"},
                    "finish_reason": "stop",
                }
            ],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15,
            },
        }
        resp = ChatResponse.from_dict(data)
        assert resp.id == "chatcmpl-123"
        assert resp.model == "gpt-4"
        assert len(resp.choices) == 1
        assert resp.choices[0].message.content == "Hello!"
        assert resp.choices[0].finish_reason == "stop"
        assert resp.usage is not None
        assert resp.usage.total_tokens == 15

    def test_from_dict_no_usage(self) -> None:
        data = {
            "id": "chatcmpl-456",
            "object": "chat.completion",
            "created": 1700000000,
            "model": "gpt-4",
            "choices": [],
        }
        resp = ChatResponse.from_dict(data)
        assert resp.usage is None


# --- ChatChunk ---


class TestChatChunk:
    def test_from_dict(self) -> None:
        data = {
            "id": "chatcmpl-chunk-1",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "gpt-4",
            "choices": [
                {
                    "index": 0,
                    "delta": {"role": "assistant", "content": "Hi"},
                    "finish_reason": None,
                }
            ],
        }
        chunk = ChatChunk.from_dict(data)
        assert chunk.id == "chatcmpl-chunk-1"
        assert len(chunk.choices) == 1
        assert chunk.choices[0].delta.content == "Hi"
        assert chunk.choices[0].finish_reason is None


# --- PaymentRequired ---


class TestPaymentRequired:
    def test_from_dict(self) -> None:
        data = {
            "x402_version": 2,
            "resource": {"url": "https://gw.example.com/v1/chat/completions", "method": "POST"},
            "accepts": [
                {
                    "scheme": "exact",
                    "network": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
                    "amount": "50000",
                    "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                    "pay_to": "RecipientWallet111111111111111111111111111",
                    "max_timeout_seconds": 300,
                }
            ],
            "cost_breakdown": {
                "provider_cost": "45000",
                "platform_fee": "5000",
                "total": "50000",
                "currency": "USDC",
                "fee_percent": 5,
            },
            "error": "Payment required",
        }
        pr = PaymentRequired.from_dict(data)
        assert pr.x402_version == 2
        assert pr.resource.url == "https://gw.example.com/v1/chat/completions"
        assert len(pr.accepts) == 1
        assert pr.accepts[0].amount == "50000"
        assert pr.cost_breakdown.total == "50000"
        assert pr.error == "Payment required"


class TestPaymentAcceptScheme:
    """`scheme` is typed Literal["exact", "escrow"] — wire validation enforces it."""

    def _accept_data(self, scheme: str) -> dict[str, Any]:
        return {
            "scheme": scheme,
            "network": SOLANA_NETWORK,
            "amount": "1000",
            "asset": USDC_MINT,
            "pay_to": "RecipientPubkey111111111111111111111111111",
            "max_timeout_seconds": 300,
        }

    def test_accepts_known_schemes(self) -> None:
        for scheme in ("exact", "escrow"):
            accept = PaymentAccept.from_dict(self._accept_data(scheme))
            assert accept.scheme == scheme

    def test_rejects_unknown_scheme(self) -> None:
        # An unknown wire scheme means either a malformed gateway response or
        # a protocol upgrade the SDK has not been taught yet — either way we
        # would rather fail loudly than silently mis-branch in scheme matching.
        with pytest.raises(ClientError, match="Unknown payment scheme"):
            PaymentAccept.from_dict(self._accept_data("instant"))

    def test_scheme_alias_exposed_from_package(self) -> None:
        # Consumers writing scheme-aware callbacks need to annotate against
        # the same alias the SDK uses internally — without reaching into
        # solvela.types directly.
        import solvela

        assert "Scheme" in solvela.__all__
        assert solvela.Scheme is not None  # type alias is importable


# --- PaymentPayload ---


class TestPaymentPayload:
    def test_to_dict_solana(self) -> None:
        payload = PaymentPayload(
            x402_version=2,
            resource=Resource(url="https://gw.example.com/v1/chat/completions", method="POST"),
            accepted=PaymentAccept(
                scheme="exact",
                network="solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
                amount="50000",
                asset="EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                pay_to="RecipientWallet111111111111111111111111111",
                max_timeout_seconds=300,
            ),
            payload=SolanaPayload(transaction="base64encodedtx=="),
        )
        d = payload.to_dict()
        assert d["x402_version"] == 2
        assert d["payload"]["transaction"] == "base64encodedtx=="


# --- ModelInfo ---


class TestModelInfo:
    """Wire-format alignment with the gateway's ``GET /v1/models`` response.

    Fixtures intentionally mirror a real ``/v1/models`` entry verbatim so a
    future shape change on the gateway side breaks this test rather than
    silently parsing into stale fields. The previous fixture used a fictional
    flat shape that the gateway never emitted — and as a result
    ``client.models()`` raised ``KeyError`` against any real gateway.
    """

    def test_from_dict_real_gateway_shape(self) -> None:
        # Verbatim from `curl http://localhost:8402/v1/models | jq .data[1]`
        # against a running gateway — the GPT-4o entry.
        data = {
            "id": "openai/gpt-4o",
            "object": "model",
            "provider": "openai",
            "display_name": "GPT-4o",
            "context_window": 128000,
            "capabilities": {
                "streaming": True,
                "tools": True,
                "vision": True,
                "reasoning": False,
            },
            "pricing": {
                "input_per_million": 2.5,
                "output_per_million": 10.0,
                "currency": "USDC",
                "fee_percent": 5,
            },
        }
        info = ModelInfo.from_dict(data)
        assert info.id == "openai/gpt-4o"
        assert info.provider == "openai"
        assert info.display_name == "GPT-4o"
        assert info.context_window == 128000
        # Capabilities flattened off the nested object.
        assert info.supports_streaming is True
        assert info.supports_tools is True
        assert info.supports_vision is True
        assert info.reasoning is False
        # Pricing pulled off the nested object as USDC-per-million floats.
        assert info.input_usdc_per_million == 2.5
        assert info.output_usdc_per_million == 10.0
        assert info.currency == "USDC"
        assert info.fee_percent == 5

    def test_from_dict_defaults_when_capabilities_and_pricing_absent(self) -> None:
        # A minimal entry — gateway is unlikely to ever emit this, but the
        # SDK should not crash if either nested object is missing.
        data = {
            "id": "test/model",
            "provider": "test",
            "display_name": "Test Model",
            "context_window": 4096,
        }
        info = ModelInfo.from_dict(data)
        assert info.supports_streaming is False
        assert info.supports_tools is False
        assert info.reasoning is False
        assert info.input_usdc_per_million == 0.0
        assert info.output_usdc_per_million == 0.0
        assert info.currency == "USDC"
        assert info.fee_percent == 5

    def test_to_dict_round_trips(self) -> None:
        # to_dict() must produce the same nested shape the gateway emits, so
        # ``from_dict(model.to_dict())`` is a stable round-trip. Tests-as-
        # contract guard against the SDK inventing fields the gateway will
        # not recognize on the way back.
        original = ModelInfo(
            id="anthropic/claude-sonnet-4-20250514",
            provider="anthropic",
            display_name="Claude Sonnet 4.6",
            context_window=200000,
            supports_streaming=True,
            supports_tools=True,
            supports_vision=False,
            reasoning=True,
            input_usdc_per_million=3.0,
            output_usdc_per_million=15.0,
            currency="USDC",
            fee_percent=5,
        )
        wire = original.to_dict()
        # Match gateway shape.
        assert wire["capabilities"] == {
            "streaming": True,
            "tools": True,
            "vision": False,
            "reasoning": True,
        }
        assert wire["pricing"]["input_per_million"] == 3.0
        assert wire["pricing"]["output_per_million"] == 15.0
        # Round-trip parity.
        assert ModelInfo.from_dict(wire) == original


# --- Cache key determinism ---


class TestCacheKey:
    def test_same_input_deterministic(self) -> None:
        req1 = ChatRequest(
            model="gpt-4",
            messages=[ChatMessage(role=Role.USER, content="hello")],
        )
        req2 = ChatRequest(
            model="gpt-4",
            messages=[ChatMessage(role=Role.USER, content="hello")],
        )
        assert req1.cache_key() == req2.cache_key()

    def test_different_model_different_key(self) -> None:
        req1 = ChatRequest(
            model="gpt-4",
            messages=[ChatMessage(role=Role.USER, content="hello")],
        )
        req2 = ChatRequest(
            model="gpt-3.5-turbo",
            messages=[ChatMessage(role=Role.USER, content="hello")],
        )
        assert req1.cache_key() != req2.cache_key()


# --- Literal validation (I8) ------------------------------------------------


class TestToolCallTypeValidation:
    """`ToolCall.type` is `Literal["function"]` — wire validation enforces it."""

    def _data(self, type_value: str) -> dict[str, object]:
        return {
            "id": "call_1",
            "type": type_value,
            "function": {"name": "get_weather", "arguments": "{}"},
        }

    def test_accepts_function_type(self) -> None:
        from solvela.types import ToolCall

        call = ToolCall.from_dict(self._data("function"))
        assert call.type == "function"

    def test_rejects_unknown_type(self) -> None:
        from solvela.types import ToolCall

        with pytest.raises(ValueError, match="Unknown tool type"):
            ToolCall.from_dict(self._data("magic"))


class TestToolCallDeltaTypeValidation:
    """Streaming partial of ToolCall must enforce the same Literal as ToolCall."""

    def test_accepts_none(self) -> None:
        from solvela.types import ToolCallDelta

        delta = ToolCallDelta.from_dict({"index": 0})
        assert delta.type is None

    def test_accepts_function_type(self) -> None:
        from solvela.types import ToolCallDelta

        delta = ToolCallDelta.from_dict({"index": 0, "type": "function"})
        assert delta.type == "function"

    def test_rejects_unknown_type(self) -> None:
        from solvela.types import ToolCallDelta

        with pytest.raises(ValueError, match="Unknown tool type"):
            ToolCallDelta.from_dict({"index": 0, "type": "magic"})


class TestAtomicUsdc:
    """``AtomicUsdc`` is a NewType — zero runtime overhead, type-only distinction."""

    def test_is_int_at_runtime(self) -> None:
        from solvela.types import AtomicUsdc

        # NewType is a callable that returns its argument unchanged. Runtime
        # type is plain int — the discrimination only exists for mypy.
        value = AtomicUsdc(1_000_000)
        assert value == 1_000_000
        assert isinstance(value, int)

    def test_arithmetic_works(self) -> None:
        from solvela.types import AtomicUsdc

        # Internal arithmetic between AtomicUsdc values still produces ints
        # at runtime; callers must re-wrap if they want the type back.
        a = AtomicUsdc(500_000)
        b = AtomicUsdc(750_000)
        assert AtomicUsdc(a + b) == 1_250_000

    def test_exposed_in_package_surface(self) -> None:
        # I10 added wire-type exports; AtomicUsdc joins that surface so
        # callers can annotate their own atomic-unit values without
        # reaching into ``solvela.types``.
        import solvela

        # Assert the export is callable and round-trips a value, not just
        # truthy — `is not None` would pass even on a stale export shadow.
        assert callable(solvela.AtomicUsdc)
        assert solvela.AtomicUsdc(1) == 1
        assert "AtomicUsdc" in solvela.__all__


class TestFinishReasonValidation:
    """`finish_reason` is the closed Literal stop|length|tool_calls|content_filter."""

    def _choice(self, finish_reason: str | None) -> dict[str, object]:
        return {
            "index": 0,
            "message": {"role": "assistant", "content": "ok"},
            "finish_reason": finish_reason,
        }

    @pytest.mark.parametrize("reason", ["stop", "length", "tool_calls", "content_filter", None])
    def test_accepts_known_reasons(self, reason: str | None) -> None:
        from solvela.types import ChatChoice

        choice = ChatChoice.from_dict(self._choice(reason))
        assert choice.finish_reason == reason

    def test_rejects_unknown_reason(self) -> None:
        from solvela.types import ChatChoice

        with pytest.raises(ValueError, match="Unknown finish_reason"):
            ChatChoice.from_dict(self._choice("function_call"))
