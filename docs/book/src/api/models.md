# Models

`GET /v1/models`

Returns all available models with pricing and capability information.

## Request

```bash
curl -s http://localhost:8402/v1/models | jq
```

## Response

```json
{
  "object": "list",
  "models": [
    {
      "id": "openai/gpt-4o",
      "provider": "openai",
      "display_name": "GPT-4o",
      "context_window": 128000,
      "supports_streaming": true,
      "supports_tools": true,
      "supports_vision": true,
      "reasoning": false,
      "pricing": {
        "input_cost_per_million": 2.5,
        "output_cost_per_million": 10.0,
        "platform_fee_percent": 5
      }
    }
  ]
}
```

## Model List

| ID | Provider | Display Name | Input $/M | Output $/M | Context | Vision | Tools | Reasoning |
|----|----------|-------------|-----------|------------|---------|--------|-------|-----------|
| `openai/gpt-5.2` | OpenAI | GPT-5.2 | 1.75 | 14.00 | 400K | Yes | Yes | Yes |
| `openai/gpt-4o` | OpenAI | GPT-4o | 2.50 | 10.00 | 128K | Yes | Yes | -- |
| `openai/gpt-4o-mini` | OpenAI | GPT-4o Mini | 0.15 | 0.60 | 128K | -- | Yes | -- |
| `openai/o3` | OpenAI | o3 | 2.00 | 8.00 | 200K | -- | -- | Yes |
| `openai/o3-mini` | OpenAI | o3 Mini | 1.10 | 4.40 | 200K | -- | Yes | Yes |
| `openai/o4-mini` | OpenAI | o4 Mini | 1.10 | 4.40 | 200K | -- | Yes | Yes |
| `openai/gpt-4.1` | OpenAI | GPT-4.1 | 2.00 | 8.00 | 1M | Yes | Yes | -- |
| `openai/gpt-4.1-mini` | OpenAI | GPT-4.1 Mini | 0.40 | 1.60 | 1M | Yes | Yes | -- |
| `openai/gpt-4.1-nano` | OpenAI | GPT-4.1 Nano | 0.10 | 0.40 | 1M | -- | Yes | -- |
| `openai/gpt-oss-120b` | OpenAI | GPT-OSS 120B | 0.00 | 0.00 | 128K | -- | -- | -- |
| `anthropic/claude-opus-4.6` | Anthropic | Claude Opus 4.6 | 5.00 | 25.00 | 200K | Yes | Yes | Yes |
| `anthropic/claude-sonnet-4.6` | Anthropic | Claude Sonnet 4.6 | 3.00 | 15.00 | 200K | -- | Yes | Yes |
| `anthropic/claude-sonnet-4.5` | Anthropic | Claude Sonnet 4.5 | 3.00 | 15.00 | 200K | Yes | Yes | -- |
| `anthropic/claude-haiku-4.5` | Anthropic | Claude Haiku 4.5 | 1.00 | 5.00 | 200K | -- | -- | -- |
| `google/gemini-3.1-pro` | Google | Gemini 3.1 Pro | 2.00 | 12.00 | 1M | -- | Yes | Yes |
| `google/gemini-2.5-flash` | Google | Gemini 2.5 Flash | 0.30 | 2.50 | 1M | -- | -- | -- |
| `google/gemini-2.5-flash-lite` | Google | Gemini 2.5 Flash Lite | 0.10 | 0.40 | 1M | -- | -- | -- |
| `google/gemini-2.0-flash` | Google | Gemini 2.0 Flash | 0.10 | 0.40 | 1M | -- | Yes | -- |
| `google/gemini-2.0-flash-lite` | Google | Gemini 2.0 Flash Lite | 0.075 | 0.30 | 1M | -- | -- | -- |
| `deepseek/deepseek-chat` | DeepSeek | DeepSeek V3.2 Chat | 0.28 | 0.42 | 128K | -- | -- | -- |
| `deepseek/deepseek-reasoner` | DeepSeek | DeepSeek V3.2 Reasoner | 0.28 | 0.42 | 128K | -- | -- | Yes |
| `deepseek/deepseek-coder` | DeepSeek | DeepSeek Coder V3 | 0.28 | 0.42 | 128K | -- | Yes | -- |
| `xai/grok-4-fast-reasoning` | xAI | Grok 4 Fast (Reasoning) | 0.20 | 0.50 | 2M | -- | -- | Yes |
| `xai/grok-code-fast-1` | xAI | Grok Code Fast | 0.20 | 1.50 | 256K | -- | -- | -- |
| `xai/grok-3` | xAI | Grok 3 | 3.00 | 15.00 | 131K | Yes | Yes | -- |
| `xai/grok-3-mini` | xAI | Grok 3 Mini | 0.30 | 0.50 | 131K | -- | Yes | Yes |

All prices are provider cost per million tokens. The 5% platform fee is added automatically.

## Pricing Endpoint

`GET /pricing`

Returns detailed pricing with example cost calculations:

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
    "input_cost": "0.002500",
    "output_cost": "0.010000",
    "platform_fee": "0.000625",
    "total_usdc": "0.013125"
  }
}
```

## Supported Endpoint

`GET /v1/supported`

Returns x402 ecosystem payment method discovery:

```bash
curl -s http://localhost:8402/v1/supported | jq
```

```json
{
  "protocol": "x402",
  "version": "2",
  "chain": "solana",
  "network": "devnet",
  "payment_token": "USDC-SPL",
  "usdc_mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
  "accepted_schemes": ["exact", "escrow"],
  "endpoints": ["/v1/chat/completions", "/v1/services/{service_id}/proxy"]
}
```
