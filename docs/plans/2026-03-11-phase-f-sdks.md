# Phase F: Language SDKs Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build Python, TypeScript, and Go SDKs for Solvela, each in its own repo, mirroring the Rust client's architecture 1:1.

> **Note:** Working SDK implementations already exist at `sdks/python/`, `sdks/typescript/`, and `sdks/go/` within the Solvela repo. This plan documents the canonical architecture and type definitions. When implementing, align with or replace the existing in-tree SDKs rather than creating new standalone repos.

**Architecture:** Layered client — thin HTTP transport + pluggable Solana signer + opt-in smart features (cache, sessions, balance monitor, quality check). Fresh implementations using the Rust client as canonical reference. OpenAI-compatible wrapper for Python and TypeScript.

**Tech Stack:**
- Python 3.10+: `httpx` (async HTTP), `solders` (Solana), `pytest` + `pytest-asyncio`
- TypeScript (Node 18+): native `fetch`, `@solana/web3.js`, `vitest`
- Go 1.21+: stdlib `net/http`, `github.com/gagliardetto/solana-go`, stdlib `testing`

---

## Shared Context

### Wire-Format Types (re-implement in each SDK)

All types come from `rustyclaw-protocol`. JSON field names must match exactly.

**Chat types:**
- `Role` enum: `system`, `user`, `assistant`, `tool`, `developer`
- `ChatMessage`: `role`, `content`, `name?`, `tool_calls?`, `tool_call_id?`
- `ChatRequest`: `model`, `messages`, `max_tokens?`, `temperature?`, `top_p?`, `stream` (default false), `tools?`, `tool_choice?`
- `ChatResponse`: `id`, `object`, `created`, `model`, `choices[]`, `usage?`
- `ChatChoice`: `index`, `message`, `finish_reason?`
- `Usage`: `prompt_tokens`, `completion_tokens`, `total_tokens`

**Streaming types:**
- `ChatChunk`: `id`, `object`, `created`, `model`, `choices[]`
- `ChatChunkChoice`: `index`, `delta`, `finish_reason?`
- `ChatDelta`: `role?`, `content?`, `tool_calls?`

**Tool types:**
- `ToolCall`: `id`, `type`, `function: { name, arguments }`
- `ToolCallDelta`: `index`, `id?`, `type?`, `function?: { name?, arguments? }`
- `ToolDefinition`: `type`, `function: { name, description?, parameters? }`

**Payment types:**
- `Resource`: `url`, `method`
- `PaymentAccept`: `scheme`, `network`, `amount`, `asset`, `pay_to`, `max_timeout_seconds`, `escrow_program_id?`
- `PaymentRequired` (402 body): `x402_version`, `resource`, `accepts[]`, `cost_breakdown`, `error`

> **Known gap:** Existing in-tree Python and TypeScript SDKs are missing the `resource` field on `PaymentRequired`. This must be added.
- `PaymentPayload` (header): `x402_version`, `resource`, `accepted`, `payload` (Direct or Escrow variant)

> **Deserialization note:** `payload` uses untagged deserialization. When deserializing, attempt `EscrowPayload` first (check for `deposit_tx` field), fall back to `SolanaPayload`. This matches Rust's `#[serde(untagged)]` enum ordering.

- `SolanaPayload`: `transaction` (base64)
- `EscrowPayload`: `deposit_tx`, `service_id`, `agent_pubkey`
- `CostBreakdown`: `provider_cost`, `platform_fee`, `total`, `currency`, `fee_percent`

**Model types:**
- `ModelInfo`: `id`, `provider`, `model_id`, `display_name`, `input_cost_per_million`, `output_cost_per_million`, `context_window`, `supports_streaming`, `supports_tools`, `supports_vision`, `reasoning`, `supports_structured_output`, `supports_batch`, `max_output_tokens?`

**Vision types (multi-modal support):**
- `ContentPart`: Tagged union — `TextPart { type: "text", text }` | `ImagePart { type: "image_url", image_url: ImageUrl }`
- `ImageUrl`: `url`, `detail?` (enum: `auto`, `low`, `high`)

> These types are needed for models with `supports_vision: true`. `ChatMessage.content` can be either a plain string or an array of `ContentPart`.

**Constants:**
- `X402_VERSION = 2`
- `USDC_MINT = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"`
- `SOLANA_NETWORK = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"`
- `MAX_TIMEOUT_SECONDS = 300`
- `PLATFORM_FEE_PERCENT = 5`

### Smart Chat Flow (same for all SDKs)

**`chat()` — 7 steps:**
1. Balance guard — if known balance == 0 and `free_fallback_model` set → swap model
2. Session lookup — `derive_session_id(messages)` → `get_or_create(id, model)` → may override model
3. Cache check — key = `hash(finalized_model + messages)`, return if hit
4. Send request — probe without payment; if 402 → sign + resend; if error → raise
5. Quality check — if degraded and retries left → resend with `X-RCR-Retry-Reason: degraded`
6. Cache store — put response in LRU
7. Session update — `record_request(session_id, request_hash)`

**`chat_stream()` — 3 steps:**
1. Balance guard + session lookup (steps 1-2)
2. Send streaming request (SSE)
3. Session update

### Quality Check Heuristics

A response is "degraded" if any of:
- Empty/whitespace-only content
- Contains known error phrases: "i cannot", "as an ai", "i'm sorry, but i" (case-insensitive)
- Any 3-word phrase repeats 5+ times (repetitive loop)
- Content >100 chars and ends with alphanumeric (truncated mid-word)

### Signer Interface

```
trait/interface/protocol Signer:
    sign_payment(amount_lamports: u64, recipient: str, memo: str) -> PaymentPayload

class KeypairSigner(Signer):
    constructor(keypair, rpc_url)
    sign_payment():
        1. Derive sender ATA for USDC mint
        2. Derive recipient ATA for USDC mint
        3. Build SPL token transfer instruction
        4. Add memo instruction
        5. Get recent blockhash
        6. Build and sign transaction
        7. Return PaymentPayload with base64 tx
```

### Payment Flow (inside `send_request`)

1. Send request without `Payment-Signature` header
2. If 200 → return response
3. If 402 → parse `PaymentRequired` from body
4. Validate: check `expected_recipient` matches, check `max_payment_amount` not exceeded
5. Find compatible scheme (prefer "exact", fall back to "escrow")
6. Call `signer.sign_payment(amount, recipient, memo)`
7. Base64-encode `PaymentPayload` JSON → set as `Payment-Signature` header
8. Resend request with header
9. If 200 → return response; else raise error

---

## Part 1: Python SDK

### Task P1: Project Scaffold

**Files:**
- Create: `pyproject.toml`
- Create: `src/rustyclaw/__init__.py`
- Create: `src/rustyclaw/py.typed` (PEP 561 marker)
- Create: `tests/__init__.py`
- Create: `tests/unit/__init__.py`
- Create: `tests/integration/__init__.py`
- Create: `tests/live/__init__.py`
- Create: `.gitignore`

**Step 1: Create GitHub repo**

```bash
mkdir rustyclaw-python && cd rustyclaw-python
git init
```

**Step 2: Create pyproject.toml**

```toml
[project]
name = "rustyclaw"
version = "0.1.0"
description = "Python SDK for Solvela — Solana-native AI agent payment gateway"
requires-python = ">=3.10"
license = "MIT"
dependencies = [
    "httpx>=0.27",
    "solders>=0.21",
]

[project.optional-dependencies]
dev = [
    "pytest>=8",
    "pytest-asyncio>=0.23",
    "pytest-httpx>=0.30",
    "ruff>=0.4",
    "mypy>=1.10",
]

[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[tool.pytest.ini_options]
asyncio_mode = "auto"

[tool.ruff]
target-version = "py310"
line-length = 100

[tool.ruff.lint]
select = ["E", "F", "I", "N", "UP", "B", "SIM", "TCH"]

[tool.mypy]
python_version = "3.10"
strict = true
```

**Step 3: Create `__init__.py` with public API**

```python
"""RustyClaw Python SDK — Solana-native AI agent payment client."""

from rustyclaw.client import Solvela Client
from rustyclaw.config import ClientConfig, ClientBuilder
from rustyclaw.errors import ClientError, WalletError, SignerError
from rustyclaw.wallet import Wallet

__all__ = [
    "Solvela Client",
    "ClientConfig",
    "ClientBuilder",
    "ClientError",
    "WalletError",
    "SignerError",
    "Wallet",
]
```

**Step 4: Create empty module files**

Create empty files: `types.py`, `constants.py`, `errors.py`, `config.py`, `wallet.py`, `signer.py`, `transport.py`, `cache.py`, `session.py`, `balance.py`, `quality.py`, `client.py`, `openai_compat.py` in `src/rustyclaw/`.

**Step 5: Create .gitignore**

```
__pycache__/
*.pyc
.mypy_cache/
.ruff_cache/
dist/
*.egg-info/
.venv/
```

**Step 6: Commit**

```bash
git add -A
git commit -m "chore: scaffold Python SDK project"
```

---

### Task P2: Types & Constants

**Files:**
- Create: `src/rustyclaw/types.py`
- Create: `src/rustyclaw/constants.py`
- Test: `tests/unit/test_types.py`

**Step 1: Write failing tests**

```python
# tests/unit/test_types.py
import json
from rustyclaw.types import (
    Role, ChatMessage, ChatRequest, ChatResponse, ChatChoice, Usage,
    ChatChunk, ChatChunkChoice, ChatDelta,
    ToolCall, FunctionCall, ToolDefinition,
    PaymentRequired, PaymentAccept, CostBreakdown, PaymentPayload,
    Resource, SolanaPayload, ModelInfo,
)
from rustyclaw.constants import X402_VERSION, USDC_MINT, SOLANA_NETWORK

def test_role_serialization():
    assert Role.USER.value == "user"
    assert Role.ASSISTANT.value == "assistant"
    assert Role.SYSTEM.value == "system"
    assert Role.TOOL.value == "tool"
    assert Role.DEVELOPER.value == "developer"

def test_chat_message_to_dict():
    msg = ChatMessage(role=Role.USER, content="Hello")
    d = msg.to_dict()
    assert d == {"role": "user", "content": "Hello"}

def test_chat_message_with_optional_fields():
    msg = ChatMessage(role=Role.ASSISTANT, content="Hi", name="bot")
    d = msg.to_dict()
    assert d["name"] == "bot"

def test_chat_message_omits_none_fields():
    msg = ChatMessage(role=Role.USER, content="Hello")
    d = msg.to_dict()
    assert "name" not in d
    assert "tool_calls" not in d

def test_chat_request_to_dict():
    req = ChatRequest(
        model="gpt-4o",
        messages=[ChatMessage(role=Role.USER, content="Hi")],
    )
    d = req.to_dict()
    assert d["model"] == "gpt-4o"
    assert d["stream"] is False
    assert "max_tokens" not in d

def test_chat_response_from_dict():
    data = {
        "id": "chatcmpl-123",
        "object": "chat.completion",
        "created": 1700000000,
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "Hello!"},
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
    }
    resp = ChatResponse.from_dict(data)
    assert resp.id == "chatcmpl-123"
    assert resp.choices[0].message.content == "Hello!"
    assert resp.usage.total_tokens == 15

def test_chat_chunk_from_dict():
    data = {
        "id": "chatcmpl-123",
        "object": "chat.completion.chunk",
        "created": 1700000000,
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "delta": {"content": "Hello"},
            "finish_reason": None,
        }],
    }
    chunk = ChatChunk.from_dict(data)
    assert chunk.choices[0].delta.content == "Hello"

def test_payment_required_from_dict():
    data = {
        "x402_version": 2,
        "resource": {"url": "/v1/chat/completions", "method": "POST"},
        "accepts": [{
            "scheme": "exact",
            "network": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
            "amount": "100000",
            "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            "pay_to": "RecipientWallet111111111111111111111111111",
            "max_timeout_seconds": 300,
        }],
        "cost_breakdown": {
            "provider_cost": "0.000095",
            "platform_fee": "0.000005",
            "total": "0.000100",
            "currency": "USDC",
            "fee_percent": 5,
        },
        "error": "Payment required",
    }
    pr = PaymentRequired.from_dict(data)
    assert pr.x402_version == 2
    assert pr.accepts[0].scheme == "exact"
    assert pr.cost_breakdown.total == "0.000100"

def test_constants():
    assert X402_VERSION == 2
    assert USDC_MINT == "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
    assert SOLANA_NETWORK == "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"

def test_payment_payload_to_dict():
    payload = PaymentPayload(
        x402_version=2,
        resource=Resource(url="/v1/chat/completions", method="POST"),
        accepted=PaymentAccept(
            scheme="exact",
            network=SOLANA_NETWORK,
            amount="100000",
            asset=USDC_MINT,
            pay_to="Recipient111",
            max_timeout_seconds=300,
        ),
        payload=SolanaPayload(transaction="base64tx=="),
    )
    d = payload.to_dict()
    assert d["x402_version"] == 2
    assert d["payload"]["transaction"] == "base64tx=="

def test_model_info_from_dict():
    data = {
        "id": "gpt-4o",
        "provider": "openai",
        "model_id": "gpt-4o-2024-08-06",
        "display_name": "GPT-4o",
        "input_cost_per_million": 2.5,
        "output_cost_per_million": 10.0,
        "context_window": 128000,
        "supports_streaming": True,
        "supports_tools": True,
        "supports_vision": True,
        "reasoning": False,
    }
    info = ModelInfo.from_dict(data)
    assert info.id == "gpt-4o"
    assert info.supports_streaming is True
```

**Step 2: Run tests — expect FAIL**

```bash
pytest tests/unit/test_types.py -v
# Expected: ModuleNotFoundError
```

**Step 3: Implement constants.py**

```python
# src/rustyclaw/constants.py
X402_VERSION: int = 2
USDC_MINT: str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
SOLANA_NETWORK: str = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"
MAX_TIMEOUT_SECONDS: int = 300
PLATFORM_FEE_PERCENT: int = 5
```

**Step 4: Implement types.py**

> **Alignment note:** The existing in-tree Python SDK uses `pydantic.BaseModel`. If replacing the existing SDK, either migrate to dataclasses consistently or stay with Pydantic. Document the rationale for whichever choice is made.

Use `@dataclass` for all types. Each type needs:
- `to_dict()` method that omits `None` fields (for request types)
- `from_dict(data: dict)` classmethod (for response types)
- Match JSON field names exactly (use Python snake_case internally, convert in to_dict/from_dict)

Key implementation notes:
- `Role` is a `StrEnum` (Python 3.11+) or `str, Enum` (3.10 compat)
- `PaymentPayload.payload` is a union type: `SolanaPayload | EscrowPayload`
- `ChatRequest.to_dict()` must serialize `stream` even when False (gateway expects it)
- `ChatMessage.to_dict()` omits `name`, `tool_calls`, `tool_call_id` when None
- `ModelInfo` booleans default to False, `max_output_tokens` defaults to None

```python
from __future__ import annotations
from dataclasses import dataclass, field
from enum import Enum
from typing import Any

class Role(str, Enum):
    SYSTEM = "system"
    USER = "user"
    ASSISTANT = "assistant"
    TOOL = "tool"
    DEVELOPER = "developer"

@dataclass
class ChatMessage:
    role: Role
    content: str
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
        tool_calls = None
        if "tool_calls" in data and data["tool_calls"] is not None:
            tool_calls = [ToolCall.from_dict(tc) for tc in data["tool_calls"]]
        return cls(
            role=Role(data["role"]),
            content=data.get("content", ""),
            name=data.get("name"),
            tool_calls=tool_calls,
            tool_call_id=data.get("tool_call_id"),
        )
# ... (continue for all types following same pattern)
```

Complete all types listed in the "Wire-Format Types" section above. Every type must have both `to_dict()` and `from_dict()`.

**Step 5: Run tests — expect PASS**

```bash
pytest tests/unit/test_types.py -v
```

**Step 6: Commit**

```bash
git add src/rustyclaw/types.py src/rustyclaw/constants.py tests/unit/test_types.py
git commit -m "feat: add wire-format types and constants"
```

---

### Task P3: Error Types

**Files:**
- Create: `src/rustyclaw/errors.py`
- Test: `tests/unit/test_errors.py`

**Step 1: Write failing tests**

```python
# tests/unit/test_errors.py
from rustyclaw.errors import (
    ClientError, WalletError, SignerError,
    InsufficientBalanceError, GatewayError, PaymentRejectedError,
    PaymentRequiredError, RecipientMismatchError, AmountExceedsMaxError,
    TimeoutError as RCTimeoutError,
)
from rustyclaw.types import PaymentRequired

def test_wallet_error_is_client_error():
    err = WalletError("bad mnemonic")
    assert isinstance(err, ClientError)
    assert "bad mnemonic" in str(err)

def test_signer_error_is_client_error():
    err = SignerError("rpc failed")
    assert isinstance(err, ClientError)

def test_insufficient_balance():
    err = InsufficientBalanceError(have=100, need=500)
    assert err.have == 100
    assert err.need == 500
    assert "100" in str(err) and "500" in str(err)

def test_gateway_error():
    err = GatewayError(status=500, message="internal error")
    assert err.status == 500

def test_recipient_mismatch():
    err = RecipientMismatchError(expected="A", actual="B")
    assert err.expected == "A"

def test_amount_exceeds_max():
    err = AmountExceedsMaxError(amount=1000, max_amount=500)
    assert err.amount == 1000
```

**Step 2: Run tests — expect FAIL**

**Step 3: Implement errors.py**

```python
# src/rustyclaw/errors.py
from __future__ import annotations
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from rustyclaw.types import PaymentRequired

class ClientError(Exception):
    """Base error for all RustyClaw client errors."""

class WalletError(ClientError):
    """Wallet operation failed."""

class SignerError(ClientError):
    """Payment signing failed."""

class InsufficientBalanceError(ClientError):
    def __init__(self, have: int, need: int) -> None:
        self.have = have
        self.need = need
        super().__init__(f"Insufficient balance: have {have}, need {need}")

class GatewayError(ClientError):
    def __init__(self, status: int, message: str) -> None:
        self.status = status
        self.message = message
        super().__init__(f"Gateway error {status}: {message}")

class PaymentRequiredError(ClientError):
    def __init__(self, payment_required: PaymentRequired) -> None:
        self.payment_required = payment_required
        super().__init__(f"Payment required: {payment_required.cost_breakdown.total} USDC")

class PaymentRejectedError(ClientError):
    def __init__(self, reason: str) -> None:
        self.reason = reason
        super().__init__(f"Payment rejected: {reason}")

class RecipientMismatchError(ClientError):
    def __init__(self, expected: str, actual: str) -> None:
        self.expected = expected
        self.actual = actual
        super().__init__(f"Recipient mismatch: expected {expected}, got {actual}")

class AmountExceedsMaxError(ClientError):
    def __init__(self, amount: int, max_amount: int) -> None:
        self.amount = amount
        self.max_amount = max_amount
        super().__init__(f"Amount {amount} exceeds max {max_amount}")

class TimeoutError(ClientError):
    def __init__(self, timeout_secs: float) -> None:
        self.timeout_secs = timeout_secs
        super().__init__(f"Request timed out after {timeout_secs}s")
```

**Step 4: Run tests — expect PASS**

**Step 5: Commit**

```bash
git add src/rustyclaw/errors.py tests/unit/test_errors.py
git commit -m "feat: add error type hierarchy"
```

---

### Task P4: Config & Builder

**Files:**
- Create: `src/rustyclaw/config.py`
- Test: `tests/unit/test_config.py`

**Step 1: Write failing tests**

```python
# tests/unit/test_config.py
from rustyclaw.config import ClientConfig, ClientBuilder

def test_default_config():
    cfg = ClientConfig()
    assert cfg.gateway_url == "http://localhost:8402"
    assert cfg.rpc_url == "https://api.mainnet-beta.solana.com"
    assert cfg.timeout == 180.0
    assert cfg.enable_cache is False
    assert cfg.enable_sessions is False
    assert cfg.session_ttl == 1800.0
    assert cfg.enable_quality_check is False
    assert cfg.max_quality_retries == 1
    assert cfg.free_fallback_model is None
    assert cfg.expected_recipient is None
    assert cfg.max_payment_amount is None
    assert cfg.prefer_escrow is False

def test_builder_fluent():
    cfg = (
        ClientBuilder()
        .gateway_url("https://example.com")
        .rpc_url("https://rpc.example.com")
        .timeout(60.0)
        .enable_cache(True)
        .enable_sessions(True)
        .session_ttl(600.0)
        .enable_quality_check(True)
        .max_quality_retries(3)
        .free_fallback_model("gpt-4o-mini")
        .expected_recipient("Wallet111")
        .max_payment_amount(1_000_000)
        .build()
    )
    assert cfg.gateway_url == "https://example.com"
    assert cfg.timeout == 60.0
    assert cfg.enable_cache is True
    assert cfg.max_quality_retries == 3
    assert cfg.free_fallback_model == "gpt-4o-mini"
    assert cfg.max_payment_amount == 1_000_000

def test_builder_default_matches_config_default():
    builder_cfg = ClientBuilder().build()
    default_cfg = ClientConfig()
    assert builder_cfg.gateway_url == default_cfg.gateway_url
    assert builder_cfg.timeout == default_cfg.timeout
    assert builder_cfg.enable_cache == default_cfg.enable_cache
```

**Step 2: Run tests — expect FAIL**

**Step 3: Implement config.py**

```python
from __future__ import annotations
from dataclasses import dataclass

@dataclass
class ClientConfig:
    gateway_url: str = "http://localhost:8402"
    rpc_url: str = "https://api.mainnet-beta.solana.com"
    prefer_escrow: bool = False
    timeout: float = 180.0
    expected_recipient: str | None = None
    max_payment_amount: int | None = None
    enable_cache: bool = False
    enable_sessions: bool = False
    session_ttl: float = 1800.0
    enable_quality_check: bool = False
    max_quality_retries: int = 1
    free_fallback_model: str | None = None

class ClientBuilder:
    def __init__(self) -> None:
        self._config = ClientConfig()

    def gateway_url(self, url: str) -> ClientBuilder:
        self._config.gateway_url = url
        return self

    def rpc_url(self, url: str) -> ClientBuilder:
        self._config.rpc_url = url
        return self

    def prefer_escrow(self, prefer: bool) -> ClientBuilder:
        self._config.prefer_escrow = prefer
        return self

    def timeout(self, timeout: float) -> ClientBuilder:
        self._config.timeout = timeout
        return self

    def expected_recipient(self, recipient: str) -> ClientBuilder:
        self._config.expected_recipient = recipient
        return self

    def max_payment_amount(self, max_amount: int) -> ClientBuilder:
        self._config.max_payment_amount = max_amount
        return self

    def enable_cache(self, enable: bool) -> ClientBuilder:
        self._config.enable_cache = enable
        return self

    def enable_sessions(self, enable: bool) -> ClientBuilder:
        self._config.enable_sessions = enable
        return self

    def session_ttl(self, ttl: float) -> ClientBuilder:
        self._config.session_ttl = ttl
        return self

    def enable_quality_check(self, enable: bool) -> ClientBuilder:
        self._config.enable_quality_check = enable
        return self

    def max_quality_retries(self, max_retries: int) -> ClientBuilder:
        self._config.max_quality_retries = max_retries
        return self

    def free_fallback_model(self, model: str) -> ClientBuilder:
        self._config.free_fallback_model = model
        return self

    def build(self) -> ClientConfig:
        return self._config
```

> **Convention:** `build()` must return a frozen copy of the config (e.g., via `dataclasses.replace()`), not the mutable internal reference. Per project immutability conventions.

**Step 4: Run tests — expect PASS**

**Step 5: Commit**

```bash
git add src/rustyclaw/config.py tests/unit/test_config.py
git commit -m "feat: add ClientConfig and ClientBuilder"
```

---

### Task P5: Wallet

**Files:**
- Create: `src/rustyclaw/wallet.py`
- Test: `tests/unit/test_wallet.py`

**Step 1: Write failing tests**

```python
# tests/unit/test_wallet.py
import os
from rustyclaw.wallet import Wallet
from rustyclaw.errors import WalletError

def test_create_returns_wallet_and_mnemonic():
    wallet, mnemonic = Wallet.create()
    assert wallet.address()  # non-empty base58 string
    assert len(mnemonic.split()) == 12

def test_from_mnemonic_roundtrip():
    wallet1, mnemonic = Wallet.create()
    wallet2 = Wallet.from_mnemonic(mnemonic)
    assert wallet1.address() == wallet2.address()

def test_from_mnemonic_invalid():
    try:
        Wallet.from_mnemonic("invalid words here")
        assert False, "Should raise WalletError"
    except WalletError:
        pass

def test_from_keypair_bytes():
    wallet1, _ = Wallet.create()
    raw = wallet1.to_keypair_bytes()
    wallet2 = Wallet.from_keypair_bytes(raw)
    assert wallet1.address() == wallet2.address()

def test_from_env(monkeypatch):
    wallet1, _ = Wallet.create()
    b58 = wallet1.to_keypair_b58()
    monkeypatch.setenv("TEST_WALLET_KEY", b58)
    wallet2 = Wallet.from_env("TEST_WALLET_KEY")
    assert wallet1.address() == wallet2.address()

def test_from_env_missing():
    try:
        Wallet.from_env("NONEXISTENT_VAR_12345")
        assert False, "Should raise WalletError"
    except WalletError:
        pass

def test_debug_redacts_secrets():
    wallet, _ = Wallet.create()
    debug_str = repr(wallet)
    # Should not contain the full keypair
    assert wallet.to_keypair_b58() not in debug_str
```

**Step 2: Run tests — expect FAIL**

**Step 3: Implement wallet.py**

Use `solders` for Solana keypair management:
- `solders.keypair.Keypair` for key generation
- `solders.pubkey.Pubkey` for address
- BIP39 mnemonic via `mnemonic` package or `solders` if supported
- Implement `__repr__` to redact secrets
- Implement `__del__` for best-effort key zeroization

```python
from __future__ import annotations
import os
from solders.keypair import Keypair
from solders.pubkey import Pubkey
from rustyclaw.errors import WalletError

class Wallet:
    def __init__(self, keypair: Keypair) -> None:
        self._keypair = keypair

    @classmethod
    def create(cls) -> tuple[Wallet, str]:
        # Generate keypair and derive mnemonic
        # Use solders or bip39 package
        ...

    @classmethod
    def from_mnemonic(cls, phrase: str) -> Wallet:
        ...

    @classmethod
    def from_keypair_bytes(cls, raw: bytes) -> Wallet:
        try:
            kp = Keypair.from_bytes(raw)
            return cls(kp)
        except Exception as e:
            raise WalletError(f"Invalid keypair bytes: {e}") from e

    @classmethod
    def from_keypair_b58(cls, b58: str) -> Wallet:
        ...

    @classmethod
    def from_env(cls, var: str) -> Wallet:
        value = os.environ.get(var)
        if value is None:
            raise WalletError(f"Environment variable {var} not set")
        return cls.from_keypair_b58(value)

    def address(self) -> str:
        return str(self._keypair.pubkey())

    def pubkey(self) -> Pubkey:
        return self._keypair.pubkey()

    def to_keypair_bytes(self) -> bytes:
        return bytes(self._keypair)

    def to_keypair_b58(self) -> str:
        import base58
        return base58.b58encode(bytes(self._keypair)).decode()

    def __repr__(self) -> str:
        return f"Wallet(address={self.address()}, secret=REDACTED)"
```

Note: BIP39 mnemonic generation in Python may need the `mnemonic` package added to dependencies. Alternatively, generate keypair directly and skip mnemonic for MVP (document this decision).

**Step 4: Run tests — expect PASS**

**Step 5: Commit**

```bash
git add src/rustyclaw/wallet.py tests/unit/test_wallet.py
git commit -m "feat: add Wallet with keypair management"
```

---

### Task P6: Cache

**Files:**
- Create: `src/rustyclaw/cache.py`
- Test: `tests/unit/test_cache.py`

**Step 1: Write failing tests**

```python
# tests/unit/test_cache.py
import time
import hashlib
from rustyclaw.cache import ResponseCache
from rustyclaw.types import (
    ChatMessage, ChatResponse, ChatChoice, Usage, Role,
)

def make_response(content: str) -> ChatResponse:
    return ChatResponse(
        id="test-id",
        object="chat.completion",
        created=0,
        model="test-model",
        choices=[ChatChoice(
            index=0,
            message=ChatMessage(role=Role.ASSISTANT, content=content),
            finish_reason="stop",
        )],
        usage=Usage(prompt_tokens=10, completion_tokens=5, total_tokens=15),
    )

def make_messages(content: str) -> list[ChatMessage]:
    return [ChatMessage(role=Role.USER, content=content)]

def test_cache_miss():
    cache = ResponseCache()
    assert cache.get(12345) is None

def test_cache_hit():
    cache = ResponseCache()
    resp = make_response("hello")
    key = ResponseCache.cache_key("model-a", make_messages("hi"))
    cache.put(key, resp)
    cached = cache.get(key)
    assert cached is not None
    assert cached.choices[0].message.content == "hello"

def test_ttl_expiry():
    cache = ResponseCache(max_entries=10, ttl=0.001, dedup_window=0)
    key = 42
    cache.put(key, make_response("ephemeral"))
    time.sleep(0.01)
    assert cache.get(key) is None

def test_lru_eviction():
    cache = ResponseCache(max_entries=3, ttl=60, dedup_window=0)
    cache.put(1, make_response("a"))
    cache.put(2, make_response("b"))
    cache.put(3, make_response("c"))
    cache.put(4, make_response("d"))
    assert cache.get(1) is None  # evicted
    assert cache.get(4) is not None

def test_dedup_window():
    cache = ResponseCache(max_entries=10, ttl=60, dedup_window=60)
    cache.put(99, make_response("first"))
    cache.put(99, make_response("second"))
    cached = cache.get(99)
    assert cached.choices[0].message.content == "first"  # not overwritten

def test_cache_key_deterministic():
    msgs = make_messages("hello world")
    k1 = ResponseCache.cache_key("model-x", msgs)
    k2 = ResponseCache.cache_key("model-x", msgs)
    assert k1 == k2

def test_cache_key_differs_for_different_models():
    msgs = make_messages("hello world")
    k1 = ResponseCache.cache_key("model-a", msgs)
    k2 = ResponseCache.cache_key("model-b", msgs)
    assert k1 != k2
```

**Step 2: Run tests — expect FAIL**

**Step 3: Implement cache.py**

Use `collections.OrderedDict` for LRU (or a simple dict with max-size eviction). Thread-safe via `threading.Lock`.

```python
from __future__ import annotations
import hashlib
import threading
import time
from collections import OrderedDict
from rustyclaw.types import ChatMessage, ChatResponse

_DEFAULT_MAX_ENTRIES = 200
_DEFAULT_TTL = 600.0
_DEFAULT_DEDUP_WINDOW = 30.0

class ResponseCache:
    def __init__(
        self,
        max_entries: int = _DEFAULT_MAX_ENTRIES,
        ttl: float = _DEFAULT_TTL,
        dedup_window: float = _DEFAULT_DEDUP_WINDOW,
    ) -> None:
        self._max_entries = max_entries
        self._ttl = ttl
        self._dedup_window = dedup_window
        self._lock = threading.Lock()
        self._entries: OrderedDict[int, tuple[ChatResponse, float]] = OrderedDict()

    @staticmethod
    def cache_key(model: str, messages: list[ChatMessage]) -> int:
        h = hashlib.sha256()
        h.update(model.encode())
        for msg in messages:
            h.update(msg.role.value.encode())
            h.update(msg.content.encode())
        return int.from_bytes(h.digest()[:8], "big")

    def get(self, key: int) -> ChatResponse | None:
        with self._lock:
            entry = self._entries.get(key)
            if entry is None:
                return None
            resp, inserted = entry
            if time.monotonic() - inserted > self._ttl:
                del self._entries[key]
                return None
            self._entries.move_to_end(key)
            return resp

    def put(self, key: int, response: ChatResponse) -> None:
        with self._lock:
            if key in self._entries:
                _, inserted = self._entries[key]
                if time.monotonic() - inserted < self._dedup_window:
                    return
            self._entries[key] = (response, time.monotonic())
            self._entries.move_to_end(key)
            while len(self._entries) > self._max_entries:
                self._entries.popitem(last=False)
```

**Step 4: Run tests — expect PASS**

**Step 5: Commit**

```bash
git add src/rustyclaw/cache.py tests/unit/test_cache.py
git commit -m "feat: add LRU response cache with TTL and dedup"
```

---

### Task P7: Session Store

**Files:**
- Create: `src/rustyclaw/session.py`
- Test: `tests/unit/test_session.py`

**Step 1: Write failing tests**

```python
# tests/unit/test_session.py
import asyncio
import time
from rustyclaw.session import SessionStore, SessionInfo
from rustyclaw.types import ChatMessage, Role

def make_messages(content: str) -> list[ChatMessage]:
    return [ChatMessage(role=Role.USER, content=content)]

def test_new_session_returns_default_model():
    store = SessionStore(ttl=60)
    info = store.get_or_create("sess-1", "gpt-4o")
    assert info.model == "gpt-4o"
    assert info.escalated is False

def test_existing_session_returns_stored_model():
    store = SessionStore(ttl=60)
    store.get_or_create("sess-1", "gpt-4o")
    info = store.get_or_create("sess-1", "claude-sonnet")
    assert info.model == "gpt-4o"  # original, not new default

def test_expired_session_creates_new():
    store = SessionStore(ttl=0.001)
    store.get_or_create("sess-1", "gpt-4o")
    time.sleep(0.01)
    info = store.get_or_create("sess-1", "claude-sonnet")
    assert info.model == "claude-sonnet"

def test_three_strike_sets_escalated():
    store = SessionStore(ttl=60)
    store.get_or_create("sess-1", "gpt-4o")
    store.record_request("sess-1", 42)
    store.record_request("sess-1", 42)
    store.record_request("sess-1", 42)
    info = store.get_or_create("sess-1", "gpt-4o")
    assert info.escalated is True

def test_less_than_three_does_not_escalate():
    store = SessionStore(ttl=60)
    store.get_or_create("sess-1", "gpt-4o")
    store.record_request("sess-1", 42)
    store.record_request("sess-1", 42)
    store.record_request("sess-1", 99)
    info = store.get_or_create("sess-1", "gpt-4o")
    assert info.escalated is False

def test_derive_session_id_deterministic():
    msgs = make_messages("Hello, world!")
    id1 = SessionStore.derive_session_id(msgs)
    id2 = SessionStore.derive_session_id(msgs)
    assert id1 == id2

def test_derive_session_id_differs():
    id_a = SessionStore.derive_session_id(make_messages("Hello"))
    id_b = SessionStore.derive_session_id(make_messages("Goodbye"))
    assert id_a != id_b

def test_cleanup_expired():
    store = SessionStore(ttl=0.001)
    store.get_or_create("sess-1", "gpt-4o")
    store.get_or_create("sess-2", "gpt-4o")
    time.sleep(0.01)
    store.get_or_create("sess-3", "claude-sonnet")
    store.cleanup_expired()
    # sess-3 should survive, sess-1 and sess-2 should be gone
    info = store.get_or_create("sess-3", "different")
    assert info.model == "claude-sonnet"  # survived cleanup
```

**Step 2: Run tests — expect FAIL**

**Step 3: Implement session.py**

Synchronous (thread-safe via `threading.Lock`). Mirrors Rust's `SessionStore` exactly.

```python
from __future__ import annotations
import hashlib
import threading
import time
from collections import deque
from dataclasses import dataclass
from rustyclaw.types import ChatMessage

_MAX_RECENT_HASHES = 10
_THREE_STRIKE_THRESHOLD = 3

@dataclass
class SessionInfo:
    model: str
    escalated: bool

class SessionStore:
    def __init__(self, ttl: float = 1800.0) -> None:
        self._ttl = ttl
        self._lock = threading.Lock()
        self._sessions: dict[str, _SessionEntry] = {}

    def get_or_create(self, session_id: str, default_model: str) -> SessionInfo:
        with self._lock:
            entry = self._sessions.get(session_id)
            if entry is not None and (time.monotonic() - entry.created) < self._ttl:
                return SessionInfo(model=entry.model, escalated=entry.escalated)
            new_entry = _SessionEntry(model=default_model, created=time.monotonic())
            self._sessions[session_id] = new_entry
            return SessionInfo(model=default_model, escalated=False)

    def record_request(self, session_id: str, request_hash: int) -> None:
        with self._lock:
            entry = self._sessions.get(session_id)
            if entry is None:
                return
            entry.request_count += 1
            if len(entry.recent_hashes) >= _MAX_RECENT_HASHES:
                entry.recent_hashes.popleft()
            entry.recent_hashes.append(request_hash)
            if not entry.escalated:
                counts: dict[int, int] = {}
                for h in entry.recent_hashes:
                    counts[h] = counts.get(h, 0) + 1
                    if counts[h] >= _THREE_STRIKE_THRESHOLD:
                        entry.escalated = True
                        break

    def cleanup_expired(self) -> None:
        with self._lock:
            now = time.monotonic()
            expired = [k for k, v in self._sessions.items() if now - v.created >= self._ttl]
            for k in expired:
                del self._sessions[k]

    @staticmethod
    def derive_session_id(messages: list[ChatMessage]) -> str:
        h = hashlib.sha256()
        if messages:
            h.update(messages[0].content.encode())
        return h.hexdigest()[:16]

class _SessionEntry:
    def __init__(self, model: str, created: float) -> None:
        self.model = model
        self.created = created
        self.request_count = 0
        self.recent_hashes: deque[int] = deque()
        self.escalated = False
```

**Step 4: Run tests — expect PASS**

**Step 5: Commit**

```bash
git add src/rustyclaw/session.py tests/unit/test_session.py
git commit -m "feat: add session store with three-strike escalation"
```

---

### Task P8: Quality Check

**Files:**
- Create: `src/rustyclaw/quality.py`
- Test: `tests/unit/test_quality.py`

**Step 1: Write failing tests**

```python
# tests/unit/test_quality.py
from rustyclaw.quality import check_degraded, DegradedReason

def test_empty_content_is_degraded():
    reason = check_degraded("")
    assert reason == DegradedReason.EMPTY_CONTENT

def test_whitespace_only_is_degraded():
    reason = check_degraded("   \n\t  ")
    assert reason == DegradedReason.EMPTY_CONTENT

def test_known_error_phrase():
    reason = check_degraded("I cannot help with that request.")
    assert reason == DegradedReason.KNOWN_ERROR_PHRASE

def test_case_insensitive_error_phrase():
    reason = check_degraded("As An AI language model, I must say...")
    assert reason == DegradedReason.KNOWN_ERROR_PHRASE

def test_im_sorry_phrase():
    reason = check_degraded("I'm sorry, but I can't do that.")
    assert reason == DegradedReason.KNOWN_ERROR_PHRASE

def test_repetitive_loop():
    phrase = "the quick brown "
    content = phrase * 20  # 3-word phrase repeated many times
    reason = check_degraded(content)
    assert reason == DegradedReason.REPETITIVE_LOOP

def test_truncated_mid_word():
    content = "A" * 101 + "truncat"  # ends with alphanumeric, >100 chars
    reason = check_degraded(content)
    assert reason == DegradedReason.TRUNCATED_MID_WORD

def test_normal_content_not_degraded():
    reason = check_degraded("The capital of France is Paris.")
    assert reason is None

def test_content_ending_with_period_not_truncated():
    content = "A" * 101 + "."
    reason = check_degraded(content)
    assert reason is None
```

**Step 2: Run tests — expect FAIL**

**Step 3: Implement quality.py**

```python
from __future__ import annotations
import re
from enum import Enum

class DegradedReason(str, Enum):
    EMPTY_CONTENT = "empty_content"
    KNOWN_ERROR_PHRASE = "known_error_phrase"
    REPETITIVE_LOOP = "repetitive_loop"
    TRUNCATED_MID_WORD = "truncated_mid_word"

_ERROR_PHRASES = [
    "i cannot",
    "as an ai",
    "i'm sorry, but i",
]

def check_degraded(content: str) -> DegradedReason | None:
    if not content or not content.strip():
        return DegradedReason.EMPTY_CONTENT

    lower = content.lower()
    for phrase in _ERROR_PHRASES:
        if phrase in lower:
            return DegradedReason.KNOWN_ERROR_PHRASE

    # Check for repetitive 3-word phrases
    words = content.split()
    if len(words) >= 15:  # need enough words to detect repetition
        trigram_counts: dict[str, int] = {}
        for i in range(len(words) - 2):
            trigram = f"{words[i]} {words[i+1]} {words[i+2]}"
            trigram_counts[trigram] = trigram_counts.get(trigram, 0) + 1
            if trigram_counts[trigram] >= 5:
                return DegradedReason.REPETITIVE_LOOP

    # Check for truncation
    if len(content) > 100 and content[-1].isalnum():
        return DegradedReason.TRUNCATED_MID_WORD

    return None
```

**Step 4: Run tests — expect PASS**

**Step 5: Commit**

```bash
git add src/rustyclaw/quality.py tests/unit/test_quality.py
git commit -m "feat: add degraded response detection heuristics"
```

---

### Task P9: Transport (HTTP + SSE)

**Files:**
- Create: `src/rustyclaw/transport.py`
- Test: `tests/unit/test_transport.py`
- Test: `tests/integration/test_transport.py`

**Step 1: Write failing tests**

```python
# tests/unit/test_transport.py
import pytest
from rustyclaw.transport import Transport
from rustyclaw.types import ChatRequest, ChatMessage, ChatResponse, Role
from rustyclaw.errors import GatewayError, TimeoutError

@pytest.fixture
def transport():
    return Transport(base_url="http://localhost:8402", timeout=10.0)

def test_transport_builds_url(transport):
    url = transport._build_url("/v1/chat/completions")
    assert url == "http://localhost:8402/v1/chat/completions"

def test_transport_builds_headers_without_payment(transport):
    headers = transport._build_headers(payment_signature=None)
    assert "Content-Type" in headers
    assert "Payment-Signature" not in headers

def test_transport_builds_headers_with_payment(transport):
    headers = transport._build_headers(payment_signature="base64sig")
    assert headers["Payment-Signature"] == "base64sig"
```

```python
# tests/integration/test_transport.py
import json
import pytest
import httpx
from pytest_httpx import HTTPXMock
from rustyclaw.transport import Transport
from rustyclaw.types import ChatRequest, ChatMessage, ChatResponse, Role
from rustyclaw.errors import GatewayError

@pytest.fixture
def transport():
    return Transport(base_url="http://test-gateway:8402", timeout=10.0)

@pytest.mark.asyncio
async def test_send_chat_success(transport, httpx_mock: HTTPXMock):
    httpx_mock.add_response(
        url="http://test-gateway:8402/v1/chat/completions",
        json={
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "Hi!"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12},
        },
    )
    req = ChatRequest(model="gpt-4o", messages=[ChatMessage(role=Role.USER, content="Hello")])
    resp = await transport.send_chat(req)
    assert resp.choices[0].message.content == "Hi!"

@pytest.mark.asyncio
async def test_send_chat_402(transport, httpx_mock: HTTPXMock):
    httpx_mock.add_response(
        url="http://test-gateway:8402/v1/chat/completions",
        status_code=402,
        json={
            "x402_version": 2,
            "resource": {"url": "/v1/chat/completions", "method": "POST"},
            "accepts": [{"scheme": "exact", "network": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp", "amount": "100000", "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", "pay_to": "Recipient111", "max_timeout_seconds": 300}],
            "cost_breakdown": {"provider_cost": "0.000095", "platform_fee": "0.000005", "total": "0.000100", "currency": "USDC", "fee_percent": 5},
            "error": "Payment required",
        },
    )
    req = ChatRequest(model="gpt-4o", messages=[ChatMessage(role=Role.USER, content="Hello")])
    result = await transport.send_chat(req)
    # Transport returns a PaymentRequired on 402, not an error
    from rustyclaw.types import PaymentRequired
    assert isinstance(result, PaymentRequired)

@pytest.mark.asyncio
async def test_send_chat_500(transport, httpx_mock: HTTPXMock):
    httpx_mock.add_response(
        url="http://test-gateway:8402/v1/chat/completions",
        status_code=500,
        json={"error": "internal server error"},
    )
    req = ChatRequest(model="gpt-4o", messages=[ChatMessage(role=Role.USER, content="Hello")])
    with pytest.raises(GatewayError) as exc_info:
        await transport.send_chat(req)
    assert exc_info.value.status == 500
```

**Step 2: Run tests — expect FAIL**

**Step 3: Implement transport.py**

```python
from __future__ import annotations
import base64
import json
from typing import AsyncIterator

import httpx

from rustyclaw.errors import GatewayError, TimeoutError
from rustyclaw.types import (
    ChatChunk, ChatRequest, ChatResponse, PaymentRequired,
)

class Transport:
    def __init__(self, base_url: str, timeout: float = 180.0) -> None:
        self._base_url = base_url.rstrip("/")
        self._timeout = timeout

    def _build_url(self, path: str) -> str:
        return f"{self._base_url}{path}"

    def _build_headers(
        self,
        payment_signature: str | None = None,
        extra_headers: dict[str, str] | None = None,
    ) -> dict[str, str]:
        headers = {"Content-Type": "application/json"}
        if payment_signature is not None:
            headers["Payment-Signature"] = payment_signature
        if extra_headers:
            headers.update(extra_headers)
        return headers

    async def send_chat(
        self,
        request: ChatRequest,
        payment_signature: str | None = None,
        extra_headers: dict[str, str] | None = None,
    ) -> ChatResponse | PaymentRequired:
        url = self._build_url("/v1/chat/completions")
        headers = self._build_headers(payment_signature, extra_headers)
        body = request.to_dict()
        body["stream"] = False

        async with httpx.AsyncClient(timeout=self._timeout) as client:
            try:
                resp = await client.post(url, json=body, headers=headers)
            except httpx.TimeoutException:
                raise TimeoutError(self._timeout)

            if resp.status_code == 200:
                return ChatResponse.from_dict(resp.json())
            elif resp.status_code == 402:
                return PaymentRequired.from_dict(resp.json())
            else:
                data = resp.json() if resp.headers.get("content-type", "").startswith("application/json") else {}
                raise GatewayError(
                    status=resp.status_code,
                    message=data.get("error", resp.text),
                )

    async def send_chat_stream(
        self,
        request: ChatRequest,
        payment_signature: str | None = None,
        extra_headers: dict[str, str] | None = None,
    ) -> AsyncGenerator[ChatChunk, None]:
        url = self._build_url("/v1/chat/completions")
        headers = self._build_headers(payment_signature, extra_headers)
        body = request.to_dict()
        body["stream"] = True

        async with httpx.AsyncClient(timeout=self._timeout) as client:
            async with client.stream("POST", url, json=body, headers=headers) as resp:
                if resp.status_code == 402:
                    data = json.loads(await resp.aread())
                    raise PaymentRequiredError(PaymentRequired.from_dict(data))
                if resp.status_code != 200:
                    data = json.loads(await resp.aread())
                    raise GatewayError(status=resp.status_code, message=data.get("error", ""))

                async for line in resp.aiter_lines():
                    if line.startswith("data: "):
                        data_str = line[6:]
                        if data_str.strip() == "[DONE]":
                            break
                        yield ChatChunk.from_dict(json.loads(data_str))

    async def fetch_models(self) -> list[dict]:
        url = self._build_url("/v1/models")
        async with httpx.AsyncClient(timeout=self._timeout) as client:
            resp = await client.get(url)
            if resp.status_code != 200:
                raise GatewayError(status=resp.status_code, message=resp.text)
            return resp.json().get("data", [])
```

Note: `send_chat_stream` is an `AsyncGenerator[ChatChunk, None]` — it uses `yield`. For 402 responses, it raises `PaymentRequiredError` rather than returning `PaymentRequired`, which keeps the generator's yield type clean and matches the Rust client's error-propagation behavior.

**Step 4: Run tests — expect PASS**

**Step 5: Commit**

```bash
git add src/rustyclaw/transport.py tests/unit/test_transport.py tests/integration/test_transport.py
git commit -m "feat: add HTTP transport with SSE streaming"
```

---

### Task P10: Signer

**Files:**
- Create: `src/rustyclaw/signer.py`
- Test: `tests/unit/test_signer.py`

**Step 1: Write failing tests**

```python
# tests/unit/test_signer.py
from rustyclaw.signer import Signer, KeypairSigner
from rustyclaw.wallet import Wallet
from rustyclaw.types import PaymentPayload

def test_signer_is_abstract():
    """Signer is a protocol/ABC — can't instantiate directly."""
    try:
        Signer()
        assert False, "Should not instantiate"
    except TypeError:
        pass

def test_keypair_signer_implements_signer():
    wallet, _ = Wallet.create()
    signer = KeypairSigner(wallet)
    assert isinstance(signer, Signer)
```

Note: Full signer tests need RPC interaction (building real transactions). Integration tests for signing will be in `tests/integration/test_signer.py` using mocked RPC responses. The unit tests verify the interface contract only.

**Step 2: Run tests — expect FAIL**

**Step 3: Implement signer.py**

```python
from __future__ import annotations
from abc import ABC, abstractmethod

from rustyclaw.errors import SignerError
from rustyclaw.types import PaymentAccept, PaymentPayload, Resource, SolanaPayload
from rustyclaw.wallet import Wallet
from rustyclaw.constants import USDC_MINT

class Signer(ABC):
    @abstractmethod
    def sign_payment(
        self,
        amount_atomic: int,
        recipient: str,
        resource: Resource,
        accepted: PaymentAccept,
    ) -> PaymentPayload:
        ...

class KeypairSigner(Signer):
    def __init__(self, wallet: Wallet, rpc_url: str = "https://api.mainnet-beta.solana.com") -> None:
        self._wallet = wallet
        self._rpc_url = rpc_url

    def sign_payment(
        self,
        amount_atomic: int,
        recipient: str,
        resource: Resource,
        accepted: PaymentAccept,
    ) -> PaymentPayload:
        try:
            from solders.keypair import Keypair
            from solders.pubkey import Pubkey
            from solders.transaction import Transaction
            from solders.system_program import TransferParams, transfer
            import httpx
            import base64

            # 1. Get recent blockhash
            resp = httpx.post(self._rpc_url, json={
                "jsonrpc": "2.0", "id": 1, "method": "getLatestBlockhash",
                "params": [{"commitment": "finalized"}],
            })
            blockhash = resp.json()["result"]["value"]["blockhash"]

            # 2. Build SPL token transfer instruction
            # Use solders/spl-token for USDC transfer
            # This is simplified — real impl needs ATA derivation

            # 3. Sign and serialize
            # tx = Transaction.new_signed_with_payer(...)
            # tx_bytes = bytes(tx)
            # tx_b64 = base64.b64encode(tx_bytes).decode()

            # 4. Return payload
            # return PaymentPayload(
            #     x402_version=2,
            #     resource=resource,
            #     accepted=accepted,
            #     payload=SolanaPayload(transaction=tx_b64),
            # )

            raise SignerError("Full SPL token signing implementation needed — see solders docs")
        except SignerError:
            raise
        except Exception as e:
            raise SignerError(f"Failed to sign payment: {e}") from e
```

Note: Full SPL token transfer signing with `solders` requires careful ATA derivation and instruction building. The implementer should reference:
- `solders` docs for transaction building
- The Rust client's `signer.rs` for the exact instruction sequence
- SPL Token program ID: `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`

**Step 4: Run tests — expect PASS**

**Step 5: Commit**

```bash
git add src/rustyclaw/signer.py tests/unit/test_signer.py
git commit -m "feat: add pluggable signer interface with KeypairSigner"
```

---

### Task P11: Balance Monitor

**Files:**
- Create: `src/rustyclaw/balance.py`
- Test: `tests/unit/test_balance.py`

**Step 1: Write failing tests**

```python
# tests/unit/test_balance.py
import asyncio
import pytest
from unittest.mock import AsyncMock
from rustyclaw.balance import BalanceMonitor

@pytest.mark.asyncio
async def test_balance_monitor_polls():
    """Monitor should call the balance fetcher on each tick."""
    fetch_count = 0
    async def mock_fetch() -> float:
        nonlocal fetch_count
        fetch_count += 1
        return 10.0

    monitor = BalanceMonitor(
        fetch_balance=mock_fetch,
        poll_interval=0.01,
    )
    monitor.start()
    await asyncio.sleep(0.05)
    monitor.stop()
    assert fetch_count >= 2

@pytest.mark.asyncio
async def test_balance_monitor_updates_state():
    async def mock_fetch() -> float:
        return 5.5

    monitor = BalanceMonitor(fetch_balance=mock_fetch, poll_interval=0.01)
    monitor.start()
    await asyncio.sleep(0.05)
    monitor.stop()
    assert monitor.last_known_balance() == 5.5

@pytest.mark.asyncio
async def test_low_balance_callback_fires_on_transition():
    """Callback fires once when crossing threshold, not every tick."""
    callback_count = 0
    def on_low(balance: float) -> None:
        nonlocal callback_count
        callback_count += 1

    async def mock_fetch() -> float:
        return 0.5  # below threshold

    monitor = BalanceMonitor(
        fetch_balance=mock_fetch,
        poll_interval=0.01,
        low_balance_threshold=1.0,
        on_low_balance=on_low,
    )
    monitor.start()
    await asyncio.sleep(0.05)
    monitor.stop()
    assert callback_count == 1  # only once, not every tick

@pytest.mark.asyncio
async def test_stop_is_idempotent():
    async def mock_fetch() -> float:
        return 10.0

    monitor = BalanceMonitor(fetch_balance=mock_fetch, poll_interval=0.01)
    monitor.start()
    monitor.stop()
    monitor.stop()  # should not raise
```

**Step 2: Run tests — expect FAIL**

**Step 3: Implement balance.py**

```python
from __future__ import annotations
import asyncio
from typing import Callable, Awaitable

class BalanceMonitor:
    def __init__(
        self,
        fetch_balance: Callable[[], Awaitable[float]],
        poll_interval: float = 30.0,
        low_balance_threshold: float | None = None,
        on_low_balance: Callable[[float], None] | None = None,
    ) -> None:
        self._fetch_balance = fetch_balance
        self._poll_interval = poll_interval
        self._threshold = low_balance_threshold
        self._on_low_balance = on_low_balance
        self._balance: float | None = None
        self._was_low = False
        self._task: asyncio.Task[None] | None = None
        self._stopped = False

    def start(self) -> None:
        self._stopped = False
        self._task = asyncio.create_task(self._run())

    def stop(self) -> None:
        self._stopped = True
        if self._task is not None and not self._task.done():
            self._task.cancel()
            self._task = None

    def last_known_balance(self) -> float | None:
        return self._balance

    async def _run(self) -> None:
        while not self._stopped:
            try:
                balance = await self._fetch_balance()
                self._balance = balance

                if self._threshold is not None and self._on_low_balance is not None:
                    is_low = balance < self._threshold
                    if is_low and not self._was_low:
                        self._on_low_balance(balance)
                    self._was_low = is_low

            except Exception:
                pass  # swallow errors, keep polling

            await asyncio.sleep(self._poll_interval)
```

**Step 4: Run tests — expect PASS**

**Step 5: Commit**

```bash
git add src/rustyclaw/balance.py tests/unit/test_balance.py
git commit -m "feat: add balance monitor with transition-debounced callback"
```

---

### Task P12: Client (Smart Chat Flow)

**Files:**
- Create: `src/rustyclaw/client.py`
- Test: `tests/unit/test_client.py`
- Test: `tests/integration/test_client.py`

This is the largest task. The client wires together all modules.

**Step 1: Write failing tests**

```python
# tests/unit/test_client.py
from rustyclaw.client import Solvela Client
from rustyclaw.config import ClientConfig
from rustyclaw.wallet import Wallet

def test_client_creation():
    wallet, _ = Wallet.create()
    config = ClientConfig()
    client = Solvela Client(wallet=wallet, config=config)
    assert client is not None

def test_client_without_wallet():
    config = ClientConfig()
    client = Solvela Client(config=config)
    assert client is not None

def test_client_last_known_balance_initially_none():
    config = ClientConfig()
    client = Solvela Client(config=config)
    assert client.last_known_balance() is None

def test_client_debug_redacts():
    wallet, _ = Wallet.create()
    config = ClientConfig()
    client = Solvela Client(wallet=wallet, config=config)
    debug = repr(client)
    assert wallet.to_keypair_b58() not in debug
```

```python
# tests/integration/test_client.py
import pytest
from pytest_httpx import HTTPXMock
from rustyclaw.client import Solvela Client
from rustyclaw.config import ClientConfig
from rustyclaw.types import ChatMessage, ChatRequest, Role

@pytest.fixture
def config():
    return ClientConfig(gateway_url="http://test-gateway:8402")

@pytest.fixture
def client(config):
    return Solvela Client(config=config)

@pytest.mark.asyncio
async def test_chat_success(client, httpx_mock: HTTPXMock):
    httpx_mock.add_response(
        url="http://test-gateway:8402/v1/chat/completions",
        json={
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "Hello!"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12},
        },
    )
    req = ChatRequest(model="gpt-4o", messages=[ChatMessage(role=Role.USER, content="Hi")])
    resp = await client.chat(req)
    assert resp.choices[0].message.content == "Hello!"

@pytest.mark.asyncio
async def test_chat_with_cache(httpx_mock: HTTPXMock):
    config = ClientConfig(gateway_url="http://test-gateway:8402", enable_cache=True)
    client = Solvela Client(config=config)

    httpx_mock.add_response(
        url="http://test-gateway:8402/v1/chat/completions",
        json={
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "Cached!"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12},
        },
    )
    req = ChatRequest(model="gpt-4o", messages=[ChatMessage(role=Role.USER, content="Hi")])

    resp1 = await client.chat(req)
    resp2 = await client.chat(req)  # should come from cache

    assert resp1.choices[0].message.content == "Cached!"
    assert resp2.choices[0].message.content == "Cached!"
    # Only one HTTP request should have been made
    assert len(httpx_mock.get_requests()) == 1

@pytest.mark.asyncio
async def test_chat_quality_retry(httpx_mock: HTTPXMock):
    config = ClientConfig(
        gateway_url="http://test-gateway:8402",
        enable_quality_check=True,
        max_quality_retries=1,
    )
    client = Solvela Client(config=config)

    # First response is degraded (empty), second is good
    httpx_mock.add_response(json={
        "id": "1", "object": "chat.completion", "created": 0, "model": "gpt-4o",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": ""}, "finish_reason": "stop"}],
    })
    httpx_mock.add_response(json={
        "id": "2", "object": "chat.completion", "created": 0, "model": "gpt-4o",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "Good response"}, "finish_reason": "stop"}],
    })

    req = ChatRequest(model="gpt-4o", messages=[ChatMessage(role=Role.USER, content="Hi")])
    resp = await client.chat(req)
    assert resp.choices[0].message.content == "Good response"
    assert len(httpx_mock.get_requests()) == 2  # original + retry
```

**Step 2: Run tests — expect FAIL**

**Step 3: Implement client.py**

The client follows the 7-step smart chat flow exactly:

```python
from __future__ import annotations
import hashlib
from rustyclaw.cache import ResponseCache
from rustyclaw.config import ClientConfig
from rustyclaw.errors import ClientError, PaymentRequiredError
from rustyclaw.quality import check_degraded
from rustyclaw.session import SessionStore
from rustyclaw.transport import Transport
from rustyclaw.types import ChatRequest, ChatResponse, ChatMessage, PaymentRequired
from rustyclaw.wallet import Wallet
from rustyclaw.signer import Signer

class Solvela Client:
    def __init__(
        self,
        config: ClientConfig | None = None,
        wallet: Wallet | None = None,
        signer: Signer | None = None,
    ) -> None:
        self._config = config or ClientConfig()
        self._wallet = wallet
        self._signer = signer
        self._transport = Transport(
            base_url=self._config.gateway_url,
            timeout=self._config.timeout,
        )
        self._cache = ResponseCache() if self._config.enable_cache else None
        self._session_store = SessionStore(ttl=self._config.session_ttl) if self._config.enable_sessions else None
        self._last_balance: float | None = None

    async def chat(self, request: ChatRequest) -> ChatResponse:
        model = request.model

        # Step 1: Balance guard
        if self._last_balance is not None and self._last_balance == 0 and self._config.free_fallback_model:
            model = self._config.free_fallback_model

        # Step 2: Session lookup
        session_id = None
        if self._session_store is not None:
            session_id = SessionStore.derive_session_id(request.messages)
            info = self._session_store.get_or_create(session_id, model)
            if model == request.model:  # not overridden by balance guard
                model = info.model

        # Step 3: Cache check (after model finalization)
        if self._cache is not None:
            cache_key = ResponseCache.cache_key(model, request.messages)
            cached = self._cache.get(cache_key)
            if cached is not None:
                return cached

        # Step 4: Send request
        effective_request = ChatRequest(
            model=model,
            messages=request.messages,
            max_tokens=request.max_tokens,
            temperature=request.temperature,
            top_p=request.top_p,
            stream=False,
            tools=request.tools,
            tool_choice=request.tool_choice,
        )
        response = await self._send_with_payment(effective_request)

        # Step 5: Quality check + degraded retry
        if self._config.enable_quality_check:
            for retry in range(self._config.max_quality_retries):
                content = response.choices[0].message.content if response.choices else ""
                reason = check_degraded(content)
                if reason is None:
                    break
                response = await self._send_with_payment(
                    effective_request,
                    extra_headers={"X-RCR-Retry-Reason": "degraded"},
                )

        # Step 6: Cache store
        if self._cache is not None:
            self._cache.put(cache_key, response)

        # Step 7: Session update
        if self._session_store is not None and session_id is not None:
            request_hash = ResponseCache.cache_key(model, request.messages)
            self._session_store.record_request(session_id, request_hash)

        return response

    async def _send_with_payment(
        self,
        request: ChatRequest,
        extra_headers: dict[str, str] | None = None,
    ) -> ChatResponse:
        result = await self._transport.send_chat(request, extra_headers=extra_headers)

        if isinstance(result, PaymentRequired):
            if self._signer is None:
                raise PaymentRequiredError(result)

            # Validate recipient and amount
            accept = self._find_compatible_scheme(result)
            self._validate_payment(accept)

            payload = self._signer.sign_payment(
                amount_atomic=int(accept.amount),
                recipient=accept.pay_to,
                resource=result.resource,
                accepted=accept,
            )
            import base64, json
            sig = base64.b64encode(json.dumps(payload.to_dict()).encode()).decode()

            result = await self._transport.send_chat(request, payment_signature=sig, extra_headers=extra_headers)
            if isinstance(result, PaymentRequired):
                raise ClientError("Payment rejected after signing")

        return result

    # ... helper methods: _find_compatible_scheme, _validate_payment, chat_stream, models, etc.

    def last_known_balance(self) -> float | None:
        return self._last_balance

    def __repr__(self) -> str:
        return f"Solvela Client(gateway={self._config.gateway_url}, wallet=REDACTED)"
```

Complete the implementation with `chat_stream()`, `models()`, `estimate_cost()`, `usdc_balance()` methods following the same patterns.

**Step 4: Run tests — expect PASS**

**Step 5: Commit**

```bash
git add src/rustyclaw/client.py tests/unit/test_client.py tests/integration/test_client.py
git commit -m "feat: add Solvela Client with smart chat flow"
```

---

### Task P13: OpenAI Compatibility Wrapper

**Files:**
- Create: `src/rustyclaw/openai_compat.py`
- Test: `tests/unit/test_openai_compat.py`

**Step 1: Write failing tests**

```python
# tests/unit/test_openai_compat.py
import pytest
from unittest.mock import AsyncMock, MagicMock
from rustyclaw.openai_compat import OpenAICompat
from rustyclaw.types import ChatResponse, ChatChoice, ChatMessage, Usage, Role

@pytest.fixture
def mock_client():
    client = MagicMock()
    client.chat = AsyncMock(return_value=ChatResponse(
        id="chatcmpl-123",
        object="chat.completion",
        created=1700000000,
        model="gpt-4o",
        choices=[ChatChoice(
            index=0,
            message=ChatMessage(role=Role.ASSISTANT, content="Hello!"),
            finish_reason="stop",
        )],
        usage=Usage(prompt_tokens=10, completion_tokens=2, total_tokens=12),
    ))
    return client

@pytest.mark.asyncio
async def test_openai_compat_create(mock_client):
    openai = OpenAICompat(mock_client)
    resp = await openai.chat.completions.create(
        model="gpt-4o",
        messages=[{"role": "user", "content": "Hi"}],
    )
    assert resp.choices[0].message.content == "Hello!"

@pytest.mark.asyncio
async def test_openai_compat_accepts_dict_messages(mock_client):
    openai = OpenAICompat(mock_client)
    await openai.chat.completions.create(
        model="gpt-4o",
        messages=[{"role": "user", "content": "Hi"}],
    )
    # Verify the client received ChatMessage objects, not dicts
    call_args = mock_client.chat.call_args
    req = call_args[0][0]
    assert isinstance(req.messages[0], ChatMessage)
```

**Step 2: Run tests — expect FAIL**

**Step 3: Implement openai_compat.py**

```python
from __future__ import annotations
from typing import Any, AsyncIterator

from rustyclaw.types import ChatMessage, ChatRequest, ChatResponse, ChatChunk, Role

class OpenAICompat:
    def __init__(self, client: Any) -> None:
        self.chat = _ChatNamespace(client)

class _ChatNamespace:
    def __init__(self, client: Any) -> None:
        self.completions = _CompletionsNamespace(client)

class _CompletionsNamespace:
    def __init__(self, client: Any) -> None:
        self._client = client

    async def create(
        self,
        model: str,
        messages: list[dict[str, str]],
        stream: bool = False,
        **kwargs: Any,
    ) -> ChatResponse | AsyncIterator[ChatChunk]:
        parsed_messages = [
            ChatMessage(role=Role(m["role"]), content=m.get("content", ""), name=m.get("name"))
            for m in messages
        ]
        request = ChatRequest(model=model, messages=parsed_messages, **kwargs)
        if stream:
            return self._client.chat_stream(request)
        return await self._client.chat(request)
```

**Step 4: Run tests — expect PASS**

**Step 5: Commit**

```bash
git add src/rustyclaw/openai_compat.py tests/unit/test_openai_compat.py
git commit -m "feat: add OpenAI-compatible wrapper"
```

---

### Task P14: Live Contract Tests

**Files:**
- Create: `tests/live/conftest.py`
- Create: `tests/live/test_live_chat.py`
- Create: `tests/live/test_live_models.py`

**Step 1: Write live tests**

```python
# tests/live/conftest.py
import os
import pytest
from rustyclaw.client import Solvela Client
from rustyclaw.config import ClientConfig

GATEWAY_URL = os.environ.get("RUSTYCLAW_GATEWAY_URL", "http://localhost:8402")

@pytest.fixture
def live_client():
    config = ClientConfig(gateway_url=GATEWAY_URL)
    return Solvela Client(config=config)

def pytest_collection_modifyitems(config, items):
    """Skip live tests unless RUSTYCLAW_LIVE_TESTS=1."""
    if os.environ.get("RUSTYCLAW_LIVE_TESTS") != "1":
        skip = pytest.mark.skip(reason="Set RUSTYCLAW_LIVE_TESTS=1 to run")
        for item in items:
            if "live" in str(item.fspath):
                item.add_marker(skip)
```

```python
# tests/live/test_live_chat.py
import pytest
from rustyclaw.types import ChatRequest, ChatMessage, Role

@pytest.mark.asyncio
async def test_live_chat_free_model(live_client):
    """Chat with a free model should work without payment."""
    req = ChatRequest(
        model="auto",  # auto profile should pick a free model if available
        messages=[ChatMessage(role=Role.USER, content="Say hello in exactly one word.")],
    )
    resp = await live_client.chat(req)
    assert resp.choices
    assert len(resp.choices[0].message.content) > 0

@pytest.mark.asyncio
async def test_live_chat_402_without_wallet(live_client):
    """Paid model without wallet should raise PaymentRequiredError."""
    from rustyclaw.errors import PaymentRequiredError
    req = ChatRequest(
        model="gpt-4o",
        messages=[ChatMessage(role=Role.USER, content="Hello")],
    )
    with pytest.raises(PaymentRequiredError):
        await live_client.chat(req)
```

```python
# tests/live/test_live_models.py
import pytest

@pytest.mark.asyncio
async def test_live_models_list(live_client):
    models = await live_client.models()
    assert len(models) > 0
    assert any(m.id for m in models)
```

**Step 2: Verify live tests are skipped by default**

```bash
pytest tests/live/ -v
# Expected: all skipped
```

**Step 3: Commit**

```bash
git add tests/live/
git commit -m "test: add live contract tests (skipped by default)"
```

---

### Task P15: CI & README

**Files:**
- Create: `.github/workflows/ci.yml`
- Create: `README.md`

**Step 1: Create CI workflow**

```yaml
# .github/workflows/ci.yml
name: CI
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        python-version: ["3.10", "3.11", "3.12"]
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-python@v5
        with:
          python-version: ${{ matrix.python-version }}
      - run: pip install -e ".[dev]"
      - run: ruff check src/ tests/
      - run: mypy src/rustyclaw/
      - run: pytest tests/unit/ tests/integration/ -v --tb=short
```

**Step 2: Create README.md**

Include: installation, quick start (minimal and full), API reference summary, link to Solvela docs.

**Step 3: Commit**

```bash
git add .github/workflows/ci.yml README.md
git commit -m "chore: add CI workflow and README"
```

---

## Part 2: TypeScript SDK

### Task T1: Project Scaffold

**Files:**
- Create: `package.json`
- Create: `tsconfig.json`
- Create: `src/index.ts`
- Create: `vitest.config.ts`
- Create: `.gitignore`

**Step 1: Create GitHub repo**

```bash
mkdir rustyclaw-ts && cd rustyclaw-ts
git init
```

**Step 2: Create package.json**

```json
{
  "name": "@rustyclaw/sdk",
  "version": "0.1.0",
  "description": "TypeScript SDK for Solvela — Solana-native AI agent payment gateway",
  "type": "module",
  "main": "dist/index.js",
  "types": "dist/index.d.ts",
  "exports": {
    ".": {
      "import": "./dist/index.js",
      "types": "./dist/index.d.ts"
    }
  },
  "scripts": {
    "build": "tsc",
    "test": "vitest run",
    "test:unit": "vitest run tests/unit",
    "test:integration": "vitest run tests/integration",
    "test:live": "RUSTYCLAW_LIVE_TESTS=1 vitest run tests/live",
    "lint": "tsc --noEmit && biome check src/ tests/"
  },
  "engines": { "node": ">=18" },
  "license": "MIT",
  "dependencies": {
    "@solana/web3.js": "^1.95",
    "@solana/spl-token": "^0.4",
    "bip39": "^3.1"
  },
  "devDependencies": {
    "typescript": "^5.5",
    "vitest": "^2",
    "msw": "^2",
    "@biomejs/biome": "^1.8"
  }
}
```

**Step 3: Create tsconfig.json**

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "outDir": "dist",
    "rootDir": "src",
    "declaration": true,
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "forceConsistentCasingInFileNames": true,
    "resolveJsonModule": true
  },
  "include": ["src"],
  "exclude": ["node_modules", "dist", "tests"]
}
```

**Step 4: Create src/index.ts with re-exports**

```typescript
export { Solvela Client } from './client.js';
export { ClientConfig, ClientBuilder } from './config.js';
export { Wallet } from './wallet.js';
export type { Signer } from './signer.js';
export { KeypairSigner } from './signer.js';
export * from './types.js';
export * from './errors.js';
```

**Step 5: Create empty module files**

Create: `types.ts`, `constants.ts`, `errors.ts`, `config.ts`, `wallet.ts`, `signer.ts`, `transport.ts`, `cache.ts`, `session.ts`, `balance.ts`, `quality.ts`, `client.ts`, `openai_compat.ts` in `src/`.

**Step 6: Create .gitignore**

```
node_modules/
dist/
.env
```

**Step 7: Commit**

```bash
npm install
git add -A
git commit -m "chore: scaffold TypeScript SDK project"
```

---

### Task T2: Types & Constants

**Files:**
- Create: `src/types.ts`
- Create: `src/constants.ts`
- Test: `tests/unit/types.test.ts`

Follow the same type definitions as Python but using TypeScript interfaces and enums. All types need `toJSON()` methods (for requests) and static `fromJSON(data: unknown)` factory methods (for responses).

**Step 1: Write failing tests**

Same test cases as Python P2, translated to TypeScript/vitest:
- Role serialization
- ChatMessage toJSON omits undefined fields
- ChatResponse fromJSON parses correctly
- ChatChunk fromJSON parses correctly
- PaymentRequired fromJSON parses correctly
- Constants values
- Cache key determinism

**Step 2-5: Implement, test, commit**

Same TDD cycle as Python. Use TypeScript discriminated unions for PaymentPayload (Direct vs Escrow).

```bash
git add src/types.ts src/constants.ts tests/unit/types.test.ts
git commit -m "feat: add wire-format types and constants"
```

---

### Task T3: Errors

Same pattern as Python P3. TypeScript error classes extending a base `ClientError`:

```typescript
export class ClientError extends Error { constructor(message: string) { super(message); this.name = 'ClientError'; } }
export class WalletError extends ClientError { ... }
export class SignerError extends ClientError { ... }
export class InsufficientBalanceError extends ClientError { constructor(public have: number, public need: number) { ... } }
export class GatewayError extends ClientError { constructor(public status: number, public override message: string) { ... } }
// ... etc
```

```bash
git commit -m "feat: add error type hierarchy"
```

---

### Task T4: Config & Builder

Same pattern as Python P4. TypeScript `ClientConfig` interface + `ClientBuilder` class with fluent API.

```bash
git commit -m "feat: add ClientConfig and ClientBuilder"
```

---

### Task T5: Wallet

Use `@solana/web3.js` `Keypair` and `bip39` package:

```typescript
import { Keypair } from '@solana/web3.js';
import * as bip39 from 'bip39';
import bs58 from 'bs58';
```

Same API as Python P5: `create()`, `fromMnemonic()`, `fromKeypairBytes()`, `fromEnv()`, `address()`, `toKeypairBytes()`.

```bash
git commit -m "feat: add Wallet with keypair management"
```

---

### Task T6: Cache

Same as Python P6. Use `Map` with manual LRU eviction (or a lightweight LRU implementation). Thread safety is not a concern in Node.js (single-threaded).

```bash
git commit -m "feat: add LRU response cache with TTL and dedup"
```

---

### Task T7: Session Store

Same as Python P7. No threading concerns in Node.js.

```bash
git commit -m "feat: add session store with three-strike escalation"
```

---

### Task T8: Quality Check

Same as Python P8. Port the 4 degradation heuristics.

```bash
git commit -m "feat: add degraded response detection heuristics"
```

---

### Task T9: Transport (HTTP + SSE)

Use native `fetch` (Node 18+) for HTTP. For SSE streaming, parse the `ReadableStream` line by line.

```typescript
const response = await fetch(url, { method: 'POST', headers, body: JSON.stringify(body) });
const reader = response.body!.getReader();
const decoder = new TextDecoder();
// Parse SSE lines...
```

Use `msw` (Mock Service Worker) for integration tests instead of `pytest-httpx`.

```bash
git commit -m "feat: add HTTP transport with SSE streaming"
```

---

### Task T10: Signer

Same interface as Python P10. Use `@solana/web3.js` `Transaction` and `@solana/spl-token` for SPL token transfer:

```typescript
export interface Signer {
    signPayment(amountAtomic: number, recipient: string, resource: Resource, accepted: PaymentAccept): Promise<PaymentPayload>;
}

export class KeypairSigner implements Signer {
    constructor(private wallet: Wallet, private rpcUrl: string = 'https://api.mainnet-beta.solana.com') {}
    // ...
}
```

```bash
git commit -m "feat: add pluggable signer interface with KeypairSigner"
```

---

### Task T11: Balance Monitor

Same as Python P11. Use `setInterval` instead of `asyncio.sleep`:

```typescript
export class BalanceMonitor {
    private intervalId: NodeJS.Timeout | null = null;
    start(): void { this.intervalId = setInterval(() => this.poll(), this.pollInterval * 1000); }
    stop(): void { if (this.intervalId) clearInterval(this.intervalId); }
}
```

```bash
git commit -m "feat: add balance monitor with transition-debounced callback"
```

---

### Task T12: Client (Smart Chat Flow)

Same 7-step flow as Python P12. Wire together all modules.

```bash
git commit -m "feat: add Solvela Client with smart chat flow"
```

---

### Task T13: OpenAI Compatibility Wrapper

Same pattern as Python P13:

```typescript
export class OpenAICompat {
    chat: { completions: { create: (params: CreateParams) => Promise<ChatResponse> } };
    constructor(client: Solvela Client) { ... }
}
```

```bash
git commit -m "feat: add OpenAI-compatible wrapper"
```

---

### Task T14: Live Contract Tests & CI

Same as Python P14-P15. CI uses GitHub Actions with Node 18/20/22 matrix.

```bash
git commit -m "test: add live contract tests"
git commit -m "chore: add CI workflow and README"
```

---

## Part 3: Go SDK

### Task G1: Project Scaffold

**Files:**
- Create: `go.mod`
- Create: `client.go`
- Create: `.gitignore`

```bash
mkdir rustyclaw-go && cd rustyclaw-go
git init
go mod init github.com/Solvela/rustyclaw-go
```

Create empty files: `types.go`, `constants.go`, `errors.go`, `config.go`, `wallet.go`, `signer.go`, `transport.go`, `cache.go`, `session.go`, `balance.go`, `quality.go`, `client.go`

All in package `rustyclaw`.

```bash
git commit -m "chore: scaffold Go SDK project"
```

---

### Task G2: Types & Constants

Use Go structs with `json` tags matching wire format exactly:

```go
type Role string
const (
    RoleSystem    Role = "system"
    RoleUser      Role = "user"
    RoleAssistant Role = "assistant"
    RoleTool      Role = "tool"
    RoleDeveloper Role = "developer"
)

type ChatMessage struct {
    Role       Role        `json:"role"`
    Content    string      `json:"content"`
    Name       *string     `json:"name,omitempty"`
    ToolCalls  []ToolCall  `json:"tool_calls,omitempty"`
    ToolCallID *string     `json:"tool_call_id,omitempty"`
}
// ... all types with json tags, omitempty for optional fields
```

Use `encoding/json` for serialization. Tests in `types_test.go`.

```bash
git commit -m "feat: add wire-format types and constants"
```

---

### Task G3: Errors

Use Go error types with `errors.New` and custom error structs:

```go
type ClientError struct { Message string }
func (e *ClientError) Error() string { return e.Message }

type InsufficientBalanceError struct { Have, Need uint64 }
func (e *InsufficientBalanceError) Error() string { return fmt.Sprintf("insufficient balance: have %d, need %d", e.Have, e.Need) }

type GatewayError struct { Status int; Message string }
// ... etc
```

```bash
git commit -m "feat: add error types"
```

---

### Task G4: Config

```go
type ClientConfig struct {
    GatewayURL        string        `json:"gateway_url"`
    RPCURL            string        `json:"rpc_url"`
    PreferEscrow      bool          `json:"prefer_escrow"`
    Timeout           time.Duration `json:"timeout"`
    ExpectedRecipient string        `json:"expected_recipient,omitempty"`
    MaxPaymentAmount  *uint64       `json:"max_payment_amount,omitempty"`
    EnableCache       bool          `json:"enable_cache"`
    EnableSessions    bool          `json:"enable_sessions"`
    SessionTTL        time.Duration `json:"session_ttl"`
    EnableQualityCheck bool         `json:"enable_quality_check"`
    MaxQualityRetries  int          `json:"max_quality_retries"`
    FreeFallbackModel  string       `json:"free_fallback_model,omitempty"`
}

func DefaultConfig() ClientConfig { ... }
```

Use functional options pattern for builder:

```go
type Option func(*ClientConfig)

func WithGatewayURL(url string) Option { return func(c *ClientConfig) { c.GatewayURL = url } }
func WithTimeout(d time.Duration) Option { return func(c *ClientConfig) { c.Timeout = d } }
// ...

func NewClient(opts ...Option) (*Solvela Client, error) {
    cfg := DefaultConfig()
    for _, opt := range opts { opt(&cfg) }
    // ...
}
```

```bash
git commit -m "feat: add ClientConfig with functional options"
```

---

### Task G5: Wallet

Use `github.com/gagliardetto/solana-go` for Solana keypair management:

```go
import "github.com/gagliardetto/solana-go"

type Wallet struct {
    keypair solana.PrivateKey
}

func CreateWallet() (*Wallet, string, error) { ... }
func WalletFromMnemonic(phrase string) (*Wallet, error) { ... }
func WalletFromKeypairB58(b58 string) (*Wallet, error) { ... }
func WalletFromEnv(varName string) (*Wallet, error) { ... }
func (w *Wallet) Address() string { return w.keypair.PublicKey().String() }
```

```bash
git commit -m "feat: add Wallet with keypair management"
```

---

### Task G6-G8: Cache, Session, Quality

Same patterns as Python, using Go idioms:
- Cache: `sync.Mutex` + `container/list` for LRU
- Session: `sync.RWMutex` + `map[string]*sessionEntry`
- Quality: same 4 heuristics with `strings` package

```bash
git commit -m "feat: add LRU response cache with TTL and dedup"
git commit -m "feat: add session store with three-strike escalation"
git commit -m "feat: add degraded response detection heuristics"
```

---

### Task G9: Transport

Use `net/http` for HTTP, manual SSE parsing for streaming:

```go
func (t *Transport) SendChat(ctx context.Context, req *ChatRequest, opts ...RequestOption) (*ChatResponse, error) {
    // ...
    resp, err := t.client.Do(httpReq)
    // Handle 200, 402, errors
}

func (t *Transport) SendChatStream(ctx context.Context, req *ChatRequest, opts ...RequestOption) (<-chan ChatChunkOrError, error) {
    // Return channel of chunks
    // Parse SSE lines from response body
}
```

```bash
git commit -m "feat: add HTTP transport with SSE streaming"
```

---

### Task G10: Signer

```go
type Signer interface {
    SignPayment(ctx context.Context, amountAtomic uint64, recipient string, resource Resource, accepted PaymentAccept) (*PaymentPayload, error)
}

type KeypairSigner struct {
    wallet *Wallet
    rpcURL string
}
```

```bash
git commit -m "feat: add pluggable signer interface with KeypairSigner"
```

---

### Task G11: Balance Monitor

Use `time.Ticker` for polling:

```go
type BalanceMonitor struct {
    ticker *time.Ticker
    stopCh chan struct{}
    // ...
}

func (m *BalanceMonitor) Start() { go m.run() }
func (m *BalanceMonitor) Stop()  { close(m.stopCh) }
```

```bash
git commit -m "feat: add balance monitor with transition-debounced callback"
```

---

### Task G12: Client

Same 7-step flow. Go uses `context.Context` for cancellation:

```go
func (c *Solvela Client) Chat(ctx context.Context, req *ChatRequest) (*ChatResponse, error) {
    // Steps 1-7...
}
```

```bash
git commit -m "feat: add Solvela Client with smart chat flow"
```

---

### Task G13: Live Tests & CI

Go live tests use build tags:

```go
//go:build live

package rustyclaw_test
```

CI: GitHub Actions with Go 1.21/1.22/1.23 matrix.

```bash
git commit -m "test: add live contract tests"
git commit -m "chore: add CI workflow and README"
```

---

## Execution Order

All three SDKs are independent — they can be implemented in parallel (one subagent per SDK). Within each SDK, tasks must be sequential (each builds on the previous).

**Python:** P1 → P2 → P3 → P4 → P5 → P6 → P7 → P8 → P9 → P10 → P11 → P12 → P13 → P14 → P15

**TypeScript:** T1 → T2 → T3 → T4 → T5 → T6 → T7 → T8 → T9 → T10 → T11 → T12 → T13 → T14

**Go:** G1 → G2 → G3 → G4 → G5 → G6 → G7 → G8 → G9 → G10 → G11 → G12 → G13

## Dependencies

- All SDKs depend on `rustyclaw-protocol` types (documented above, no code dependency)
- Live tests depend on a running Solvela instance (`cargo run -p gateway`)
- Signer implementations depend on Solana devnet for integration testing
