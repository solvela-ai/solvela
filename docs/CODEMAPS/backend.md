<!-- Generated: 2026-05-27 | Files scanned: 45 | Token estimate: ~850 -->

# Backend Routes & Middleware Codemap

**Last Updated:** 2026-05-27  
**Entry:** `crates/gateway/src/lib.rs` (`build_router()` function)  
**Server:** `crates/gateway/src/main.rs` (Axum on port 8402)

## Route Tree

Routes as registered in `crates/gateway/src/lib.rs::build_router`.

```
GET  /.well-known/agent-card.json   (alias: /.well-known/agent.json)
GET  /health
POST /a2a

GET  /v1/models
GET  /v1/supported
POST /v1/chat/completions         [x402 payment required]
POST /v1/images/generations       [x402 payment required]

GET  /pricing
GET  /v1/services
POST /v1/services/register
POST /v1/services/{service_id}/proxy

GET  /v1/escrow/config
GET  /v1/escrow/health
POST /v1/escrow/settle

GET  /v1/nonce
GET  /v1/wallet/{address}/stats

GET  /metrics
GET  /v1/admin/stats

POST /v1/orgs                                           [requires OrgContext or admin token]
GET  /v1/orgs                                           [list]
GET  /v1/orgs/{id}

POST /v1/orgs/{id}/teams
GET  /v1/orgs/{id}/teams
POST /v1/orgs/{id}/teams/{tid}/wallets
GET  /v1/orgs/{id}/teams/{tid}/wallets

POST /v1/orgs/{id}/members
GET  /v1/orgs/{id}/members

POST /v1/orgs/{id}/api-keys
GET  /v1/orgs/{id}/api-keys
DELETE /v1/orgs/{id}/api-keys/{kid}

PUT  /v1/orgs/{id}/teams/{tid}/budget
GET  /v1/orgs/{id}/teams/{tid}/budget
PUT  /v1/wallets/{wallet}/budget
GET  /v1/wallets/{wallet}/budget

GET  /v1/orgs/{id}/stats
GET  /v1/orgs/{id}/teams/{tid}/stats
GET  /v1/orgs/{id}/audit-logs
```

## Public Routes

### GET /.well-known/agent-card.json (alias: /.well-known/agent.json)
**Handler:** `a2a/agent_card.rs`  
**Response:** AgentCard with AP2 + x402 extensions. A2A v0.3 canonical path; the `agent.json` path is served as a backward-compat alias.
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
**Handler:** `routes/chat/mod.rs` (~890 lines)  
**Middleware (see "Middleware Chain" section below for the full router-level stack):**
- `rate_limit::rate_limit` — per-wallet token bucket
- `api_key::extract_api_key` — sets `OrgContext` when a `solvela_k_` bearer is present (additive)
- `x402::extract_payment` — decodes the `payment-signature` request header into `PaymentInfo` extension (additive; never returns 402)
- `TraceLayer`, `metrics::record_metrics`, `CorsLayer`, security headers

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
**Response:** List all available models from ModelRegistry (OpenAI-compatible `data: [...]` shape).

### GET /v1/supported
**Handler:** `routes/supported.rs`  
**Response:** List of supported model IDs/providers (lighter shape, no pricing).

### GET /pricing
**Handler:** `routes/pricing.rs`  
**Response:** Per-token pricing table (input/output costs by model). Note: this endpoint is mounted at `/pricing`, not `/v1/pricing`.

---

## Service Marketplace

### GET /v1/services
**Handler:** `routes/services.rs`  
**Response:** Available external services (from `config/services.toml`).

---

## Escrow Integration

### GET /v1/escrow/config
**Handler:** `routes/escrow.rs::escrow_config`  
**Response:** Escrow program config (program ID, USDC mint, fee payer, recipient).

### GET /v1/escrow/health
**Handler:** `routes/escrow.rs::escrow_health`  
**Response:** Health of the escrow claim worker (queue depth, recent failures).

### POST /v1/escrow/settle
**Handler:** `routes/escrow_settle.rs::handle_settle`  
**Purpose:** Submit/settle a pending claim (operator/admin).

---

## Nonce Management

### GET /v1/nonce
**Handler:** `routes/nonce.rs`  
**Response:** Current durable-nonce account state. There is no `POST /v1/nonce/reserve` route — nonce reservation happens internally during claim submission.

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

### GET /v1/admin/stats
**Handler:** `routes/admin_stats.rs`  
**Requires:** `Authorization: Bearer <SOLVELA_ADMIN_TOKEN>`  
**Response:** System-wide statistics (uptime, memory, connections).

### GET /v1/wallet/{address}/stats
**Handler:** `routes/stats.rs::wallet_stats`  
**Response:** Per-wallet aggregated usage stats.

---

## Organization Management (API Key or Admin Token Auth)

Org-scoped routes use the additive `extract_api_key` middleware that populates an `OrgContext` from a `solvela_k_…` / `rcr_k_…` bearer token (legacy prefix). Handlers themselves do per-route auth — there are not currently any `RequireOrg` / `RequireOrgAdmin` extractors gating these routes at the router layer; instead the handler reads `Option<Extension<OrgContext>>` and falls back to the `SOLVELA_ADMIN_TOKEN` path. Mutating routes accept admin-token auth as a superuser bypass.

### POST /v1/orgs
**Handler:** `routes/orgs/crud.rs::create_org`  
**Request:**
```json
{
  "name": "My AI Company",
  "slug": "my-ai-company"
}
```
**Response:** Organization object with ID, owner_wallet.

### GET /v1/orgs
**Handler:** `routes/orgs/crud.rs::list_orgs`  
**Response:** List of orgs visible to the caller.

### GET /v1/orgs/{id}
**Handler:** `routes/orgs/crud.rs::get_org`  
**Response:** Org details. Note: no `PATCH` or `DELETE` route is currently registered.

---

## Teams

### POST /v1/orgs/{id}/teams
**Handler:** `routes/orgs/teams.rs::create_team`  
**Request:**
```json
{
  "name": "Data Science"
}
```

### GET /v1/orgs/{id}/teams
**Handler:** `routes/orgs/teams.rs::list_teams`  
**Response:** List all teams in org.

### POST /v1/orgs/{id}/teams/{tid}/wallets
**Handler:** `routes/orgs/teams.rs::assign_wallet`  
**Request:**
```json
{
  "wallet_address": "Hpq..."
}
```
**Purpose:** Associate wallet with team (for team-scoped budgets/usage).

### GET /v1/orgs/{id}/teams/{tid}/wallets
**Handler:** `routes/orgs/teams.rs::list_team_wallets`  
**Response:** List wallets assigned to the team.

---

## Members

### POST /v1/orgs/{id}/members
**Handler:** `routes/orgs/teams.rs::add_member`  
**Request:**
```json
{
  "wallet_address": "Hpq...",
  "role": "member"  // or "admin", "owner"
}
```

### GET /v1/orgs/{id}/members
**Handler:** `routes/orgs/teams.rs::list_members`  
**Response:** List all members (wallet, role, created_at). No DELETE route is currently registered.

---

## API Keys

### POST /v1/orgs/{id}/api-keys
**Handler:** `routes/orgs/api_keys.rs::create_api_key`  
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
  "key": "solvela_k_...",
  "key_prefix": "solvela_k_1a2b3c...",
  "name": "...",
  "created_at": "..."
}
```
Note: Full key only shown once; store securely. The literal prefix is `solvela_k_` (the legacy `rcr_k_` prefix is still accepted on the auth side).

### GET /v1/orgs/{id}/api-keys
**Handler:** `routes/orgs/api_keys.rs::list_api_keys`  
**Response:** List API keys (without full key value).

### DELETE /v1/orgs/{id}/api-keys/{kid}
**Handler:** `routes/orgs/api_keys.rs::revoke_api_key`

---

## Budgets

Budgets are scoped per-team or per-wallet (not per-org-collection as a flat list).

### PUT /v1/orgs/{id}/teams/{tid}/budget
**Handler:** `routes/orgs/budget.rs::set_team_budget`  
**Request:**
```json
{
  "hourly_limit_usdc": 25.0,
  "daily_limit_usdc": 100.0,
  "monthly_limit_usdc": 2000.0
}
```

### GET /v1/orgs/{id}/teams/{tid}/budget
**Handler:** `routes/orgs/budget.rs::get_team_budget`

### PUT /v1/wallets/{wallet}/budget
**Handler:** `routes/orgs/budget.rs::set_wallet_budget`

### GET /v1/wallets/{wallet}/budget
**Handler:** `routes/orgs/budget.rs::get_wallet_budget`

---

## Analytics / Stats

### GET /v1/orgs/{id}/stats
**Handler:** `routes/orgs/analytics.rs::get_org_stats`  
**Response:** Aggregated org spend data (totals, top models, by-period breakdowns).

### GET /v1/orgs/{id}/teams/{tid}/stats
**Handler:** `routes/orgs/analytics.rs::get_team_stats`  
**Response:** Team-scoped spend stats.

---

## Audit Logs

### GET /v1/orgs/{id}/audit-logs
**Handler:** `routes/orgs/audit.rs::list_audit_logs`  
**Response:** Paginated list of org actions (team created, member added, budget updated, etc.).

---

## Middleware Chain (lib.rs, build_router())

Layers applied to the router via `.layer(...)`. In Axum each `.layer()` wraps the layers below it (outer layers run first), so the order in code reads top-down as outer→inner:

```
rate_limit::rate_limit        (token-bucket per wallet/IP; skips /health, /v1/models, /metrics)
Extension(rate_limiter)       (shared limiter state)
api_key::extract_api_key      (additive — sets OrgContext when a solvela_k_ bearer token is present)
x402::extract_payment         (additive — decodes payment-signature header, never returns 402)
RequestBodyLimitLayer(10 MiB)
TraceLayer::new_for_http
metrics::record_metrics
CorsLayer
SetResponseHeaderLayer × N    (x-content-type-options, etc.)
```

Note: `x402::extract_payment` is intentionally additive — it never returns 402 from the middleware. Routes (notably `routes/chat/mod.rs`) return 402 themselves when they require payment and `PaymentInfo` is absent from extensions. This matches CLAUDE.md architectural rule #8 ("Payment middleware extracts, routes enforce").

The default request-body cap is 10 MiB, not 1 MB. There is no separate `PromptGuardLayer` or `RequestIdLayer` mounted at the router level today (helpers exist as modules but are not wired as Tower layers).

---

## Extractors (Axum Patterns)

### RequireOrg
Defined in `middleware/api_key.rs`. Extracts `OrgContext` from request extensions; errors `401` if absent. The current router does **not** wrap individual routes with this extractor — handlers consume `Option<Extension<OrgContext>>` directly and pair it with the admin-token bypass. The extractor is available for future tightening.

### RequireOrgAdmin
Like `RequireOrg`, but additionally requires role `admin` or `owner`.

### OrgContext
```rust
pub struct OrgContext {
    pub org_id: Uuid,
    pub api_key_id: Uuid,
    pub role: OrgRole,  // OrgRole::{Owner, Admin, Member}
}
```

`OrgContext` does **not** carry a wallet address — it is keyed off the API key that authenticated the request.

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
