# Python SDK

`solvela-sdk` is the official Python SDK for the Solvela gateway. It's async-first, dataclass-typed, and handles the x402 payment handshake transparently when a `Signer` is configured.

- **Package:** [`solvela-sdk` on PyPI](https://pypi.org/project/solvela-sdk/)
- **Source:** [solvela-ai/solvela-python](https://github.com/solvela-ai/solvela-python)
- **Python:** 3.10+

## Installation

```bash
pip install solvela-sdk
```

Solana wallet, keypair signing, and RPC integration are all built in — no extras required.

## Quick Start

The SDK is async-only. Every example here is meant to be awaited from inside an async function (or wrapped with `asyncio.run`).

```python
import asyncio
from solvela import SolvelaClient, ClientConfig

async def main():
    client = SolvelaClient(config=ClientConfig(gateway_url="http://localhost:8402"))

    # List available models from the gateway's registry.
    models = await client.models()
    for m in models[:3]:
        print(f"{m.id}  —  {m.display_name}  (${m.input_usdc_per_million}/M in)")

asyncio.run(main())
```

Without a `Signer` configured, paid endpoints will raise `PaymentRequiredError` on the first call so the agent can react.

## Configuration

`ClientConfig` is a dataclass; `ClientBuilder` is a fluent equivalent. Both are exported from `solvela`.

```python
from solvela import ClientConfig

config = ClientConfig(
    gateway_url="http://localhost:8402",    # default: https://api.solvela.ai
    rpc_url="https://api.mainnet-beta.solana.com",
    prefer_escrow=False,                    # "exact" first, "escrow" fallback
    timeout=180.0,
    max_payment_amount=10_000_000,          # cap per call (10 USDC = 10_000_000 atomic)
    expected_recipient=None,                # if set, gateway must pay-to this address
    enable_cache=False,                     # opt-in response caching
    enable_sessions=False,                  # opt-in three-strike session escalation
    session_ttl=1800.0,
    enable_quality_check=False,             # opt-in degraded-response retry
    max_quality_retries=1,
    free_fallback_model=None,               # swap to free model when balance hits zero
)
```

Or with the builder:

```python
from solvela import ClientBuilder

config = (
    ClientBuilder()
    .gateway_url("http://localhost:8402")
    .enable_cache(True)
    .enable_sessions(True)
    .free_fallback_model("openai/gpt-4o-mini")
    .max_payment_amount(100_000)            # 0.10 USDC cap
    .build()
)
```

### Security defaults

- Non-loopback `gateway_url` or `rpc_url` must use `https://`. Plain `http://` to a remote host raises `ClientError` at construction.
- Unknown payment schemes from the gateway raise `ClientError` rather than silently mis-branching.
- Malformed amount strings in the 402 body raise `ClientError`, not bare `ValueError`.

## The chat flow

`SolvelaClient.chat()` runs a seven-step smart flow when enabled: balance guard → session lookup → cache check → request → quality check → cache store → session record. For a minimal call only the request step fires.

```python
from solvela import SolvelaClient, ClientConfig
from solvela.types import ChatRequest, ChatMessage, Role

async def ask(prompt: str) -> str:
    client = SolvelaClient(config=ClientConfig(gateway_url="http://localhost:8402"))
    request = ChatRequest(
        model="openai/gpt-4o-mini",
        messages=[ChatMessage(role=Role.USER, content=prompt)],
        max_tokens=200,
    )
    response = await client.chat(request)
    return response.choices[0].message.content or ""
```

`ChatResponse` is OpenAI-compatible — `response.id`, `response.choices[i].message`, `response.usage`.

### With a wallet and signer (paid endpoints)

```python
from solvela import SolvelaClient, ClientConfig, Wallet, KeypairSigner

wallet, mnemonic = Wallet.create()
print(f"Save this mnemonic: {mnemonic}")
print(f"Fund this address with USDC: {wallet.address()}")

signer = KeypairSigner(wallet=wallet, rpc_url="https://api.devnet.solana.com")
client = SolvelaClient(
    config=ClientConfig(gateway_url="http://localhost:8402"),
    wallet=wallet,
    signer=signer,
)

response = await client.chat(request)
```

The SDK automatically:

1. Sends the request.
2. Receives 402 with the price quote.
3. Picks a compatible scheme (`exact` or `escrow`, respecting `prefer_escrow`).
4. Calls `signer.sign_payment(...)` to build and sign the SPL transfer.
5. Retries the request with the `Payment-Signature` header.

If the gateway returns 402 a *second* time after the signed retry, the SDK raises `PaymentRejectedError` carrying the second `PaymentRequired` body so the caller can inspect *why* it was rejected.

## Streaming

```python
async for chunk in client.chat_stream(request):
    print(chunk.choices[0].delta.content or "", end="", flush=True)
print()
```

`chat_stream` runs the same preflight payment handshake as `chat()`. A post-signing 402 on the streaming POST is converted to `PaymentRejectedError` so streaming callers can distinguish "needs signing" from "signed and rejected."

## Models and pricing

```python
models = await client.models()  # list[ModelInfo]

for m in models:
    print(
        f"{m.id:<35} "
        f"in={m.input_usdc_per_million:>5.2f}  "
        f"out={m.output_usdc_per_million:>5.2f}  "
        f"ctx={m.context_window:>7}  "
        f"tools={m.supports_tools}  vision={m.supports_vision}  reasoning={m.reasoning}"
    )
```

`ModelInfo` mirrors the gateway's `/v1/models` shape — capabilities and pricing are flattened off the nested objects the gateway emits, so the dataclass surface stays terse:

| Field | Type | Description |
|---|---|---|
| `id` | `str` | Canonical model identifier (e.g. `"openai/gpt-4o"`) |
| `provider` | `str` | Upstream provider (`"openai"`, `"anthropic"`, etc.) |
| `display_name` | `str` | Human-readable label |
| `context_window` | `int` | Maximum input tokens |
| `supports_streaming` / `supports_tools` / `supports_vision` / `reasoning` | `bool` | Capability flags |
| `input_usdc_per_million` / `output_usdc_per_million` | `float` | Provider rate in USDC per million tokens (human units, not atomic) |
| `currency` / `fee_percent` | `str` / `int` | Platform fee terms |

### Cost estimation

```python
quote = await client.estimate_cost("openai/gpt-4o")
print(f"Estimated total: {quote.cost_breakdown.total} {quote.cost_breakdown.currency}")
print(f"Provider: {quote.cost_breakdown.provider_cost}, fee: {quote.cost_breakdown.platform_fee}")
```

`estimate_cost` triggers a 402 against the model, returns the parsed `PaymentRequired` without signing.

## Error hierarchy

All SDK errors inherit from `ClientError`. Catch the base for "anything the SDK threw," catch a subclass for specific recovery.

```python
from solvela import (
    ClientError,
    PaymentRequiredError,
    PaymentRejectedError,
    AmountExceedsMaxError,
    RecipientMismatchError,
    InsufficientBalanceError,
    GatewayError,
    SignerError,
    WalletError,
    TimeoutError as SolvelaTimeoutError,
)

try:
    response = await client.chat(request)
except PaymentRequiredError as exc:
    # No signer configured — adopt one or surface to the user.
    pr = exc.payment_required
    print(f"Payment needed: {pr.cost_breakdown.total} {pr.cost_breakdown.currency}")
except PaymentRejectedError as exc:
    # Signed but the gateway still rejected. `payment_required` carries the
    # second 402 body (cost, schemes, gateway error message).
    print(f"Rejected: {exc.reason}; body: {exc.payment_required}")
except AmountExceedsMaxError as exc:
    print(f"Quote {exc.amount} exceeds local cap {exc.max_amount}")
except GatewayError as exc:
    print(f"Gateway HTTP {exc.status}: {exc.message}")
except SolvelaTimeoutError as exc:
    print(f"Timed out after {exc.timeout_secs}s")
```

| Error | Raised when |
|---|---|
| `PaymentRequiredError` | Gateway returns 402 and no `Signer` is configured. Carries `payment_required` (the parsed body). |
| `PaymentRejectedError` | Gateway returns 402 *again* after a signed retry. Carries `reason` and (optional) `payment_required`. |
| `AmountExceedsMaxError` | Gateway's quoted amount exceeds the local `max_payment_amount` cap. Carries `amount` and `max_amount` as `AtomicUsdc`. |
| `RecipientMismatchError` | Gateway's `pay_to` doesn't match the configured `expected_recipient`. |
| `InsufficientBalanceError` | Wallet balance is too low for the quoted amount. Carries `have` / `need` as `AtomicUsdc`. |
| `GatewayError` | Non-200/402 HTTP response. Carries `status` and `message`. |
| `SignerError` | The configured `Signer` failed to build or sign the payment transaction. |
| `WalletError` | Wallet operation failed (e.g. mnemonic parse, keypair derivation). |
| `TimeoutError` | HTTP timeout exceeded. |
| `ClientError` | Base — also raised directly for wire-format violations (unknown scheme, malformed amount). |

## Balance monitoring

For long-running agents, `BalanceMonitor` polls USDC balance in the background and wires transitions back into the client's balance guard.

```python
from solvela import BalanceMonitor

monitor = BalanceMonitor(
    fetch_balance=client.usdc_balance,
    poll_interval=30.0,
    low_balance_threshold=0.50,             # USDC
    on_low_balance=lambda b: print(f"Low: ${b:.4f}"),
    on_balance_change=client.balance_state_setter(),
)
monitor.start()
# ... agent loop ...
monitor.stop()
```

When the polled balance hits `0.0` and `ClientConfig.free_fallback_model` is set, the client's chat balance guard swaps the requested model for the free fallback automatically. On RPC failure, `on_balance_change(None)` clears the cached balance to "unknown" so a stale `0.0` doesn't lock callers into the fallback during an outage.

## OpenAI-compatible interface

For code that already targets the OpenAI Python SDK, `OpenAICompat` provides a drop-in shim.

```python
from solvela import SolvelaClient
from solvela.openai_compat import OpenAICompat

client = SolvelaClient()
openai = OpenAICompat(client)

response = await openai.chat.completions.create(
    model="openai/gpt-4o",
    messages=[{"role": "user", "content": "Hello!"}],
    max_tokens=100,
)
print(response.choices[0].message.content)
```

`openai.chat.completions.create(stream=True, ...)` returns the same `AsyncIterator[ChatChunk]` as `SolvelaClient.chat_stream()`.

## Custom signers

The default `KeypairSigner` builds and signs Solana SPL transfers. You can implement the `Signer` ABC to plug in HSM-backed signing, multi-sig flows, or a custodial backend:

```python
from solvela import Signer
from solvela.types import AtomicUsdc, PaymentAccept, PaymentPayload, Resource

class MyCustomSigner(Signer):
    async def sign_payment(
        self,
        amount_atomic: AtomicUsdc,
        recipient: str,
        resource: Resource,
        accepted: PaymentAccept,
    ) -> PaymentPayload:
        # Build, sign, and return a PaymentPayload your way.
        ...
```

`AtomicUsdc` is a type-only `NewType` over `int`; pass `AtomicUsdc(123)` at the boundary to make mypy track the unit. Wire amounts (`accept.amount`) are decimal strings in atomic USDC (1 USDC = 1,000,000).

## Cancellation and timeouts

`ClientConfig.timeout` (default 180s) caps every HTTP call. Cancel an in-flight `chat()` by cancelling the surrounding asyncio task — `httpx` propagates cancellation through cleanly.

## Where to look next

- **Source:** [src/solvela/client.py](https://github.com/solvela-ai/solvela-python/blob/main/src/solvela/client.py) is the canonical entry point for the chat flow.
- **Examples:** [scripts/smoke.py](https://github.com/solvela-ai/solvela-python/blob/main/scripts/smoke.py) is a 60-line end-to-end script that hits `/v1/models` and an unsigned `/v1/chat/completions` and verifies the wire contract.
- **Release process:** [docs/RELEASE_CHECKLIST.md](https://github.com/solvela-ai/solvela-python/blob/main/docs/RELEASE_CHECKLIST.md) in the SDK repo.
- **x402 protocol:** [Solvela's x402 implementation](../concepts/x402-protocol.html), [x402.org](https://www.x402.org/).
