<!-- Generated: 2026-05-27 | Files scanned: 45 | Token estimate: ~850 -->

# Backend Routes & Middleware Codemap

**Last Updated:** 2026-05-27  
**Entry:** `crates/gateway/src/lib.rs` (`build_router()` function)  
**Server:** `crates/gateway/src/main.rs` (Axum on port 8402)

## Route Tree

```
GET  /
GET  /.well-known/agent.json
GET  /health
POST /a2a

GET  /v1/models
GET  /v1/models/{model_id}
POST /v1/chat/completions         [x402 payment required]
POST /v1/images/generations       [x402 payment required]

GET  /v1/pricing
GET  /v1/services
GET  /v1/escrow/config
GET  /v1/escrow/claim/{tx_sig}

GET  /v1/nonce
POST /v1/nonce/reserve

GET  /debug/headers

GET  /metrics
GET  /admin/stats

POST /orgs
GET  /orgs/{org_id}
PATCH /orgs/{org_id}
DELETE /orgs/{org_id}                                    [RequireOrgAdmin]

POST /orgs/{org_id}/teams
GET  /orgs/{org_id}/teams
POST /orgs/{org_id}/teams/{team_id}/wallets             [RequireOrgAdmin]

POST /orgs/{org_id}/members
GET  /orgs/{org_id}/members
DELETE /orgs/{org_id}/members/{member_id}               [RequireOrgAdmin]

POST /orgs/{org_id}/api-keys                            [RequireOrgAdmin]
GET  /orgs/{org_id}/api-keys
DELETE /orgs/{org_id}/api-keys/{key_id}                 [RequireOrgAdmin]

POST /orgs/{org_id}/budgets
GET  /orgs/{org_id}/budgets
PUT  /orgs/{org_id}/budgets/{wallet}                    [RequireOrgAdmin]

GET  /orgs/{org_id}/analytics
GET  /orgs/{org_id}/audit-logs                          [RequireOrgAdmin]
```

## Public Routes

### GET /
Landing page (redirect or static content).

### GET /.well-known/agent.json
**Handler:** `a2a/agent_card.rs`  
**Response:** AgentCard with AP2 + x402 extensions.
```json
{
  "name": "Solvela Gateway",
  "protocols": {
    "ap2": { "version": "1.0", ... },
    "x402": { "payment_methods": [...] }
  }
}
```

### GET /health
**Handler:** `routes/health.rs`  
**Response:** `200 OK` with uptime, version.

### POST /a2a
**Handler:** `a2a/handler.rs`  
**Request:** JSON-RPC 2.0 (`message/send` method)  
**Flow:**
1. Parse JSON-RPC
2. First call: cost-only (returns Task with x402.payment.required)
3. Second call: with payment payload (verifies, proxies, returns Task completed)

---

## Chat Completions (x402 Payment Required)

### POST /v1/chat/completions
**Handler:** `routes/chat/mod.rs` (~776 lines)  
**Middleware:**
1. TraceLayer (request/response logging)
2. CorsLayer (CORS headers)
3. RateLimitLayer (per-wallet/org sliding window)
4. PromptGuardLayer (injection/jailbreak/PII detection)
5. RequestIdLayer (generates X-Request-ID)
6. X402Layer (extracts PAYMENT-SIGNATURE header)
7. MetricsLayer (Prometheus request count/latency)

**Request:**
```json
{
  "model": "gpt-4o",  // or "auto", "sonnet", "eco" (alias/profile)
  "messages": [...],
  "temperature": 0.7,
  "stream": false,
  "max_tokens": 1024
}
```

**Response Flow:**
1. **Parse & validate** — Check model exists, resolve alias/profile
2. **Cost calculation** — Token estimation + 5% platform fee (cost.rs)
3. **Payment check** — If no `PAYMENT-SIGNATURE` header:
   - Return **402 Payment Required**
   - Body: cost breakdown + accepted payment schemes (exact + escrow if configured)
4. **Verify payment** — Decode signature (base64 or JSON), verify via Facilitator
5. **Proxy to LLM** — Send to provider (OpenAI, Anthropic, Google, etc.)
6. **Cache response** — Store in Redis (model+messages+temp hash key)
7. **Log spend** — PostgreSQL fire-and-forget via `tokio::spawn`
8. **Claim escrow** — If escrow configured, fire claim task
9. **Return** — JSON or Server-Sent Events (SSE) stream

**Cost Breakdown:**
```json
{
  "model": "gpt-4o",
  "input_cost_usdc": 0.0025,
  "output_cost_usdc": 0.01,
  "platform_fee_usdc": 0.000625,  // 5% of total
  "total_usdc": 0.012625,
  "estimated_input_tokens": 50,
  "estimated_output_tokens": 100
}
```

**Handlers (chat/):**
- `mod.rs` — Main handler, request/response flow
- `cost.rs` — `estimate_cost()`, fee breakdown
- `payment.rs` — Payment verification, 402 response
- `provider.rs` — Provider selection, model routing
- `response.rs` — Response streaming, token counting

---

## Model Endpoints

### GET /v1/models
**Handler:** `routes/models.rs`  
**Response:** List all available models from ModelRegistry.

### GET /v1/models/{model_id}
**Handler:** `routes/models.rs`  
**Response:** Detailed model info (pricing, capabilities, context window).

### GET /v1/pricing
**Handler:** `routes/pricing.rs`  
**Response:** Per-token pricing table (input/output costs by model).

---

## Service Marketplace

### GET /v1/services
**Handler:** `routes/services.rs`  
**Response:** Available external services (from `config/services.toml`).

---

## Escrow Integration

### GET /v1/escrow/config
**Handler:** `routes/escrow.rs`  
**Response:** Escrow program config (PDA, fee payer, nonce accounts).

### GET /v1/escrow/claim/{tx_sig}
**Handler:** `routes/escrow.rs`  
**Response:** Claim status for a given transaction signature.

---

## Nonce Management

### GET /v1/nonce
**Handler:** `routes/nonce.rs`  
**Response:** Current nonce account state (for durable transactions).

### POST /v1/nonce/reserve
**Handler:** `routes/nonce.rs`  
**Response:** Reserve a nonce account for a client transaction.

---

## Debug Routes

### GET /debug/headers
**Handler:** `routes/debug_headers.rs`  
**Response:** Echo back request headers (for debugging payment encoding).

---

## Observability

### GET /metrics
**Handler:** `routes/metrics.rs`  
**Response:** Prometheus-formatted metrics (requests, latencies, errors).  
**Metrics tracked:**
- `http_requests_total{method, status}` — Request count
- `http_request_duration_seconds{method, status}` — Request latency
- `http_request_body_bytes{direction}` — Payload sizes
- `chat_completions_total{status}` — Chat request count
- `chat_completion_tokens_total{role}` — Token usage
- `payment_attempts{status}` — Payment verification attempts
- `provider_requests{provider, status}` — Provider call counts
- `escrow_claims{status}` — Escrow claim submissions

### GET /admin/stats
**Handler:** `routes/admin_stats.rs`  
**Requires:** `SOLVELA_ADMIN_TOKEN` header  
**Response:** System-wide statistics (uptime, memory, connections).

---

## Organization Management (API Key or Wallet Auth)

### POST /orgs
**Handler:** `routes/orgs/crud.rs`  
**Request:**
```json
{
  "name": "My AI Company",
  "slug": "my-ai-company"
}
```
**Response:** Organization object with ID, owner_wallet.

### GET /orgs/{org_id}
**Handler:** `routes/orgs/crud.rs`  
**Response:** Org details.

### PATCH /orgs/{org_id}
**Handler:** `routes/orgs/crud.rs`  
**Requires:** `RequireOrgAdmin` extractor  
**Request:** Partial org update (name, slug)

### DELETE /orgs/{org_id}
**Handler:** `routes/orgs/crud.rs`  
**Requires:** `RequireOrgAdmin`

---

## Teams

### POST /orgs/{org_id}/teams
**Handler:** `routes/orgs/teams.rs`  
**Requires:** `RequireOrgAdmin`  
**Request:**
```json
{
  "name": "Data Science"
}
```

### GET /orgs/{org_id}/teams
**Handler:** `routes/orgs/teams.rs`  
**Response:** List all teams in org.

### POST /orgs/{org_id}/teams/{team_id}/wallets
**Handler:** `routes/orgs/teams.rs`  
**Requires:** `RequireOrgAdmin`  
**Request:**
```json
{
  "wallet_address": "Hpq..."
}
```
**Purpose:** Associate wallet with team (for team-scoped budgets/usage).

---

## Members

### POST /orgs/{org_id}/members
**Handler:** `routes/orgs/crud.rs`  
**Requires:** `RequireOrgAdmin`  
**Request:**
```json
{
  "wallet_address": "Hpq...",
  "role": "member"  // or "admin", "owner"
}
```

### GET /orgs/{org_id}/members
**Handler:** `routes/orgs/crud.rs`  
**Response:** List all members (wallet, role, created_at).

### DELETE /orgs/{org_id}/members/{member_id}
**Handler:** `routes/orgs/crud.rs`  
**Requires:** `RequireOrgAdmin`

---

## API Keys

### POST /orgs/{org_id}/api-keys
**Handler:** `routes/orgs/api_keys.rs`  
**Requires:** `RequireOrgAdmin`  
**Request:**
```json
{
  "name": "Dashboard API Key",
  "role": "member",
  "expires_at": "2027-05-27T00:00:00Z"  // optional
}
```
**Response:**
```json
{
  "id": "uuid",
  "key": "solv_...",
  "key_prefix": "solv_1a2b3c...",
  "name": "...",
  "created_at": "..."
}
```
Note: Full key only shown once; store securely.

### GET /orgs/{org_id}/api-keys
**Handler:** `routes/orgs/api_keys.rs`  
**Response:** List API keys (without full key value).

### DELETE /orgs/{org_id}/api-keys/{key_id}
**Handler:** `routes/orgs/api_keys.rs`  
**Requires:** `RequireOrgAdmin`

---

## Budgets

### POST /orgs/{org_id}/budgets
**Handler:** `routes/orgs/budget.rs`  
**Request:**
```json
{
  "wallet_address": "Hpq...",
  "daily_limit_usdc": 100.0,
  "monthly_limit_usdc": 2000.0
}
```

### GET /orgs/{org_id}/budgets
**Handler:** `routes/orgs/budget.rs`  
**Response:** List all budgets for org.

### PUT /orgs/{org_id}/budgets/{wallet}
**Handler:** `routes/orgs/budget.rs`  
**Requires:** `RequireOrgAdmin`

---

## Analytics

### GET /orgs/{org_id}/analytics
**Handler:** `routes/orgs/analytics.rs`  
**Response:** Aggregated spend data (daily, hourly trends).
```json
{
  "period": "2026-05-27T00:00:00Z",
  "total_spend_usdc": 123.45,
  "request_count": 456,
  "top_models": ["gpt-4o", "claude-opus"],
  "hourly_breakdown": [...]
}
```

---

## Audit Logs

### GET /orgs/{org_id}/audit-logs
**Handler:** `routes/orgs/audit.rs`  
**Requires:** `RequireOrgAdmin`  
**Response:** Paginated list of org actions (team created, member added, budget updated, etc.).

---

## Middleware Chain (lib.rs, build_router())

Layers applied **bottom-up** (innermost runs last):

```
1. TraceLayer
   └─ Logs every request/response with structured fields

2. CorsLayer
   └─ Adds CORS headers (Access-Control-Allow-Origin, etc.)

3. RequestBodyLimitLayer (max 1MB)
   └─ Protects against unbounded uploads

4. TimeoutLayer (30s request timeout)
   └─ Kills slow requests

5. ConcurrencyLimitLayer (configurable max)
   └─ Bounds in-flight request count

6. RateLimitLayer
   └─ Per-wallet/org sliding window (defaults from config)

7. RequestIdLayer
   └─ Generates/tracks X-Request-ID header

8. PromptGuardLayer
   └─ Checks for prompt injection, jailbreaks, PII

9. X402Layer (payment extraction)
   └─ Decodes PAYMENT-SIGNATURE header
   └─ Returns 402 if missing (for protected routes)

10. MetricsLayer
    └─ Emits Prometheus metrics

11. Router (actual handler)
    └─ Routes request to handler function
```

---

## Extractors (Axum Patterns)

### RequireOrg
Extracts `OrgContext` from request extensions. Populated by `api_key` middleware.
- Requires: Valid API key (in Authorization header) or wallet signature
- Provides: `org_id`, `wallet_address`, `role`

### RequireOrgAdmin
Like `RequireOrg`, but requires role `admin` or `owner`.

### OrgContext
```rust
pub struct OrgContext {
    pub org_id: Uuid,
    pub wallet_address: String,
    pub role: String,  // "owner", "admin", "member"
}
```

---

## Error Responses

All errors return JSON with status code:

```json
{
  "error": "invalid_model",
  "message": "Model 'gpt-99' not found",
  "request_id": "req_...",
  "status": 400
}
```

### Common Status Codes
- **200** — Success
- **400** — Bad request (invalid model, missing field)
- **402** — Payment required (missing PAYMENT-SIGNATURE)
- **401** — Unauthorized (invalid API key)
- **403** — Forbidden (insufficient role)
- **404** — Not found (model, org, team)
- **429** — Rate limited
- **500** — Internal server error

---

## Testing

Integration tests live in `crates/gateway/tests/`.

```rust
#[tokio::test]
async fn test_chat_endpoint() {
    let app = test_app();
    let request = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .body(Body::from("..."))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
}
```

Use `tower::ServiceExt::oneshot` — no live server required.

---

## Related

- [Architecture Overview](architecture.md)
- [Frontend Routes](frontend.md)
- [Database Schema](data.md)
