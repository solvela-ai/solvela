# Chat Completions

`POST /v1/chat/completions`

OpenAI-compatible chat completion endpoint with x402 payment. Supports both JSON and SSE streaming responses.

## Request

```bash
curl -X POST http://localhost:8402/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "PAYMENT-SIGNATURE: <base64-encoded-payment>" \
  -d '{
    "model": "openai/gpt-4o",
    "messages": [
      {"role": "system", "content": "You are a helpful assistant."},
      {"role": "user", "content": "Explain the x402 protocol in one paragraph."}
    ],
    "max_tokens": 500,
    "temperature": 0.7,
    "stream": false
  }'
```

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `model` | `string` | Yes | Model ID, alias, or routing profile |
| `messages` | `ChatMessage[]` | Yes | Conversation messages |
| `max_tokens` | `integer` | No | Maximum output tokens (capped at 128,000) |
| `temperature` | `float` | No | Sampling temperature (0.0--2.0) |
| `stream` | `boolean` | No | Enable SSE streaming (default: `false`) |
| `tools` | `Tool[]` | No | Tool/function definitions (OpenAI format) |

### ChatMessage

| Field | Type | Description |
|-------|------|-------------|
| `role` | `string` | `"system"`, `"user"`, `"assistant"`, or `"tool"` |
| `content` | `string` | Message content |

### Model Resolution

The `model` field accepts:

- **Direct ID**: `"openai/gpt-4o"`, `"anthropic/claude-sonnet-4.6"`, `"deepseek/deepseek-chat"`
- **Profile alias**: `"auto"`, `"eco"`, `"premium"`, `"free"`, `"cheap"`, `"best"`, `"budget"`, `"quality"`, `"oss"`, `"open"`, `"balanced"`, `"default"`
- **Short alias**: `"fast"`, `"smart"`, `"reason"`, `"code"`, `"creative"`, `"analyze"`

### Headers

| Header | Required | Description |
|--------|----------|-------------|
| `Content-Type` | Yes | Must be `application/json` |
| `PAYMENT-SIGNATURE` | No | Signed payment payload. Omit to get a 402 price quote. |
| `X-Request-Id` | No | Client-provided request ID (UUID format). Server generates one if absent. |
| `X-Session-Id` | No | Session identifier for spend tracking. Max 128 chars, `[a-zA-Z0-9\-_]` only. |
| `X-Solvela-Debug` | No | Set to `true` to receive debug response headers. (`X-RCR-Debug` accepted as deprecated alias.) |

## Response (JSON)

```json
{
  "id": "chatcmpl-abc123",
  "object": "chat.completion",
  "created": 1710000000,
  "model": "openai/gpt-4o",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "The x402 protocol is an HTTP-native payment mechanism..."
      },
      "finish_reason": "stop"
    }
  ],
  "usage": {
    "prompt_tokens": 25,
    "completion_tokens": 87,
    "total_tokens": 112
  }
}
```

## Response (SSE Streaming)

When `stream: true`, the response is delivered as server-sent events:

```
data: {"id":"chatcmpl-abc123","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":"The"},"finish_reason":null}]}

data: {"id":"chatcmpl-abc123","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":" x402"},"finish_reason":null}]}

: heartbeat

data: {"id":"chatcmpl-abc123","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":" protocol"},"finish_reason":null}]}

data: {"id":"chatcmpl-abc123","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}

data: [DONE]
```

Heartbeat comments (`: heartbeat`) are sent periodically to keep the connection alive through proxies and load balancers.

## 402 Payment Required

When `PAYMENT-SIGNATURE` is absent:

```json
{
  "error": "payment_required",
  "payment_required": {
    "recipient_wallet": "7YkAz...",
    "usdc_mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
    "amount_usdc": "0.006563",
    "cost_breakdown": {
      "input_tokens_estimated": 150,
      "output_tokens_max": 500,
      "input_cost_usdc": "0.000375",
      "output_cost_usdc": "0.005000",
      "platform_fee_usdc": "0.000269",
      "platform_fee_percent": 5,
      "total_usdc": "0.006563"
    },
    "accepted_schemes": ["exact"],
    "chain": "solana",
    "network": "devnet"
  }
}
```

## Error Codes

| Status | Error | Description |
|--------|-------|-------------|
| 400 | `bad_request` | Invalid JSON, unknown model, malformed payment header, session ID too long |
| 401 | `unauthorized` | Payment signature verification failed |
| 402 | `payment_required` | No payment header provided; response includes price quote |
| 429 | `rate_limited` | Per-wallet rate limit exceeded |
| 500 | `internal_error` | Unexpected server error |
| 503 | `service_unavailable` | Provider unavailable (no API key configured or circuit breaker open) |

## Debug Headers

When `X-Solvela-Debug: true` is sent, the response includes additional headers (each canonical `X-Solvela-*` header is also emitted as a deprecated `X-RCR-*` alias for backward compatibility):

| Header | Description |
|--------|-------------|
| `X-Solvela-Request-Id` | Unique request identifier (always present, not gated by debug) |
| `X-Solvela-Model` | The actual model used after resolution |
| `X-Solvela-Profile` | Routing profile used (if smart-routed) |
| `X-Solvela-Tier` | Complexity tier (if smart-routed) |
| `X-Solvela-Score` | Raw scorer output (if smart-routed) |
| `X-Solvela-Provider` | Provider that handled the request |
| `X-Solvela-Cache` | `hit`, `miss`, or `skip` |
| `X-Solvela-Payment-Status` | `verified`, `cached`, `free`, `none`, or `failed` |
| `X-Solvela-Token-Estimate-In` | Estimated input token count |
| `X-Solvela-Token-Estimate-Out` | Estimated max output token count |
| `X-Solvela-Latency-Ms` | Request processing time in milliseconds |
| `X-Solvela-Session` | Session identifier (echoed back when `X-Session-Id` is sent) |
| `X-Solvela-Fallback` | Set when a free-tier fallback fired |
