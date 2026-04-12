# Python SDK

The Python SDK provides a synchronous and asynchronous client for Solvela with transparent x402 payment handling.

## Installation

```bash
pip install solvela
```

With Solana wallet support (transaction signing):

```bash
pip install solvela[solana]
```

## Quick Start

```python
from solvela import LLMClient

client = LLMClient(api_url="http://localhost:8402")

# Simple one-shot chat
reply = client.chat("openai/gpt-4o", "What is the x402 protocol?")
print(reply)
```

The SDK handles the full x402 flow automatically:
1. Sends the request
2. Receives the 402 with the price quote
3. Signs a USDC-SPL transaction using the configured wallet
4. Retries with the `PAYMENT-SIGNATURE` header
5. Returns the response

## Configuration

The client reads from environment variables by default:

| Variable | Description |
|----------|-------------|
| `SOLANA_WALLET_KEY` | Base58 wallet private key for signing payments |
| `RCR_API_URL` | Gateway URL (default: `http://localhost:8402`) |
| `SOLANA_RPC_URL` | Solana RPC endpoint for transaction submission |

Or configure explicitly:

```python
client = LLMClient(
    api_url="https://solvela-gateway.fly.dev",
    wallet_key="your-base58-private-key",
    rpc_url="https://api.mainnet-beta.solana.com",
)
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

## Streaming

```python
for chunk in client.chat_stream("openai/gpt-4o", "Write a poem about Solana"):
    print(chunk, end="", flush=True)
print()
```

Async streaming:

```python
async for chunk in client.chat_stream("openai/gpt-4o", "Write a poem"):
    print(chunk, end="", flush=True)
```

## Session Budgets

Set a maximum USDC spend per session to prevent runaway costs:

```python
from solvela import LLMClient, BudgetExceededError

client = LLMClient(session_budget=0.50)  # Max $0.50 USDC

try:
    reply = client.chat("openai/gpt-4o", "Expensive analysis...")
except BudgetExceededError:
    print(f"Budget exceeded! Spent: ${client.session_spent:.4f}")
```

## Smart Routing

Let the gateway pick the optimal model based on request complexity:

```python
# Use a routing profile
response = client.smart_chat("Explain quantum computing", profile="eco")

# Aliases work too
response = client.smart_chat("Write a sorting algorithm", profile="auto")
```

Profile options: `eco`, `auto`, `premium`, `free`

## Full Chat Completion

For full control over the request:

```python
from solvela import LLMClient

client = LLMClient(api_url="http://localhost:8402")

response = client.chat_completion(
    model="anthropic/claude-sonnet-4.6",
    messages=[
        {"role": "system", "content": "You are a Rust expert."},
        {"role": "user", "content": "Explain ownership and borrowing."},
    ],
    max_tokens=1000,
    temperature=0.3,
)

print(response["choices"][0]["message"]["content"])
print(f"Tokens used: {response['usage']['total_tokens']}")
```

## Error Handling

```python
from solvela import (
    LLMClient,
    BudgetExceededError,
    PaymentError,
    ProviderError,
)

client = LLMClient(api_url="http://localhost:8402")

try:
    reply = client.chat("openai/gpt-4o", "Hello")
except BudgetExceededError:
    print("Session budget exceeded")
except PaymentError as e:
    print(f"Payment failed: {e}")
except ProviderError as e:
    print(f"Provider error: {e.status_code} - {e.message}")
```

## List Models

```python
models = client.list_models()
for model in models:
    print(f"{model['id']}: ${model['pricing']['input_cost_per_million']}/M input")
```
