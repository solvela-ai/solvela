"""Solvela wire-format types — OpenAI-compatible chat and x402 payment types."""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from enum import Enum
from typing import Any, Literal, NewType, get_args

from solvela.errors import ClientError

# USDC amounts in atomic units (1 USDC = 1_000_000 atomic). NewType is a
# zero-cost runtime wrapper but a real distinction at the type level — mypy
# treats AtomicUsdc and int as incompatible, so an internal site that
# accidentally passes a human-USDC float or a wire-string-parsed int without
# the explicit ``AtomicUsdc(...)`` cast is flagged before it reaches the
# signer or an error message.
AtomicUsdc = NewType("AtomicUsdc", int)

# OpenAI tool-call domain. Only "function" is defined today; broaden the
# Literal here when the upstream protocol grows.
ToolType = Literal["function"]
_KNOWN_TOOL_TYPES: frozenset[str] = frozenset(get_args(ToolType))

# OpenAI finish-reason domain. The set is closed by the OpenAI spec
# (stop/length/tool_calls/content_filter); validating here surfaces an
# unmodeled extension instead of silently propagating an unknown string
# through the choice handler.
FinishReason = Literal["stop", "length", "tool_calls", "content_filter"]
_KNOWN_FINISH_REASONS: frozenset[str] = frozenset(get_args(FinishReason))

# x402 schemes recognized by this client. Adding a new scheme requires updating
# `_find_compatible_scheme` in client.py and the `Scheme` Literal below.
Scheme = Literal["exact", "escrow"]
_KNOWN_SCHEMES: frozenset[str] = frozenset(get_args(Scheme))


def _validate_tool_type(value: str) -> ToolType:
    if value not in _KNOWN_TOOL_TYPES:
        raise ValueError(f"Unknown tool type: {value!r}")
    return value  # type: ignore[return-value]


def _validate_optional_tool_type(value: str | None) -> ToolType | None:
    if value is None:
        return None
    return _validate_tool_type(value)


def _validate_finish_reason(value: str | None) -> FinishReason | None:
    if value is None:
        return None
    if value not in _KNOWN_FINISH_REASONS:
        raise ValueError(f"Unknown finish_reason: {value!r}")
    return value  # type: ignore[return-value]


class Role(str, Enum):
    """Chat message roles (3.10-compatible StrEnum)."""

    SYSTEM = "system"
    USER = "user"
    ASSISTANT = "assistant"
    TOOL = "tool"
    DEVELOPER = "developer"


# ---------------------------------------------------------------------------
# Tool / Function types
# ---------------------------------------------------------------------------


@dataclass
class FunctionCall:
    """A function call made by the model."""

    name: str
    arguments: str

    def to_dict(self) -> dict[str, Any]:
        return {"name": self.name, "arguments": self.arguments}

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> FunctionCall:
        return cls(name=data["name"], arguments=data["arguments"])


@dataclass
class FunctionCallDelta:
    """Partial function call in a streaming chunk."""

    name: str | None = None
    arguments: str | None = None

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {}
        if self.name is not None:
            d["name"] = self.name
        if self.arguments is not None:
            d["arguments"] = self.arguments
        return d

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> FunctionCallDelta:
        return cls(name=data.get("name"), arguments=data.get("arguments"))


@dataclass
class ToolCall:
    """A tool call returned by the model."""

    id: str
    type: ToolType
    function: FunctionCall

    def to_dict(self) -> dict[str, Any]:
        return {"id": self.id, "type": self.type, "function": self.function.to_dict()}

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ToolCall:
        return cls(
            id=data["id"],
            type=_validate_tool_type(data["type"]),
            function=FunctionCall.from_dict(data["function"]),
        )


@dataclass
class ToolCallDelta:
    """Partial tool call in a streaming chunk."""

    index: int
    id: str | None = None
    type: str | None = None
    function: FunctionCallDelta | None = None

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {"index": self.index}
        if self.id is not None:
            d["id"] = self.id
        if self.type is not None:
            d["type"] = self.type
        if self.function is not None:
            d["function"] = self.function.to_dict()
        return d

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ToolCallDelta:
        fn = data.get("function")
        return cls(
            index=data["index"],
            id=data.get("id"),
            type=_validate_optional_tool_type(data.get("type")),
            function=FunctionCallDelta.from_dict(fn) if fn else None,
        )


@dataclass
class FunctionDefinitionInner:
    """Inner function definition within a tool definition."""

    name: str
    description: str | None = None
    parameters: dict[str, Any] | None = None

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {"name": self.name}
        if self.description is not None:
            d["description"] = self.description
        if self.parameters is not None:
            d["parameters"] = self.parameters
        return d

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> FunctionDefinitionInner:
        return cls(
            name=data["name"],
            description=data.get("description"),
            parameters=data.get("parameters"),
        )


@dataclass
class ToolDefinition:
    """Tool definition for function calling."""

    type: ToolType
    function: FunctionDefinitionInner

    def to_dict(self) -> dict[str, Any]:
        return {"type": self.type, "function": self.function.to_dict()}

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ToolDefinition:
        return cls(
            type=_validate_tool_type(data["type"]),
            function=FunctionDefinitionInner.from_dict(data["function"]),
        )


# ---------------------------------------------------------------------------
# Chat types
# ---------------------------------------------------------------------------


@dataclass
class ChatMessage:
    """A single chat message."""

    role: Role
    content: str | None
    name: str | None = None
    tool_calls: list[ToolCall] | None = None
    tool_call_id: str | None = None

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {"role": self.role.value, "content": self.content}
        if self.name is not None:
            d["name"] = self.name
        if self.tool_calls is not None:
            d["tool_calls"] = [tc.to_dict() for tc in self.tool_calls]
        if self.tool_call_id is not None:
            d["tool_call_id"] = self.tool_call_id
        return d

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ChatMessage:
        raw_tool_calls = data.get("tool_calls")
        tool_calls = (
            [ToolCall.from_dict(tc) for tc in raw_tool_calls]
            if raw_tool_calls is not None
            else None
        )
        return cls(
            role=Role(data["role"]),
            content=data.get("content"),
            name=data.get("name"),
            tool_calls=tool_calls,
            tool_call_id=data.get("tool_call_id"),
        )


@dataclass
class ChatRequest:
    """OpenAI-compatible chat completion request."""

    model: str
    messages: list[ChatMessage]
    max_tokens: int | None = None
    temperature: float | None = None
    top_p: float | None = None
    stream: bool = False
    tools: list[ToolDefinition] | None = None
    tool_choice: str | dict[str, Any] | None = None

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {
            "model": self.model,
            "messages": [m.to_dict() for m in self.messages],
            "stream": self.stream,
        }
        if self.max_tokens is not None:
            d["max_tokens"] = self.max_tokens
        if self.temperature is not None:
            d["temperature"] = self.temperature
        if self.top_p is not None:
            d["top_p"] = self.top_p
        if self.tools is not None:
            d["tools"] = [t.to_dict() for t in self.tools]
        if self.tool_choice is not None:
            d["tool_choice"] = self.tool_choice
        return d

    def cache_key(self) -> str:
        """Deterministic cache key for this request."""
        canonical = json.dumps(self.to_dict(), sort_keys=True, separators=(",", ":"))
        return hashlib.sha256(canonical.encode()).hexdigest()

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ChatRequest:
        raw_tools = data.get("tools")
        tools = [ToolDefinition.from_dict(t) for t in raw_tools] if raw_tools is not None else None
        return cls(
            model=data["model"],
            messages=[ChatMessage.from_dict(m) for m in data["messages"]],
            max_tokens=data.get("max_tokens"),
            temperature=data.get("temperature"),
            top_p=data.get("top_p"),
            stream=data.get("stream", False),
            tools=tools,
            tool_choice=data.get("tool_choice"),
        )


@dataclass
class Usage:
    """Token usage statistics."""

    prompt_tokens: int
    completion_tokens: int
    total_tokens: int

    def to_dict(self) -> dict[str, Any]:
        return {
            "prompt_tokens": self.prompt_tokens,
            "completion_tokens": self.completion_tokens,
            "total_tokens": self.total_tokens,
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> Usage:
        return cls(
            prompt_tokens=data["prompt_tokens"],
            completion_tokens=data["completion_tokens"],
            total_tokens=data["total_tokens"],
        )


@dataclass
class ChatChoice:
    """A single choice in a chat completion response."""

    index: int
    message: ChatMessage
    finish_reason: FinishReason | None = None

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {"index": self.index, "message": self.message.to_dict()}
        if self.finish_reason is not None:
            d["finish_reason"] = self.finish_reason
        return d

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ChatChoice:
        return cls(
            index=data["index"],
            message=ChatMessage.from_dict(data["message"]),
            finish_reason=_validate_finish_reason(data.get("finish_reason")),
        )


@dataclass
class ChatResponse:
    """OpenAI-compatible chat completion response."""

    id: str
    object: str
    created: int
    model: str
    choices: list[ChatChoice]
    usage: Usage | None = None

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {
            "id": self.id,
            "object": self.object,
            "created": self.created,
            "model": self.model,
            "choices": [c.to_dict() for c in self.choices],
        }
        if self.usage is not None:
            d["usage"] = self.usage.to_dict()
        return d

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ChatResponse:
        raw_usage = data.get("usage")
        return cls(
            id=data["id"],
            object=data["object"],
            created=data["created"],
            model=data["model"],
            choices=[ChatChoice.from_dict(c) for c in data["choices"]],
            usage=Usage.from_dict(raw_usage) if raw_usage else None,
        )


# ---------------------------------------------------------------------------
# Streaming chunk types
# ---------------------------------------------------------------------------


@dataclass
class ChatDelta:
    """Delta content in a streaming chunk."""

    role: str | None = None
    content: str | None = None
    tool_calls: list[ToolCallDelta] | None = None

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {}
        if self.role is not None:
            d["role"] = self.role
        if self.content is not None:
            d["content"] = self.content
        if self.tool_calls is not None:
            d["tool_calls"] = [tc.to_dict() for tc in self.tool_calls]
        return d

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ChatDelta:
        raw_tc = data.get("tool_calls")
        return cls(
            role=data.get("role"),
            content=data.get("content"),
            tool_calls=[ToolCallDelta.from_dict(tc) for tc in raw_tc] if raw_tc else None,
        )


@dataclass
class ChatChunkChoice:
    """A single choice in a streaming chunk."""

    index: int
    delta: ChatDelta
    finish_reason: FinishReason | None = None

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {"index": self.index, "delta": self.delta.to_dict()}
        if self.finish_reason is not None:
            d["finish_reason"] = self.finish_reason
        return d

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ChatChunkChoice:
        return cls(
            index=data["index"],
            delta=ChatDelta.from_dict(data["delta"]),
            finish_reason=_validate_finish_reason(data.get("finish_reason")),
        )


@dataclass
class ChatChunk:
    """OpenAI-compatible streaming chunk."""

    id: str
    object: str
    created: int
    model: str
    choices: list[ChatChunkChoice]

    def to_dict(self) -> dict[str, Any]:
        return {
            "id": self.id,
            "object": self.object,
            "created": self.created,
            "model": self.model,
            "choices": [c.to_dict() for c in self.choices],
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ChatChunk:
        return cls(
            id=data["id"],
            object=data["object"],
            created=data["created"],
            model=data["model"],
            choices=[ChatChunkChoice.from_dict(c) for c in data["choices"]],
        )


# ---------------------------------------------------------------------------
# Payment / x402 types
# ---------------------------------------------------------------------------


@dataclass
class Resource:
    """HTTP resource identifier."""

    url: str
    method: str

    def to_dict(self) -> dict[str, Any]:
        return {"url": self.url, "method": self.method}

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> Resource:
        return cls(url=data["url"], method=data["method"])


@dataclass
class PaymentAccept:
    """An accepted payment scheme."""

    scheme: Scheme
    network: str
    amount: str
    asset: str
    pay_to: str
    max_timeout_seconds: int
    escrow_program_id: str | None = None

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {
            "scheme": self.scheme,
            "network": self.network,
            "amount": self.amount,
            "asset": self.asset,
            "pay_to": self.pay_to,
            "max_timeout_seconds": self.max_timeout_seconds,
        }
        if self.escrow_program_id is not None:
            d["escrow_program_id"] = self.escrow_program_id
        return d

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> PaymentAccept:
        scheme = data["scheme"]
        if scheme not in _KNOWN_SCHEMES:
            # Domain-encoded at the type level; an unknown scheme means either
            # a malformed gateway response or a protocol upgrade the SDK has
            # not been taught yet. Raising here forces the caller to handle
            # the mismatch explicitly rather than silently mis-branching at
            # the scheme-matching call site.
            #
            # ClientError (not ValueError) so the wire-decoding failure stays
            # inside the SDK's typed error hierarchy and is caught by callers
            # using `except ClientError`.
            raise ClientError(f"Unknown payment scheme: {scheme!r}")
        return cls(
            scheme=scheme,
            network=data["network"],
            amount=data["amount"],
            asset=data["asset"],
            pay_to=data["pay_to"],
            max_timeout_seconds=data["max_timeout_seconds"],
            escrow_program_id=data.get("escrow_program_id"),
        )


@dataclass
class CostBreakdown:
    """Cost breakdown for a request."""

    provider_cost: str
    platform_fee: str
    total: str
    currency: str
    fee_percent: int

    def to_dict(self) -> dict[str, Any]:
        return {
            "provider_cost": self.provider_cost,
            "platform_fee": self.platform_fee,
            "total": self.total,
            "currency": self.currency,
            "fee_percent": self.fee_percent,
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> CostBreakdown:
        return cls(
            provider_cost=data["provider_cost"],
            platform_fee=data["platform_fee"],
            total=data["total"],
            currency=data["currency"],
            fee_percent=data["fee_percent"],
        )


@dataclass
class PaymentRequired:
    """402 Payment Required response body."""

    x402_version: int
    resource: Resource
    accepts: list[PaymentAccept]
    cost_breakdown: CostBreakdown
    error: str

    def to_dict(self) -> dict[str, Any]:
        return {
            "x402_version": self.x402_version,
            "resource": self.resource.to_dict(),
            "accepts": [a.to_dict() for a in self.accepts],
            "cost_breakdown": self.cost_breakdown.to_dict(),
            "error": self.error,
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> PaymentRequired:
        return cls(
            x402_version=data["x402_version"],
            resource=Resource.from_dict(data["resource"]),
            accepts=[PaymentAccept.from_dict(a) for a in data["accepts"]],
            cost_breakdown=CostBreakdown.from_dict(data["cost_breakdown"]),
            error=data["error"],
        )


@dataclass
class SolanaPayload:
    """Solana direct-payment payload."""

    transaction: str

    def to_dict(self) -> dict[str, Any]:
        return {"transaction": self.transaction}

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> SolanaPayload:
        return cls(transaction=data["transaction"])


@dataclass
class EscrowPayload:
    """Escrow payment payload."""

    deposit_tx: str
    service_id: str
    agent_pubkey: str

    def to_dict(self) -> dict[str, Any]:
        return {
            "deposit_tx": self.deposit_tx,
            "service_id": self.service_id,
            "agent_pubkey": self.agent_pubkey,
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> EscrowPayload:
        return cls(
            deposit_tx=data["deposit_tx"],
            service_id=data["service_id"],
            agent_pubkey=data["agent_pubkey"],
        )


@dataclass
class PaymentPayload:
    """Payment header payload sent to the gateway."""

    x402_version: int
    resource: Resource
    accepted: PaymentAccept
    payload: SolanaPayload | EscrowPayload

    def to_dict(self) -> dict[str, Any]:
        return {
            "x402_version": self.x402_version,
            "resource": self.resource.to_dict(),
            "accepted": self.accepted.to_dict(),
            "payload": self.payload.to_dict(),
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> PaymentPayload:
        raw_payload = data["payload"]
        if "transaction" in raw_payload:
            payload: SolanaPayload | EscrowPayload = SolanaPayload.from_dict(raw_payload)
        else:
            payload = EscrowPayload.from_dict(raw_payload)
        return cls(
            x402_version=data["x402_version"],
            resource=Resource.from_dict(data["resource"]),
            accepted=PaymentAccept.from_dict(data["accepted"]),
            payload=payload,
        )


# ---------------------------------------------------------------------------
# Model info
# ---------------------------------------------------------------------------


@dataclass
class ModelInfo:
    """Model metadata from the gateway's ``GET /v1/models`` registry.

    Mirrors the gateway's response shape: capabilities are reported under a
    nested ``capabilities`` object, and pricing under a nested ``pricing``
    object. ``from_dict`` flattens those so internal code stays terse.

    Pricing values are USDC per million tokens as floats (e.g. ``0.28`` for
    DeepSeek's input rate), not atomic USDC units — the gateway formats them
    that way to match the human-readable shape of every other LLM pricing
    page. Convert at the boundary if you need atomic units.
    """

    id: str
    provider: str
    display_name: str
    context_window: int
    supports_streaming: bool = False
    supports_tools: bool = False
    supports_vision: bool = False
    reasoning: bool = False
    input_usdc_per_million: float = 0.0
    output_usdc_per_million: float = 0.0
    currency: str = "USDC"
    fee_percent: int = 5

    def to_dict(self) -> dict[str, Any]:
        return {
            "id": self.id,
            "object": "model",
            "provider": self.provider,
            "display_name": self.display_name,
            "context_window": self.context_window,
            "capabilities": {
                "streaming": self.supports_streaming,
                "tools": self.supports_tools,
                "vision": self.supports_vision,
                "reasoning": self.reasoning,
            },
            "pricing": {
                "input_per_million": self.input_usdc_per_million,
                "output_per_million": self.output_usdc_per_million,
                "currency": self.currency,
                "fee_percent": self.fee_percent,
            },
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ModelInfo:
        caps = data.get("capabilities") or {}
        pricing = data.get("pricing") or {}
        return cls(
            id=data["id"],
            provider=data["provider"],
            display_name=data["display_name"],
            context_window=data["context_window"],
            supports_streaming=bool(caps.get("streaming", False)),
            supports_tools=bool(caps.get("tools", False)),
            supports_vision=bool(caps.get("vision", False)),
            reasoning=bool(caps.get("reasoning", False)),
            input_usdc_per_million=float(pricing.get("input_per_million", 0.0)),
            output_usdc_per_million=float(pricing.get("output_per_million", 0.0)),
            currency=pricing.get("currency", "USDC"),
            fee_percent=int(pricing.get("fee_percent", 5)),
        )
