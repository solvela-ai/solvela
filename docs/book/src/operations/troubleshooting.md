# Troubleshooting

## Common Issues

| Symptom | Cause | Fix |
|---------|-------|-----|
| Gateway returns 503 for all chat requests | No provider API key configured | Set at least one provider key in `.env` (e.g., `OPENAI_API_KEY`) |
| 402 response but SDK does not auto-pay | Wallet key not configured | Set `SOLANA_WALLET_KEY` environment variable |
| "payment_required" with `amount_usdc: "0.000000"` | Using a free model (`gpt-oss-120b`) | Free models do not require payment; remove the `PAYMENT-SIGNATURE` header |
| Redis connection refused | Redis not running | Run `docker compose up -d redis` |
| "replay detected" error | Same transaction signature sent twice | Generate a new transaction for each request |
| "insufficient amount" error | Payment amount less than quoted price | Use the `amount_usdc` from the 402 response |
| "recipient mismatch" error | Payment sent to wrong wallet | Send to the `recipient_wallet` from the 402 response |
| 429 Too Many Requests | Per-wallet rate limit exceeded | Wait for the `Retry-After` period, then retry |
| 500 Internal Server Error on chat | Provider returned unexpected response | Check `RUST_LOG=gateway=debug` for details |
| "unknown model" error | Model ID not in registry | Use `GET /v1/models` to list valid model IDs |
| Escrow claim failures | Fee payer SOL balance too low | Fund fee payer wallets; check `GET /v1/escrow/health` |
| Metrics endpoint returns 403 | `RCR_ADMIN_TOKEN` not set or wrong token | Set `RCR_ADMIN_TOKEN` and use `Authorization: Bearer <token>` |
| Wallet stats returns 503 | PostgreSQL not configured | Set `DATABASE_URL` in `.env` |
| CORS errors in browser | Origin not in allowlist | Add your domain to `RCR_CORS_ORIGINS` |
| Slow responses (>10s) | Provider latency or network issues | Check `rcr_provider_request_duration_seconds` metric; try a different provider |
| "circuit breaker open" in escrow health | >50% claim failures in 5-minute window | Wait 1 minute for auto-reset; check fee payer balance and RPC connectivity |

## Debug Headers

Enable debug headers to diagnose routing and payment issues:

```bash
curl -X POST http://localhost:8402/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "X-RCR-Debug: true" \
  -H "PAYMENT-SIGNATURE: <payment>" \
  -d '{"model": "auto", "messages": [{"role": "user", "content": "Hello"}]}'
```

The response includes:

```
X-RCR-Request-Id: 550e8400-e29b-41d4-a716-446655440000
X-RCR-Model-Requested: auto
X-RCR-Model-Resolved: google/gemini-2.5-flash
X-RCR-Route-Profile: auto
X-RCR-Route-Tier: simple
X-RCR-Route-Score: -0.3500
X-RCR-Provider: google
X-RCR-Cache-Status: miss
X-RCR-Payment-Status: verified
X-RCR-Token-Estimate: 10
X-RCR-Duration-Ms: 1247
```

`X-RCR-Request-Id` is always returned (not gated by the debug flag).

## Health Check Interpretation

```bash
curl -s http://localhost:8402/health | jq
```

| Response | Meaning |
|----------|---------|
| `{"status": "ok"}` | Gateway is running and ready to serve requests |
| Connection refused | Gateway process is not running |
| Timeout | Gateway is starting up or overloaded |

For deeper diagnostics, check the escrow health endpoint:

```bash
curl -s http://localhost:8402/v1/escrow/health \
  -H "Authorization: Bearer <admin-token>" | jq
```

Key fields to check:

- `claims_failed > 0`: some claims are failing; check fee payer balance
- `circuit_breaker_open: true`: claiming is paused; check RPC connectivity
- `queue_depth > 50`: claim processing is falling behind
- `fee_payers_healthy < fee_payers_total`: some fee payer keys are in cooldown

## CLI Diagnostics

The `rcr doctor` command runs a comprehensive check:

```bash
cargo run -p cli -- doctor
```

Checks performed:

1. **Wallet loaded** -- verifies `SOLANA_WALLET_KEY` is valid
2. **Gateway reachable** -- connects to the gateway's `/health` endpoint
3. **Models available** -- fetches model list from `/v1/models`
4. **RPC connected** -- verifies Solana RPC connectivity
5. **Balance check** -- queries USDC-SPL balance
6. **Payment flow** -- sends a test request and verifies the 402/200 cycle

Each check reports PASS, FAIL, WARN, or SKIP.

## Log Levels

Set via `RUST_LOG`:

```bash
# Recommended for production
RUST_LOG=gateway=info,tower_http=info

# For debugging request flow
RUST_LOG=gateway=debug,tower_http=debug

# For debugging payment verification
RUST_LOG=gateway=debug,x402=debug

# For debugging smart routing
RUST_LOG=gateway=debug,router=debug

# Maximum verbosity (noisy)
RUST_LOG=debug
```

The gateway uses structured logging via `tracing`:

```
INFO gateway::routes::chat: processing request wallet=7YkAz... model=openai/gpt-4o
INFO gateway::providers::openai: forwarding to OpenAI model=gpt-4o tokens_est=150
INFO gateway::usage: spend logged wallet=7YkAz... cost_usdc=0.006563 model=openai/gpt-4o
```

## Database Troubleshooting

If PostgreSQL-dependent features (stats, spend logging) are not working:

```bash
# Check PostgreSQL is running
docker compose ps postgres

# Check connectivity
psql postgres://rcr:rcr_dev_password@localhost:5432/solvela -c "SELECT 1"

# Check migrations ran
psql postgres://rcr:rcr_dev_password@localhost:5432/solvela \
  -c "\dt"

# Check spend logs
psql postgres://rcr:rcr_dev_password@localhost:5432/solvela \
  -c "SELECT COUNT(*) FROM spend_log"
```

## Redis Troubleshooting

If caching or replay protection is not working:

```bash
# Check Redis is running
docker compose ps redis

# Check connectivity
redis-cli -h localhost ping

# Check cache keys
redis-cli -h localhost keys "rcr:*" | head -20

# Check memory usage
redis-cli -h localhost info memory | grep used_memory_human
```
