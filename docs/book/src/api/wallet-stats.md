# Wallet Stats

`GET /v1/wallet/{address}/stats`

Returns aggregated spend statistics for a specific wallet over a configurable time period. Requires session authentication.

## Request

```bash
curl -s http://localhost:8402/v1/wallet/7YkAzWalletPubkey.../stats?days=30 \
  -H "Authorization: Bearer <hmac-session-token>" | jq
```

### Path Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `address` | `string` | Solana wallet address (base58, 32-44 characters) |

### Query Parameters

| Parameter | Default | Range | Description |
|-----------|---------|-------|-------------|
| `days` | `30` | 1--365 | Number of days to include in the stats period |

### Authentication

The endpoint requires a valid session token in the `Authorization: Bearer <token>` header. The gateway issues this token in the `x-solvela-session` response header (also mirrored to the legacy `x-rcr-session` header) after a successful paid chat completion -- clients pass it back on subsequent stats requests. The token is HMAC-signed with the gateway's `SOLVELA_SESSION_SECRET` (legacy `RCR_SESSION_SECRET` accepted as a fallback). The SDKs handle token acquisition automatically.

## Response

```json
{
  "wallet": "7YkAzWalletPubkey...",
  "period_days": 30,
  "summary": {
    "total_requests": 847,
    "total_cost_usdc": "2.143750",
    "total_input_tokens": 125430,
    "total_output_tokens": 412890
  },
  "by_model": [
    {
      "model": "openai/gpt-4o",
      "requests": 312,
      "cost_usdc": "1.287500",
      "input_tokens": 48200,
      "output_tokens": 187600
    },
    {
      "model": "anthropic/claude-sonnet-4.6",
      "requests": 215,
      "cost_usdc": "0.643125",
      "input_tokens": 32100,
      "output_tokens": 98700
    },
    {
      "model": "google/gemini-2.5-flash",
      "requests": 320,
      "cost_usdc": "0.213125",
      "input_tokens": 45130,
      "output_tokens": 126590
    }
  ],
  "by_day": [
    {
      "date": "2026-03-12",
      "requests": 42,
      "cost_usdc": "0.087500"
    },
    {
      "date": "2026-03-11",
      "requests": 38,
      "cost_usdc": "0.072300"
    }
  ]
}
```

### Response Fields

| Field | Type | Description |
|-------|------|-------------|
| `wallet` | `string` | The queried wallet address |
| `period_days` | `integer` | The time period in days |
| `summary.total_requests` | `integer` | Total number of paid requests |
| `summary.total_cost_usdc` | `string` | Total USDC spent (decimal string) |
| `summary.total_input_tokens` | `integer` | Total input tokens across all requests |
| `summary.total_output_tokens` | `integer` | Total output tokens across all requests |
| `by_model[].model` | `string` | Model identifier |
| `by_model[].requests` | `integer` | Request count for this model |
| `by_model[].cost_usdc` | `string` | Total cost for this model |
| `by_day[].date` | `string` | Date in `YYYY-MM-DD` format |
| `by_day[].requests` | `integer` | Request count for this day |
| `by_day[].cost_usdc` | `string` | Total cost for this day |

## Error Cases

| Status | Cause |
|--------|-------|
| 400 | Invalid wallet address format (not base58, wrong length) |
| 401 | Missing or invalid `Authorization: Bearer` token |
| 400 | `days` parameter out of range (< 1 or > 365) |
| 503 | PostgreSQL not configured (stats require database) |

## Requirements

This endpoint requires PostgreSQL (`DATABASE_URL`) to be configured. Without a database, the endpoint returns 503.

The session token is verified using HMAC-SHA256 with the `SOLVELA_SESSION_SECRET` (legacy `RCR_SESSION_SECRET` accepted as a fallback). If no secret is configured, the gateway generates an ephemeral one at startup (it changes on restart, invalidating all existing tokens).
