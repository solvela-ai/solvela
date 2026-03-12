# Quick Start

This walkthrough takes you from zero to a working LLM request through RustyClawRouter.

## 1. Start the Gateway

```bash
# Start backing services (optional but recommended)
docker compose up -d

# Configure at least one provider key
cp .env.example .env
echo 'OPENAI_API_KEY=sk-your-key-here' >> .env

# Start the gateway
RUST_LOG=info cargo run -p gateway
```

The gateway listens on `http://localhost:8402`.

## 2. Check Available Models

```bash
curl -s http://localhost:8402/v1/models | jq '.models[:3]'
```

```json
[
  {
    "id": "openai/gpt-4o",
    "provider": "openai",
    "display_name": "GPT-4o",
    "context_window": 128000,
    "pricing": {
      "input_cost_per_million": 2.5,
      "output_cost_per_million": 10.0,
      "platform_fee_percent": 5
    }
  },
  {
    "id": "openai/gpt-4o-mini",
    "provider": "openai",
    "display_name": "GPT-4o Mini",
    "context_window": 128000,
    "pricing": {
      "input_cost_per_million": 0.15,
      "output_cost_per_million": 0.6,
      "platform_fee_percent": 5
    }
  }
]
```

## 3. Understand the 402 Flow

Make a chat request without payment:

```bash
curl -s -w "\nHTTP Status: %{http_code}\n" \
  -X POST http://localhost:8402/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "openai/gpt-4o-mini",
    "messages": [{"role": "user", "content": "Hello"}]
  }'
```

The gateway returns **HTTP 402 Payment Required** with a cost breakdown:

```json
{
  "error": "payment_required",
  "payment_required": {
    "recipient_wallet": "GatewayWallet...",
    "usdc_mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
    "amount_usdc": "0.000394",
    "cost_breakdown": {
      "input_tokens_estimated": 10,
      "output_tokens_max": 500,
      "input_cost_usdc": "0.000002",
      "output_cost_usdc": "0.000300",
      "platform_fee_usdc": "0.000015",
      "total_usdc": "0.000394"
    },
    "accepted_schemes": ["exact"],
    "chain": "solana",
    "network": "devnet"
  }
}
```

This is the x402 protocol in action. The agent must:

1. Parse the 402 response to learn the price
2. Sign a USDC-SPL `TransferChecked` transaction for that amount
3. Retry with the `PAYMENT-SIGNATURE` header containing the signed transaction

## 4. Make a Paid Request

With an SDK (recommended), payment is transparent:

```python
from rustyclawrouter import LLMClient

client = LLMClient(api_url="http://localhost:8402")
reply = client.chat("openai/gpt-4o-mini", "Hello!")
print(reply)
```

Or using the CLI:

```bash
cargo run -p cli -- chat "Hello, world!"
```

The SDK and CLI handle the 402 dance automatically: first request gets the price, the client signs and retries, and the response comes back.

## 5. Use Smart Routing

Instead of specifying a model, use a routing profile:

```bash
curl -s -X POST http://localhost:8402/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "PAYMENT-SIGNATURE: <signed-payment>" \
  -d '{
    "model": "auto",
    "messages": [{"role": "user", "content": "Explain quantum entanglement"}]
  }'
```

The smart router scores your request across 15 dimensions and selects the best model for the request complexity and routing profile:

| Profile | Description |
|---------|-------------|
| `eco` / `cheap` / `budget` | Cheapest capable model per tier |
| `auto` / `balanced` / `default` | Balanced cost and quality |
| `premium` / `best` / `quality` | Best available model regardless of cost |
| `free` / `oss` / `open` | Free-tier models only |

## 6. Check Pricing

```bash
curl -s http://localhost:8402/pricing | jq '.models[0]'
```

```json
{
  "id": "openai/gpt-4o",
  "pricing": {
    "input_per_million_usdc": 2.5,
    "output_per_million_usdc": 10.0,
    "platform_fee_percent": 5
  },
  "example_1k_token_request": {
    "total_usdc": "0.006563"
  }
}
```

## 7. Run Diagnostics

The CLI includes a `doctor` command that checks gateway connectivity, wallet status, model availability, and more:

```bash
cargo run -p cli -- doctor
```

```
[PASS] Wallet loaded
[PASS] Gateway reachable at http://localhost:8402
[PASS] 22 models available
[PASS] Solana RPC connected
[PASS] USDC balance: 10.50
[PASS] Payment flow verified
```

## Next Steps

- [Configuration](./configuration.md) -- full environment variable and config file reference
- [How It Works](../concepts/how-it-works.md) -- detailed request flow
- [x402 Payment Protocol](../concepts/x402-protocol.md) -- deep dive into the payment protocol
- [API Reference](../api/chat-completions.md) -- endpoint documentation
