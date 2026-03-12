# Services

The service marketplace allows external x402-compatible services to register with the gateway and be accessible through a unified proxy endpoint.

## List Services

`GET /v1/services`

Returns all registered services with health status.

```bash
curl -s http://localhost:8402/v1/services | jq
```

```json
{
  "services": [
    {
      "id": "llm-gateway",
      "name": "LLM Intelligence",
      "endpoint": "/v1/chat/completions",
      "category": "intelligence",
      "x402_enabled": true,
      "internal": true,
      "healthy": true,
      "description": "OpenAI-compatible LLM inference with 15-dimension smart routing across 5+ providers",
      "pricing_label": "per-token (see /pricing)"
    },
    {
      "id": "web-search",
      "name": "Web Search",
      "endpoint": "https://search.example.com/v1/query",
      "category": "search",
      "x402_enabled": true,
      "internal": false,
      "healthy": true,
      "description": "Real-time web search with structured results",
      "pricing_label": "$0.005/query",
      "price_per_request_usdc": 0.005
    }
  ]
}
```

### Service Fields

| Field | Type | Description |
|-------|------|-------------|
| `id` | `string` | Unique service identifier |
| `name` | `string` | Human-readable name |
| `endpoint` | `string` | Service endpoint (path for internal, URL for external) |
| `category` | `string` | Service category |
| `x402_enabled` | `boolean` | Whether the service requires x402 payment |
| `internal` | `boolean` | `true` if hosted by the gateway |
| `healthy` | `boolean` | Current health status (probed every 60s) |
| `description` | `string` | Service description |
| `pricing_label` | `string` | Human-readable pricing |
| `price_per_request_usdc` | `float` | USDC cost per request (external services) |

## Register Service

`POST /v1/services/register`

Registers a new external service. Requires admin authentication.

```bash
curl -X POST http://localhost:8402/v1/services/register \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer <RCR_ADMIN_TOKEN>" \
  -d '{
    "id": "translation-api",
    "name": "Translation Service",
    "endpoint": "https://translate.example.com/v1/translate",
    "category": "language",
    "x402_enabled": true,
    "description": "Real-time translation across 100+ languages",
    "price_per_request_usdc": 0.002
  }'
```

### Validation Rules

- `id`: alphanumeric + hyphens only, must be unique
- `endpoint`: must use HTTPS (no HTTP endpoints allowed)
- Uniqueness: `id` must not already exist in the registry
- SSRF prevention: endpoint hostname is validated against private network ranges at registration time

### Response

```json
{
  "status": "registered",
  "service_id": "translation-api"
}
```

Status code: **201 Created**

## Proxy Service

`POST /v1/services/{service_id}/proxy`

Proxies an arbitrary JSON request to an external service with x402 payment verification.

```bash
curl -X POST http://localhost:8402/v1/services/web-search/proxy \
  -H "Content-Type: application/json" \
  -H "PAYMENT-SIGNATURE: <signed-payment>" \
  -d '{
    "query": "latest Solana ecosystem news",
    "max_results": 10
  }'
```

### How It Works

1. Looks up the service by `service_id` in the registry
2. Verifies the x402 payment (including 5% platform fee)
3. Validates the target endpoint against SSRF (private network filtering)
4. Forwards the request body to the external service with a 60-second timeout
5. Returns the external service's response to the client

### Error Cases

| Status | Cause |
|--------|-------|
| 402 | No payment; returns cost quote based on `price_per_request_usdc` |
| 404 | Service ID not found in registry |
| 502 | External service returned an error |
| 504 | External service timed out (>60s) |

## Health Monitoring

The gateway runs a background health checker that:

- Probes all external services every 60 seconds (configurable via `RCR_SERVICE_HEALTH_INTERVAL_SECS`)
- Sends concurrent `HEAD` requests to each service endpoint
- Considers 2xx, 402, and 405 responses as "healthy" (402 means the service is up but requires payment; 405 means the server is running)
- Updates the `healthy` field in the service registry
- Shuts down gracefully via a `watch` channel on SIGTERM
