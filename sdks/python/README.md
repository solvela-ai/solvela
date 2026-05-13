# solvela

Python SDK for [Solvela](https://github.com/solvela-ai/solvela) — a Solana-native AI agent payment gateway. AI agents pay for LLM API calls with USDC-SPL on Solana via the x402 protocol.

## Installation

```bash
pip install solvela-sdk
```

For development:

```bash
pip install -e ".[dev]"
```

## Quick Start

```python
import asyncio
from solvela import SolvelaClient, ClientConfig, Wallet

async def main():
    wallet, mnemonic = Wallet.create()
    print(f"Wallet: {wallet.address()}")
    print(f"Mnemonic (save this!): {mnemonic}")

    config = ClientConfig(gateway_url="http://localhost:8402")
    client = SolvelaClient(config=config, wallet=wallet)

    # List available models
    models = await client.models()
    for m in models:
        print(f"  {m.id} — {m.display_name}")

    # Estimate cost (triggers 402)
    cost = await client.estimate_cost("gpt-4o")
    print(f"Cost: {cost.cost_breakdown.total} {cost.cost_breakdown.currency}")

asyncio.run(main())
```

## OpenAI-Compatible Interface

```python
from solvela import SolvelaClient
from solvela.openai_compat import OpenAICompat

client = SolvelaClient()
openai = OpenAICompat(client)

# Same interface as the OpenAI Python SDK
response = await openai.chat.completions.create(
    model="gpt-4o",
    messages=[{"role": "user", "content": "Hello!"}],
)
```

## Smart Features

- **Response caching** — `enable_cache=True` deduplicates identical requests
- **Session tracking** — `enable_sessions=True` tracks conversation context with three-strike escalation
- **Quality checks** — `enable_quality_check=True` detects degraded responses and retries
- **Balance guard** — automatic fallback to free models when USDC balance hits zero
- **Balance monitor** — background polling with low-balance callbacks

## Configuration

```python
from solvela import ClientConfig, ClientBuilder

# Dataclass
config = ClientConfig(
    gateway_url="http://localhost:8402",
    enable_cache=True,
    enable_sessions=True,
    enable_quality_check=True,
    free_fallback_model="deepseek-chat",
    max_payment_amount=100_000,  # atomic USDC (0.10 USDC)
)

# Fluent builder
config = (
    ClientBuilder()
    .gateway_url("http://localhost:8402")
    .enable_cache(True)
    .max_payment_amount(100_000)
    .build()
)
```

## Running Tests

```bash
# Unit + integration tests
pytest tests/unit/ tests/integration/ -v

# Live contract tests (requires running gateway)
SOLVELA_LIVE_TESTS=1 pytest tests/live/ -v

# Linting
ruff check src/ tests/
```

## License

MIT
