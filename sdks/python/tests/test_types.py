"""Tests for Pydantic model serialization/deserialization."""

import pytest

from rustyclawrouter.types import (
    ChatChoice,
    ChatMessage,
    ChatRequest,
    ChatResponse,
    CostBreakdown,
    PaymentAccept,
    PaymentRequired,
    Role,
    SpendInfo,
    Usage,
)


class TestRole:
    def test_role_values(self):
        assert Role.SYSTEM == "system"
        assert Role.USER == "user"
        assert Role.ASSISTANT == "assistant"
        assert Role.TOOL == "tool"

    def test_role_is_string(self):
        assert isinstance(Role.USER, str)
        assert Role.USER == "user"


class TestChatMessage:
    def test_basic_message(self):
        msg = ChatMessage(role=Role.USER, content="Hello")
        assert msg.role == Role.USER
        assert msg.content == "Hello"
        assert msg.name is None

    def test_message_with_name(self):
        msg = ChatMessage(role=Role.ASSISTANT, content="Hi", name="bot")
        assert msg.name == "bot"

    def test_message_serialization(self):
        msg = ChatMessage(role=Role.USER, content="Hello")
        data = msg.model_dump()
        assert data == {"role": "user", "content": "Hello", "name": None}

    def test_message_deserialization(self):
        data = {"role": "assistant", "content": "World"}
        msg = ChatMessage.model_validate(data)
        assert msg.role == Role.ASSISTANT
        assert msg.content == "World"

    def test_message_from_json_string_role(self):
        data = {"role": "system", "content": "You are helpful."}
        msg = ChatMessage.model_validate(data)
        assert msg.role == Role.SYSTEM


class TestChatRequest:
    def test_minimal_request(self):
        req = ChatRequest(
            model="gpt-4o",
            messages=[ChatMessage(role=Role.USER, content="Hi")],
        )
        assert req.model == "gpt-4o"
        assert len(req.messages) == 1
        assert req.max_tokens is None
        assert req.temperature is None
        assert req.stream is False

    def test_full_request(self):
        req = ChatRequest(
            model="claude-sonnet-4",
            messages=[
                ChatMessage(role=Role.SYSTEM, content="Be concise."),
                ChatMessage(role=Role.USER, content="Hello"),
            ],
            max_tokens=100,
            temperature=0.7,
            top_p=0.9,
            stream=True,
        )
        assert req.max_tokens == 100
        assert req.temperature == 0.7
        assert req.top_p == 0.9
        assert req.stream is True

    def test_request_serialization(self):
        req = ChatRequest(
            model="gpt-4o",
            messages=[ChatMessage(role=Role.USER, content="Hi")],
        )
        data = req.model_dump()
        assert data["model"] == "gpt-4o"
        assert data["stream"] is False
        assert len(data["messages"]) == 1


class TestUsage:
    def test_usage(self):
        usage = Usage(prompt_tokens=10, completion_tokens=20, total_tokens=30)
        assert usage.prompt_tokens == 10
        assert usage.completion_tokens == 20
        assert usage.total_tokens == 30

    def test_usage_roundtrip(self):
        data = {"prompt_tokens": 5, "completion_tokens": 15, "total_tokens": 20}
        usage = Usage.model_validate(data)
        assert usage.model_dump() == data


class TestChatResponse:
    def test_full_response(self):
        resp = ChatResponse(
            id="chatcmpl-abc123",
            object="chat.completion",
            created=1700000000,
            model="gpt-4o",
            choices=[
                ChatChoice(
                    index=0,
                    message=ChatMessage(role=Role.ASSISTANT, content="Hello!"),
                    finish_reason="stop",
                )
            ],
            usage=Usage(prompt_tokens=10, completion_tokens=5, total_tokens=15),
        )
        assert resp.id == "chatcmpl-abc123"
        assert resp.choices[0].message.content == "Hello!"
        assert resp.usage.total_tokens == 15

    def test_response_deserialization(self):
        data = {
            "id": "chatcmpl-xyz",
            "object": "chat.completion",
            "created": 1700000000,
            "model": "claude-sonnet-4",
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": "Hi there"},
                    "finish_reason": "stop",
                }
            ],
            "usage": {
                "prompt_tokens": 8,
                "completion_tokens": 3,
                "total_tokens": 11,
            },
        }
        resp = ChatResponse.model_validate(data)
        assert resp.model == "claude-sonnet-4"
        assert resp.choices[0].message.role == Role.ASSISTANT

    def test_response_without_usage(self):
        data = {
            "id": "chatcmpl-nousage",
            "object": "chat.completion",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": "Ok"},
                    "finish_reason": "stop",
                }
            ],
        }
        resp = ChatResponse.model_validate(data)
        assert resp.usage is None


class TestCostBreakdown:
    def test_cost_breakdown(self):
        cb = CostBreakdown(
            provider_cost="0.002500",
            platform_fee="0.000125",
            total="0.002625",
            currency="USDC",
            fee_percent=5,
        )
        assert cb.provider_cost == "0.002500"
        assert cb.fee_percent == 5
        assert cb.currency == "USDC"

    def test_cost_breakdown_roundtrip(self):
        data = {
            "provider_cost": "0.001000",
            "platform_fee": "0.000050",
            "total": "0.001050",
            "currency": "USDC",
            "fee_percent": 5,
        }
        cb = CostBreakdown.model_validate(data)
        dumped = cb.model_dump()
        assert dumped == data


class TestPaymentRequired:
    def test_payment_required(self):
        pr = PaymentRequired(
            x402_version=2,
            accepts=[
                PaymentAccept(
                    scheme="exact",
                    network="solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
                    amount="2625",
                    asset="EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                    pay_to="RecipientWalletPubkey",
                    max_timeout_seconds=300,
                )
            ],
            cost_breakdown=CostBreakdown(
                provider_cost="0.002500",
                platform_fee="0.000125",
                total="0.002625",
                currency="USDC",
                fee_percent=5,
            ),
            error="Payment required",
        )
        assert pr.x402_version == 2
        assert len(pr.accepts) == 1
        assert pr.accepts[0].scheme == "exact"

    def test_payment_required_deserialization(self):
        data = {
            "x402_version": 2,
            "accepts": [
                {
                    "scheme": "exact",
                    "network": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
                    "amount": "1000",
                    "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                    "pay_to": "SomeWallet",
                    "max_timeout_seconds": 300,
                }
            ],
            "cost_breakdown": {
                "provider_cost": "0.001000",
                "platform_fee": "0.000050",
                "total": "0.001050",
                "currency": "USDC",
                "fee_percent": 5,
            },
            "error": "Payment required",
        }
        pr = PaymentRequired.model_validate(data)
        assert pr.accepts[0].amount == "1000"
        assert pr.cost_breakdown.total == "0.001050"


class TestSpendInfo:
    def test_defaults(self):
        info = SpendInfo()
        assert info.total_requests == 0
        assert info.total_cost_usdc == 0.0
        assert info.daily_cost_usdc == 0.0

    def test_with_values(self):
        info = SpendInfo(
            total_requests=42,
            total_cost_usdc=1.23,
            daily_cost_usdc=0.45,
        )
        assert info.total_requests == 42
        assert info.total_cost_usdc == 1.23
