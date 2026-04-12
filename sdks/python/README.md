# Solvela Python SDK

Python SDK for Solvela — AI agent payments with USDC on Solana via the x402 protocol.

## Installation

```bash
pip install solvela
```

With Solana wallet support (signing transactions):

```bash
pip install solvela[solana]
```

## Quick Start

```python
from solvela import LLMClient

# Uses SOLANA_WALLET_KEY env var for payment
client = LLMClient(api_url="http://localhost:8402")

# Simple chat — payment is handled transparently
reply = client.chat("openai/gpt-4o", "What is the x402 protocol?")
print(reply)
```

## Async Usage

```python
import asyncio
from solvela import AsyncLLMClient

async def main():
    async with AsyncLLMClient(api_url="http://localhost:8402") as client:
        reply = await client.chat("openai/gpt-4o", "Hello!")
        print(reply)

asyncio.run(main())
```

## Session Budgets

```python
client = LLMClient(session_budget=0.50)  # Max $0.50 USDC per session
try:
    reply = client.chat("openai/gpt-4o", "Expensive prompt...")
except BudgetExceededError:
    print(f"Budget exceeded! Spent: ${client.session_spent:.4f}")
```

## Smart Routing

```python
# Let the gateway pick the cheapest capable model
response = client.smart_chat("Explain quantum computing", profile="eco")
```

## Configuration

| Parameter | Default | Description |
|-----------|---------|-------------|
| `private_key` | `$SOLANA_WALLET_KEY` | Base58 Solana private key |
| `api_url` | `https://api.solvela.com` | Gateway URL |
| `session_budget` | `None` | Max USDC spend per session |
| `timeout` | `60.0` | HTTP timeout in seconds |

## License

MIT
