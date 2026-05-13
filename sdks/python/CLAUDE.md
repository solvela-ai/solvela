# CLAUDE.md

This file provides guidance to Claude Code when working with the solvela-python SDK.

## Build / Test / Lint Commands

```bash
# Install (editable, with dev dependencies)
pip install -e ".[dev]"

# Run all tests (unit + integration)
pytest tests/unit/ tests/integration/ -v

# Run a single test file
pytest tests/unit/test_client.py -v

# Run tests matching a pattern
pytest tests/unit/ -k "test_cache_hit" -v

# Live contract tests (requires running Solvela gateway)
SOLVELA_LIVE_TESTS=1 pytest tests/live/ -v

# Linting
ruff check src/ tests/
ruff format --check src/ tests/

# Type checking
mypy src/
```

## Architecture

Python SDK for Solvela — enables AI agents to pay for LLM API calls with USDC-SPL tokens on Solana via the x402 protocol. This is the client SDK; the server is Solvela (Rust/Axum).

### Module Map

```
src/solvela/
  client.py        # SolvelaClient — high-level 7-step smart chat flow
  config.py        # ClientConfig dataclass + ClientBuilder (fluent API)
  transport.py     # Async HTTP + SSE streaming via httpx
  types.py         # Wire-format dataclasses — OpenAI-compatible chat + x402 payment types
  errors.py        # Error hierarchy (ClientError base, 8 domain subtypes)
  wallet.py        # Solana keypair management with BIP39 mnemonic support
  signer.py        # Pluggable payment signing — Signer ABC + KeypairSigner (SPL transfer)
  balance.py       # BalanceMonitor — background USDC-SPL balance poller with low-balance callback
  cache.py         # LRU ResponseCache with TTL + dedup window (thread-safe)
  session.py       # SessionStore with TTL + three-strike escalation (thread-safe)
  quality.py       # Degraded response detection (4 heuristics: empty, error phrase, repetition, truncation)
  openai_compat.py # OpenAI-compatible wrapper (openai.chat.completions.create interface)
  constants.py     # Protocol constants (x402 version, USDC mint, network, fee percent)
```

### Smart Chat Flow

`SolvelaClient.chat()` (`client.py:61`) runs 7 steps:

1. **Balance guard** (`client.py:76`) — if `_last_balance == 0` and `free_fallback_model` is set, swap model.
2. **Session lookup** (`client.py:84`) — `SessionStore.derive_session_id` from messages; `get_or_create` may force a model on third strike.
3. **Cache check** (`client.py:92`) — keyed on `(model, messages)` *after* model finalization to prevent cross-model pollution.
4. **Send** (`client.py:110` → `transport.py:39`) — on `402`, `_send_with_payment` picks a scheme, `Signer` builds an SPL transfer tx, retries with the `Payment-Signature` header.
5. **Quality check** (`client.py:113`) — `check_degraded` runs up to `max_quality_retries`; retries set `X-Solvela-Retry-Reason: degraded`.
6. **Cache store** (`client.py:124`).
7. **Session record** (`client.py:128`).

`chat_stream()` (`client.py:135`) runs only steps 1, 2, 4, 7 — no cache, no quality check.

### Payment Flow (x402 protocol)

1. Client sends chat request to gateway
2. Gateway returns 402 with `PaymentRequired` (cost breakdown + accepted payment schemes)
3. Client selects scheme (prefer "exact", fallback to "escrow")
4. `Signer` builds and signs a Solana SPL Token transfer transaction
5. Client retries with `Payment-Signature` header (base64-encoded JSON payload)
6. Gateway verifies payment and returns chat completion

### Where to Look

| Task | File |
|---|---|
| Change request flow | `client.py` (`chat`, `chat_stream`, `_send_with_payment`) |
| Add/change wire fields | `types.py` (+ round-trip in `tests/unit/test_types.py`) |
| Add a payment scheme or signer | `signer.py` (implement `Signer` ABC) |
| HTTP behavior or status mapping | `transport.py` |
| Bump x402 version, USDC mint, fee % | `constants.py` |
| Add an error type | `errors.py` (re-export from `__init__.py`) |
| Adjust degraded-response heuristics | `quality.py` |
| Test against a running gateway | `tests/live/test_live_chat.py` (gated by `SOLVELA_LIVE_TESTS=1`) |

## Code Conventions

- **Python 3.10+** — uses `X | Y` union syntax, no `from __future__ import annotations` needed (but present for consistency)
- **Async-first** — all I/O is async via `httpx.AsyncClient`; tests use `pytest-asyncio` with `asyncio_mode = "auto"`
- **Dataclasses for types** — all wire-format types are `@dataclass` with `to_dict()` / `from_dict()` classmethods
- **Thread-safe caches** — `ResponseCache` and `SessionStore` use `threading.Lock`
- **Error hierarchy** — `ClientError` base, never raise bare `Exception`
- **No mutation of inputs** — build new `ChatRequest` with overrides rather than modifying the original
- **Pluggable signing** — `Signer` ABC allows custom implementations; `KeypairSigner` is the default
- **Ruff lint rules** — `E, F, I, N, UP, B, SIM, TCH` selected; line length 100
- **Mypy strict mode** — `strict = true` in pyproject.toml
- **Test layout** — `tests/unit/` (mocked, fast), `tests/integration/` (httpx mocked transport), `tests/live/` (real gateway, gated by env var)
- **Solders for Solana** — `solders` crate bindings for keypair, pubkey, transaction construction
- **USDC amounts in atomic units** — 1 USDC = 1,000,000 atomic units (6 decimals)
