# Health and Metrics

## Health Check

`GET /health`

Public endpoint. Returns gateway status.

```bash
curl -s http://localhost:8402/health | jq
```

```json
{
  "status": "ok"
}
```

No authentication required. Used by load balancers, health probes, and the `rcr doctor` command.

## Prometheus Metrics

`GET /metrics`

Returns Prometheus text exposition format. Admin-gated via `RCR_ADMIN_TOKEN`.

```bash
curl -s http://localhost:8402/metrics \
  -H "Authorization: Bearer <RCR_ADMIN_TOKEN>"
```

```
# HELP rcr_requests_total Total HTTP requests
# TYPE rcr_requests_total counter
rcr_requests_total{method="POST",path="/v1/chat/completions",status="200"} 1247
rcr_requests_total{method="POST",path="/v1/chat/completions",status="402"} 892
rcr_requests_total{method="GET",path="/health",status="200"} 4521

# HELP rcr_request_duration_seconds Request processing time
# TYPE rcr_request_duration_seconds histogram
rcr_request_duration_seconds_bucket{method="POST",path="/v1/chat/completions",le="0.5"} 312
...
```

### All Metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `rcr_requests_total` | Counter | `method`, `path`, `status` | Total HTTP requests (excludes `/metrics`) |
| `rcr_request_duration_seconds` | Histogram | `method`, `path` | Request processing time |
| `rcr_active_requests` | Gauge | -- | Currently in-flight requests |
| `rcr_payments_total` | Counter | `status` | Payment outcomes (`verified`, `cached`, `free`, `none`, `failed`) |
| `rcr_payment_amount_usdc` | Histogram | -- | Payment amounts in USDC |
| `rcr_replay_rejections_total` | Counter | -- | Replay attack rejections |
| `rcr_provider_request_duration_seconds` | Histogram | `provider` | Upstream provider latency |
| `rcr_provider_errors_total` | Counter | `provider`, `error_type` | Provider errors by type (`timeout`, `auth`, `rate_limit`, `server_error`, `unknown`) |
| `rcr_cache_total` | Counter | `result` | Cache outcomes (`hit`, `miss`, `skip`) |
| `rcr_escrow_claims_total` | Counter | `result` | Escrow claim outcomes (`success`, `failure`) |
| `rcr_escrow_queue_depth` | Gauge | -- | Pending escrow claims in queue |
| `rcr_fee_payer_balance_sol` | Gauge | `pubkey` | Fee payer SOL balance (for monitoring tx fee funding) |
| `rcr_service_health` | Gauge | `service_id` | Service health status (1.0 = healthy, 0.0 = unhealthy) |

```admonish tip
All metric names use the `rcr_` prefix to avoid collisions with other exporters. This matches the `RCR_` prefix convention used for environment variables.
```

## Escrow Config

`GET /v1/escrow/config`

Public endpoint. Returns escrow program configuration for client discovery.

```bash
curl -s http://localhost:8402/v1/escrow/config | jq
```

```json
{
  "program_id": "GTs7ik3NbW3xwSXq33jyVRGgmshNEyW1h9rxDNATiFLy",
  "current_slot": 298451623,
  "usdc_mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
}
```

Returns 404 when escrow is not configured.

The `current_slot` is cached with a 5-second TTL to reduce RPC calls.

## Escrow Health

`GET /v1/escrow/health`

Admin-gated. Returns claim processor operational metrics.

```bash
curl -s http://localhost:8402/v1/escrow/health \
  -H "Authorization: Bearer <RCR_ADMIN_TOKEN>" | jq
```

```json
{
  "claims_submitted": 1250,
  "claims_succeeded": 1230,
  "claims_failed": 15,
  "claims_retried": 42,
  "queue_depth": 3,
  "circuit_breaker_open": false,
  "fee_payers_healthy": 2,
  "fee_payers_total": 3
}
```

| Field | Description |
|-------|-------------|
| `claims_submitted` | Total claims queued since startup |
| `claims_succeeded` | Successfully settled claims |
| `claims_failed` | Claims that exhausted all retries |
| `claims_retried` | Total retry attempts across all claims |
| `queue_depth` | Claims currently waiting to be processed |
| `circuit_breaker_open` | `true` when claiming is paused due to high failure rate |
| `fee_payers_healthy` | Number of fee payer keys in healthy state |
| `fee_payers_total` | Total configured fee payer keys |

## Nonce Endpoint

`GET /v1/nonce`

Returns a fresh nonce for client-side transaction construction. Used by SDKs to build durable nonce transactions.

```bash
curl -s http://localhost:8402/v1/nonce | jq
```

## Admin Authentication

The `/metrics` and `/v1/escrow/health` endpoints require the `Authorization: Bearer <token>` header where `<token>` matches the `RCR_ADMIN_TOKEN` environment variable. Token comparison uses constant-time equality to prevent timing attacks.

If `RCR_ADMIN_TOKEN` is not set, admin endpoints return 403.
