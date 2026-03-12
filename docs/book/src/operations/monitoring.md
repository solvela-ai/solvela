# Monitoring

RustyClawRouter exposes Prometheus metrics at `GET /metrics` (admin-gated). This chapter covers the full metrics reference, Grafana dashboard suggestions, and alerting rules.

## Prometheus Setup

### Scrape Configuration

Add to your `prometheus.yml`:

```yaml
scrape_configs:
  - job_name: 'rustyclawrouter'
    scrape_interval: 15s
    scheme: https
    bearer_token: '<RCR_ADMIN_TOKEN>'
    static_configs:
      - targets: ['rustyclawrouter-gateway.fly.dev']
    metrics_path: '/metrics'
```

For local development:

```yaml
scrape_configs:
  - job_name: 'rustyclawrouter-local'
    scrape_interval: 5s
    bearer_token: '<RCR_ADMIN_TOKEN>'
    static_configs:
      - targets: ['localhost:8402']
```

## Metrics Reference

### Request Metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `rcr_requests_total` | Counter | `method`, `path`, `status` | Total HTTP requests. Excludes `/metrics` to avoid scrape feedback loops. |
| `rcr_request_duration_seconds` | Histogram | `method`, `path` | End-to-end request processing time including provider latency. |
| `rcr_active_requests` | Gauge | -- | Number of currently in-flight requests. Uses a drop guard for safety. |

### Payment Metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `rcr_payments_total` | Counter | `status` | Payment outcomes. Status values: `verified` (valid payment), `cached` (cached response), `free` (free model), `none` (402 returned), `failed` (verification failed). |
| `rcr_payment_amount_usdc` | Histogram | -- | Distribution of payment amounts in USDC. |
| `rcr_replay_rejections_total` | Counter | -- | Number of rejected replay attacks. |

### Provider Metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `rcr_provider_request_duration_seconds` | Histogram | `provider` | Upstream provider response time. Provider values: `openai`, `anthropic`, `google`, `xai`, `deepseek`. |
| `rcr_provider_errors_total` | Counter | `provider`, `error_type` | Provider errors. Error types: `timeout`, `auth`, `rate_limit`, `server_error`, `unknown`. |

### Cache Metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `rcr_cache_total` | Counter | `result` | Cache outcomes. Result values: `hit` (served from cache), `miss` (not cached), `skip` (caching disabled or streaming). |

### Escrow Metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `rcr_escrow_claims_total` | Counter | `result` | Claim settlement outcomes. Result values: `success`, `failure`. |
| `rcr_escrow_queue_depth` | Gauge | -- | Number of pending claims waiting to be processed. |

### Infrastructure Metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `rcr_fee_payer_balance_sol` | Gauge | `pubkey` | SOL balance of each fee payer wallet. Monitors tx fee funding. |
| `rcr_service_health` | Gauge | `service_id` | External service health. 1.0 = healthy, 0.0 = unhealthy. |

## Grafana Dashboard Suggestions

### Overview Panel

- **Request rate**: `rate(rcr_requests_total[5m])` by `status`
- **Error rate**: `rate(rcr_requests_total{status=~"5.."}[5m]) / rate(rcr_requests_total[5m])`
- **Active requests**: `rcr_active_requests`
- **P95 latency**: `histogram_quantile(0.95, rate(rcr_request_duration_seconds_bucket[5m]))`

### Payment Panel

- **Payment success rate**: `rate(rcr_payments_total{status="verified"}[5m]) / rate(rcr_payments_total[5m])`
- **Revenue (USDC/min)**: `rate(rcr_payment_amount_usdc_sum[5m]) * 60`
- **402 rate**: `rate(rcr_payments_total{status="none"}[5m])`
- **Replay rejections**: `rate(rcr_replay_rejections_total[5m])`

### Provider Panel

- **Provider latency by provider**: `histogram_quantile(0.95, rate(rcr_provider_request_duration_seconds_bucket[5m])) by (provider)`
- **Provider error rate**: `rate(rcr_provider_errors_total[5m]) by (provider, error_type)`
- **Cache hit rate**: `rate(rcr_cache_total{result="hit"}[5m]) / rate(rcr_cache_total[5m])`

### Escrow Panel

- **Claim queue depth**: `rcr_escrow_queue_depth`
- **Claim success rate**: `rate(rcr_escrow_claims_total{result="success"}[5m]) / rate(rcr_escrow_claims_total[5m])`
- **Fee payer balances**: `rcr_fee_payer_balance_sol` by `pubkey`

### Service Health Panel

- **Service health**: `rcr_service_health` by `service_id` (1 = up, 0 = down)

## Alerting Rules

Example Prometheus alerting rules:

```yaml
groups:
  - name: rustyclawrouter
    rules:
      - alert: HighErrorRate
        expr: |
          rate(rcr_requests_total{status=~"5.."}[5m])
          / rate(rcr_requests_total[5m]) > 0.05
        for: 5m
        labels:
          severity: critical
        annotations:
          summary: "Error rate above 5% for 5 minutes"

      - alert: HighLatency
        expr: |
          histogram_quantile(0.95,
            rate(rcr_request_duration_seconds_bucket[5m])
          ) > 10
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "P95 latency above 10 seconds"

      - alert: EscrowQueueBacklog
        expr: rcr_escrow_queue_depth > 100
        for: 10m
        labels:
          severity: warning
        annotations:
          summary: "Escrow claim queue depth above 100"

      - alert: FeePayerLowBalance
        expr: rcr_fee_payer_balance_sol < 0.1
        for: 5m
        labels:
          severity: critical
        annotations:
          summary: "Fee payer SOL balance below 0.1"

      - alert: ProviderDown
        expr: |
          rate(rcr_provider_errors_total{error_type="server_error"}[5m]) > 0.5
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "Provider returning server errors"

      - alert: ReplayAttackSpike
        expr: rate(rcr_replay_rejections_total[5m]) > 1
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "Elevated replay attack attempts"

      - alert: ServiceUnhealthy
        expr: rcr_service_health == 0
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "External service health check failing"
```
