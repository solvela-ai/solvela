<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# middleware

## Purpose
Tower/Axum middleware layers applied in `lib.rs::build_router`. Each file owns one cross-cutting concern: payment extraction, rate limiting, API-key auth, prompt-guard, Prometheus metrics, and request-id tagging.

## Key Files
| File | Description |
|------|-------------|
| `mod.rs` | Module root; re-exports each layer |
| `x402.rs` | Decodes the `PAYMENT-SIGNATURE` header, verifies via the facilitator, inserts `Option<PaymentInfo>` into request extensions. **Never returns 402** — routes do |
| `api_key.rs` | `OrgContext`, `RequireOrg`, `RequireOrgAdmin` extractors — resolves API key → org/team/scopes and attaches to request extensions |
| `rate_limit.rs` | Per-wallet / per-IP / per-API-key rate limiting with Redis backing |
| `prompt_guard.rs` | Injection / jailbreak / PII scanner; short-circuits with 400 on block |
| `metrics.rs` | Prometheus request counters + histograms |
| `request_id.rs` | Generates or forwards `x-request-id`; propagated into tracing spans |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- **Separation of concerns rule**: `x402.rs` extracts payment info only. Returning 402 is the job of `routes/chat/payment.rs` / `routes/chat/mod.rs`. Do not mix these.
- Rate limiting keys by `wallet` when payment is attached, otherwise by IP. Per-API-key limits override when an `OrgContext` is attached.
- `RequireOrg` / `RequireOrgAdmin` are Axum extractors — org-scoped routes take them as handler parameters; never read org info from query params or body.
- Middleware layer order matters; see `build_router` in `../lib.rs`.

### Testing Requirements
```bash
cargo test -p gateway middleware
cargo test -p gateway rate_limit
```

### Common Patterns
- Each layer is a tower `Service` or uses `axum::middleware::from_fn`.
- Errors converted to HTTP responses via `IntoResponse` on `GatewayError`.

## Dependencies

### Internal
- `crate::error::GatewayError`, `crate::config::AppConfig`, `crate::cache`, `crate::orgs`.
- `x402` for payment verification.

### External
- `axum`, `tower`, `redis`, `sqlx`, `tracing`, `metrics`.

<!-- MANUAL: -->
