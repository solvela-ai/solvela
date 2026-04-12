# Configuration

Solvela is configured through environment variables and TOML config files. Environment variables take precedence. Secrets (API keys, private keys) are **only** accepted via environment variables, never config files.

## Environment Variables

### Server

| Variable | Default | Description |
|----------|---------|-------------|
| `RCR_SERVER_HOST` | `0.0.0.0` | Host to bind to |
| `RCR_SERVER_PORT` | `8402` | Port to listen on |
| `RCR_ENV` | `development` | Environment mode. Set to `production` to disable localhost CORS origins |
| `RUST_LOG` | -- | Log level filter. Recommended: `gateway=info,tower_http=info` |

### Solana

| Variable | Default | Description |
|----------|---------|-------------|
| `RCR_SOLANA_RPC_URL` | `https://api.devnet.solana.com` | Solana RPC endpoint |
| `RCR_SOLANA_RECIPIENT_WALLET` | -- | Gateway's USDC recipient wallet (base58 pubkey) |
| `RCR_SOLANA_FEE_PAYER_KEY` | -- | Primary hot wallet private key (base58) for claim tx fees |
| `RCR_SOLANA_ESCROW_PROGRAM_ID` | -- | Escrow program ID (base58). Enables escrow payment mode |

Additional fee payer keys for rotation are loaded from `RCR_SOLANA__FEE_PAYER_KEY_2` through `_8`.

### LLM Providers

At least one key is required. Providers without a key fall back to the stub `FallbackProvider` (returns 503).

| Variable | Provider |
|----------|----------|
| `OPENAI_API_KEY` | OpenAI (GPT-5.2, GPT-4o, GPT-4o Mini, o3, o3-mini, o4-mini, GPT-4.1, GPT-4.1 Mini, GPT-4.1 Nano, GPT-OSS 120B) |
| `ANTHROPIC_API_KEY` | Anthropic (Claude Opus 4.6, Claude Sonnet 4.6, Claude Sonnet 4.5, Claude Haiku 4.5) |
| `GOOGLE_API_KEY` | Google (Gemini 3.1 Pro, Gemini 2.5 Flash, Gemini 2.5 Flash Lite, Gemini 2.0 Flash, Gemini 2.0 Flash Lite) |
| `XAI_API_KEY` | xAI (Grok 4 Fast Reasoning, Grok Code Fast, Grok 3, Grok 3 Mini) |
| `DEEPSEEK_API_KEY` | DeepSeek (DeepSeek V3.2 Chat, DeepSeek V3.2 Reasoner, DeepSeek Coder V3) |

### Infrastructure (Optional)

| Variable | Default | Description |
|----------|---------|-------------|
| `DATABASE_URL` | -- | PostgreSQL connection string. Without it, spend events log to stdout only |
| `REDIS_URL` | -- | Redis connection string. Without it, replay protection uses in-memory LRU |

### Security

| Variable | Default | Description |
|----------|---------|-------------|
| `RCR_ADMIN_TOKEN` | -- | Bearer token for admin endpoints (`/metrics`, `/v1/escrow/health`, `/v1/services/register`) |
| `RCR_CORS_ORIGINS` | -- | Comma-separated allowed CORS origins for browser clients |
| `RCR_SESSION_SECRET` | (generated) | HMAC secret for session token signing. Auto-generated if not set |

### Service Health

| Variable | Default | Description |
|----------|---------|-------------|
| `RCR_SERVICE_HEALTH_INTERVAL_SECS` | `60` | Interval between service health probe cycles |

## Config Files

### `config/default.toml`

Server host/port and Solana RPC defaults. These are overridden by environment variables with the `RCR_` prefix.

### `config/models.toml`

Model registry with per-token pricing for all providers. Loaded by the router crate at startup.

#### Model Pricing Table

All prices are provider cost per million tokens. The 5% platform fee is applied automatically on top.

| Model | Provider | Input $/M | Output $/M | Context | Reasoning | Streaming |
|-------|----------|-----------|------------|---------|-----------|-----------|
| `gpt-5.2` | OpenAI | 1.75 | 14.00 | 400K | Yes | Yes |
| `gpt-4o` | OpenAI | 2.50 | 10.00 | 128K | -- | Yes |
| `gpt-4o-mini` | OpenAI | 0.15 | 0.60 | 128K | -- | Yes |
| `o3` | OpenAI | 2.00 | 8.00 | 200K | Yes | Yes |
| `o3-mini` | OpenAI | 1.10 | 4.40 | 200K | Yes | Yes |
| `o4-mini` | OpenAI | 1.10 | 4.40 | 200K | Yes | Yes |
| `gpt-4.1` | OpenAI | 2.00 | 8.00 | 1M | -- | Yes |
| `gpt-4.1-mini` | OpenAI | 0.40 | 1.60 | 1M | -- | Yes |
| `gpt-4.1-nano` | OpenAI | 0.10 | 0.40 | 1M | -- | Yes |
| `gpt-oss-120b` | OpenAI | 0.00 | 0.00 | 128K | -- | Yes |
| `claude-opus-4.6` | Anthropic | 5.00 | 25.00 | 200K | Yes | Yes |
| `claude-sonnet-4.6` | Anthropic | 3.00 | 15.00 | 200K | Yes | Yes |
| `claude-sonnet-4.5` | Anthropic | 3.00 | 15.00 | 200K | -- | Yes |
| `claude-haiku-4.5` | Anthropic | 1.00 | 5.00 | 200K | -- | Yes |
| `gemini-3.1-pro` | Google | 2.00 | 12.00 | 1M | Yes | Yes |
| `gemini-2.5-flash` | Google | 0.30 | 2.50 | 1M | -- | Yes |
| `gemini-2.5-flash-lite` | Google | 0.10 | 0.40 | 1M | -- | Yes |
| `gemini-2.0-flash` | Google | 0.10 | 0.40 | 1M | -- | Yes |
| `gemini-2.0-flash-lite` | Google | 0.075 | 0.30 | 1M | -- | Yes |
| `deepseek-chat` | DeepSeek | 0.28 | 0.42 | 128K | -- | Yes |
| `deepseek-reasoner` | DeepSeek | 0.28 | 0.42 | 128K | Yes | Yes |
| `deepseek-coder` | DeepSeek | 0.28 | 0.42 | 128K | -- | Yes |
| `grok-4-fast-reasoning` | xAI | 0.20 | 0.50 | 2M | Yes | Yes |
| `grok-code-fast-1` | xAI | 0.20 | 1.50 | 256K | -- | Yes |
| `grok-3` | xAI | 3.00 | 15.00 | 131K | -- | Yes |
| `grok-3-mini` | xAI | 0.30 | 0.50 | 131K | Yes | Yes |

### `config/services.toml`

Service marketplace registry. Defines both internal gateway services and external x402-compatible endpoints:

```toml
[services.llm-gateway]
name = "LLM Intelligence"
endpoint = "/v1/chat/completions"
category = "intelligence"
x402_enabled = true
internal = true
description = "OpenAI-compatible LLM inference with smart routing"
pricing_label = "per-token (see /pricing)"

[services.web-search]
name = "Web Search"
endpoint = "https://search.example.com/v1/query"
category = "search"
x402_enabled = true
internal = false
price_per_request_usdc = 0.005
```

- `internal = true` -- service is hosted by the gateway (no external proxy)
- `internal = false` -- gateway proxies requests to the external endpoint with a 5% platform fee
