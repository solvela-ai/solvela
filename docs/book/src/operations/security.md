# Security

## Security Model

RustyClawRouter operates in a zero-trust model: every request must prove payment before receiving service. The gateway never stores private keys, never trusts client-provided data without verification, and degrades safely when dependencies are unavailable.

## Payment Verification

### Transaction Verification Pipeline

Every `PAYMENT-SIGNATURE` header is validated through a multi-step pipeline:

1. **Header size limit**: 50KB maximum to prevent DoS via oversized headers
2. **Decoding**: base64 or raw JSON parsing
3. **Replay protection**: Redis `SET NX EX 120` or in-memory LRU fallback
4. **Transaction deserialization**: parse as Solana `VersionedTransaction`
5. **Instruction validation**: verify `TransferChecked` discriminator
6. **ATA derivation**: compute expected Associated Token Accounts
7. **Amount validation**: transferred amount >= quoted price
8. **Recipient validation**: destination matches gateway wallet USDC ATA
9. **USDC mint validation**: token mint matches configured USDC mint

### Amount Bypass Prevention

The gateway computes costs using integer arithmetic on USDC atomic units (6 decimal places) to prevent floating-point rounding exploits. The `compute_actual_atomic_cost` function ensures the actual amount charged matches what was quoted.

## Replay Protection

Transaction signatures are tracked to prevent double-spending:

- **Redis (primary)**: `SET NX EX 120` -- if the key already exists, the payment is rejected
- **In-memory LRU (fallback)**: bounded to 10,000 entries when Redis is unavailable
- The 120-second TTL exceeds Solana's ~60-second blockhash expiry window

```admonish warning
When Redis is unavailable, replay protection uses an in-memory LRU cache. This is a degraded mode -- it cannot protect against replays across gateway restarts or multi-instance deployments. Always use Redis in production.
```

## SSRF Prevention

The service marketplace validates endpoints against private network ranges:

- Registration time: endpoint hostname is resolved and checked against RFC 1918 / loopback / link-local ranges
- Proxy time: the target hostname is re-validated before forwarding
- Defense-in-depth: both layers must pass for a request to be proxied

Blocked ranges:

- `127.0.0.0/8` (loopback)
- `10.0.0.0/8` (private)
- `172.16.0.0/12` (private)
- `192.168.0.0/16` (private)
- `169.254.0.0/16` (link-local)
- `::1` (IPv6 loopback)
- `fc00::/7` (IPv6 unique local)

## Rate Limiting

Rate limiting is per-wallet (Solana pubkey), not per-IP:

- Extracts the wallet address from the payment header or request context
- Prevents spoofing via `X-Forwarded-For` (ignored entirely)
- Configurable limits per endpoint
- Cleanup cooldown prevents stale entries from consuming memory

When the rate limit is exceeded, the gateway returns **429 Too Many Requests** with a `Retry-After` header.

## Admin Endpoints

The following endpoints require `Authorization: Bearer <RCR_ADMIN_TOKEN>`:

| Endpoint | Purpose |
|----------|---------|
| `GET /metrics` | Prometheus metrics |
| `GET /v1/escrow/health` | Escrow claim processor status |
| `POST /v1/services/register` | Service marketplace registration |

Token comparison uses constant-time equality (`subtle::ConstantTimeEq` equivalent) to prevent timing attacks.

If `RCR_ADMIN_TOKEN` is not set, admin endpoints return 403.

## CORS

CORS is configured restrictively:

- **Development** (`RCR_ENV != "production"`): allows `localhost:3000`, `localhost:8080`, `127.0.0.1:3000`
- **Production** (`RCR_ENV=production`): only origins listed in `RCR_CORS_ORIGINS`
- **Allowed methods**: GET, POST, OPTIONS only
- **Allowed headers**: `Content-Type`, `Authorization`, `PAYMENT-SIGNATURE`, `X-Request-Id`, `X-RCR-Debug`, `X-Session-Id`

SDK and agent clients are unaffected by CORS since they do not run in browsers.

## Secret Management

### Rules

- All secrets come from environment variables, never config files
- API keys and fee payer keys are redacted in all `Debug` output via custom `fmt::Debug` implementations
- Fee payer private keys (`SolanaConfig.fee_payer_key`, `fee_payer_keys`) show `[REDACTED]` in logs
- Provider API keys (`ProvidersConfig`) show `[REDACTED]` when set, `None` when absent

### Key Rotation

Fee payer keys support rotation:

- Primary key: `RCR_SOLANA_FEE_PAYER_KEY`
- Additional keys: `RCR_SOLANA__FEE_PAYER_KEY_2` through `_8`
- The `FeePayerPool` rotates across healthy keys automatically
- Failed keys enter a 60-second cooldown before reuse

## Security Headers

Every response includes:

| Header | Value | Purpose |
|--------|-------|---------|
| `X-Content-Type-Options` | `nosniff` | Prevents MIME type sniffing |
| `X-Frame-Options` | `DENY` | Prevents clickjacking |
| `Referrer-Policy` | `no-referrer` | Prevents referrer leakage |
| `X-RCR-Request-Id` | UUID | Request correlation (always present) |

## Prompt Guard

The gateway includes middleware for detecting:

- **Prompt injection**: patterns that attempt to override system prompts
- **Jailbreak attempts**: patterns that bypass safety filters
- **PII exposure**: personal information in prompts

Flagged requests are rejected with 400 before any payment is processed.

## Input Validation

- `max_tokens` capped at 128,000 (prevents unbounded cost)
- Session IDs: max 128 characters, `[a-zA-Z0-9\-_]` only
- Wallet addresses: base58 character set, 32-44 characters
- Service IDs: alphanumeric + hyphens only
- Service endpoints: HTTPS required, no HTTP
- Request body: 10MB limit (Tower `RequestBodyLimitLayer`)
