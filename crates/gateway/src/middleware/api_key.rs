use std::sync::Arc;

use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use tracing::warn;
use uuid::Uuid;

use crate::middleware::auth::{AuthProvider, AuthRequest};
use crate::orgs::models::OrgRole;
use crate::AppState;

/// Resolved organization context for an authenticated request.
///
/// Inserted into request extensions by [`extract_api_key`] — either from the
/// built-in API-key path or from an optional
/// [`AuthProvider`](crate::middleware::auth::AuthProvider) (SSO/OIDC/SAML).
/// Downstream extractors ([`RequireOrg`], [`RequireOrgAdmin`]) and routes treat
/// both origins identically; the audit trail distinguishes them via
/// `api_key_id` (`None` for an external principal).
#[derive(Debug, Clone)]
pub struct OrgContext {
    pub org_id: Uuid,
    /// The API key that authenticated this request, or `None` when the identity
    /// came from an external [`AuthProvider`](crate::middleware::auth::AuthProvider)
    /// rather than a `solvela_k_…` key.
    pub api_key_id: Option<Uuid>,
    pub role: OrgRole,
}

/// `503 Service Unavailable` response used when an authentication backend (the
/// API-key database, or an [`AuthProvider`]) cannot make a trustworthy
/// decision. Fail closed — never serve the request as anonymous on a backend
/// error, as that would silently downgrade an auth failure to a pass-through.
fn auth_unavailable_response() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "error": {
                "type": "service_unavailable",
                "message": "Authentication service temporarily unavailable"
            }
        })),
    )
        .into_response()
}

/// Middleware: resolve an `OrgContext` for the request, if any.
///
/// Additive — it never *requires* auth; routes decide that via the
/// [`RequireOrg`] / [`RequireOrgAdmin`] extractors. Two paths, in strict order:
///
/// 1. **Built-in API keys** (`solvela_k_…` / `rcr_k_…`) — authoritative and own
///    the request whenever such a token is presented. A DB *error* fails closed
///    (`503`); an invalid key (or no DB configured) abstains.
/// 2. **Optional [`AuthProvider`]** ([`AppState::auth_provider`]) — consulted
///    *only* for requests that did NOT present a built-in key and carry no
///    `OrgContext`. It can never see, override, or be confused by a built-in
///    credential. Its backend errors fail closed (`503`); a bad credential
///    abstains.
pub async fn extract_api_key(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    // Whether the caller presented a built-in API key. If so, the built-in path
    // OWNS the request and the fallback provider is never consulted for it — a
    // `solvela_k_`/`rcr_k_` credential is unambiguously ours, valid or not.
    let mut presented_builtin_key = false;

    if let Some(auth) = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        if auth.starts_with("solvela_k_") || auth.starts_with("rcr_k_") {
            presented_builtin_key = true;
            match &state.db_pool {
                Some(pool) => {
                    match crate::orgs::queries::verify_api_key(
                        pool,
                        auth,
                        state.api_key_hmac_secret.as_deref(),
                    )
                    .await
                    {
                        Ok(Some((api_key, org_id))) => {
                            request.extensions_mut().insert(OrgContext {
                                org_id,
                                api_key_id: Some(api_key.id),
                                role: api_key.role,
                            });
                        }
                        Ok(None) => {
                            warn!("invalid or expired API key");
                        }
                        Err(e) => {
                            // DB present but the lookup failed — a transient
                            // backend error. Fail closed (503); never serve the
                            // request anonymously on a verification error.
                            warn!(error = %e, "API key verification DB error — returning 503");
                            return auth_unavailable_response();
                        }
                    }
                }
                None => {
                    // No database at all: the org/API-key subsystem is simply
                    // absent (a supported degraded mode — CLAUDE.md rule 12).
                    // The key can't be verified, so the request continues
                    // unauthenticated and the route's `RequireOrg` extractor
                    // decides (401 for org-scoped routes; paid/free routes are
                    // unaffected). NOT a 503 — org auth isn't "temporarily"
                    // unavailable here, it's not configured at all.
                    warn!(
                        "API key presented but no database configured — cannot verify; \
                         treating request as unauthenticated"
                    );
                }
            }
        }
    }

    // Fallback provider: only for requests that did NOT present a built-in key
    // and have no resolved identity. This keeps the built-in path authoritative.
    if !presented_builtin_key && request.extensions().get::<OrgContext>().is_none() {
        if let Some(provider) = &state.auth_provider {
            if let Err(resp) = apply_fallback_auth_provider(&mut request, provider.as_ref()).await {
                return resp;
            }
        }
    }

    next.run(request).await
}

/// Run `provider` as a fallback, but only when the built-in API-key path left
/// no [`OrgContext`] on the request. Built-in identity always wins: a request
/// that already carries an `OrgContext` is returned untouched and the provider
/// is never consulted.
///
/// Returns `Err(503)` when the provider's backend is unavailable (fail closed);
/// `Ok(())` when it authenticated (context inserted) or abstained.
async fn apply_fallback_auth_provider(
    request: &mut Request,
    provider: &dyn AuthProvider,
) -> Result<(), Response> {
    if request.extensions().get::<OrgContext>().is_some() {
        return Ok(());
    }

    // `AuthRequest` borrows `request`'s headers, but the borrow ends when this
    // `.await` resolves — `outcome` owns its result and holds no reference into
    // `request`, so the `extensions_mut()` reborrow below is sound. If a future
    // field on `AuthRequest` borrows `request`, re-check this before the mutate.
    let outcome = provider
        .authenticate(AuthRequest {
            headers: request.headers(),
        })
        .await;

    match outcome {
        Ok(Some(ctx)) => {
            request.extensions_mut().insert(ctx);
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(e) => {
            warn!(
                provider = provider.name(),
                error = %e,
                "auth provider failed — returning 503 (fail closed)"
            );
            Err(auth_unavailable_response())
        }
    }
}

/// Extractor that requires a valid API key with org context.
/// Returns 401 if no valid API key is present.
#[derive(Debug)]
pub struct RequireOrg(pub OrgContext);

impl<S> FromRequestParts<S> for RequireOrg
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, Json<serde_json::Value>);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<OrgContext>()
            .cloned()
            .map(RequireOrg)
            .ok_or_else(|| {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({
                        "error": {
                            "type": "unauthorized",
                            "message": "Valid API key required"
                        }
                    })),
                )
            })
    }
}

/// Extractor that requires org admin or owner role.
/// Returns 401 if no API key, 403 if insufficient role.
#[derive(Debug)]
pub struct RequireOrgAdmin(pub OrgContext);

impl<S> FromRequestParts<S> for RequireOrgAdmin
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, Json<serde_json::Value>);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let ctx = parts
            .extensions
            .get::<OrgContext>()
            .cloned()
            .ok_or_else(|| {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({
                        "error": {
                            "type": "unauthorized",
                            "message": "Valid API key required"
                        }
                    })),
                )
            })?;

        if ctx.role.is_admin_or_owner() {
            Ok(RequireOrgAdmin(ctx))
        } else {
            Err((
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({
                    "error": {
                        "type": "forbidden",
                        "message": "Admin or owner role required"
                    }
                })),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    use crate::config::AppConfig;
    use crate::providers::health::{CircuitBreakerConfig, ProviderHealthTracker};
    use crate::providers::ProviderRegistry;
    use crate::routes::escrow::new_slot_cache;
    use crate::services::ServiceRegistry;
    use crate::usage::UsageTracker;
    use solvela_router::models::ModelRegistry;
    use solvela_x402::facilitator::Facilitator;

    /// Helper: build a minimal Router that runs `extract_api_key` middleware
    /// and returns 200 with the OrgContext debug string if present, or "none".
    fn test_router(state: Arc<AppState>) -> axum::Router {
        axum::Router::new()
            .route(
                "/test",
                axum::routing::get(|ext: Option<axum::Extension<OrgContext>>| async move {
                    match ext {
                        Some(axum::Extension(ctx)) => format!("org:{}", ctx.org_id),
                        None => "none".to_string(),
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                extract_api_key,
            ))
            .with_state(state)
    }

    fn base_state() -> AppState {
        AppState {
            config: AppConfig::default(),
            model_registry: ModelRegistry::from_toml(
                r#"
[models.test-model]
provider = "test"
model_id = "test-model"
display_name = "Test"
input_cost_per_million = 1.0
output_cost_per_million = 2.0
context_window = 4096
supports_streaming = false
supports_tools = false
supports_vision = false
                "#,
            )
            .expect("valid test model TOML"),
            service_registry: RwLock::new(ServiceRegistry::empty()),
            providers: ProviderRegistry::from_env(reqwest::Client::new()),
            native_anthropic: None,
            search_provider: None,
            facilitator: Facilitator::new(vec![]),
            usage: UsageTracker::noop(),
            cache: None,
            semantic_cache: None,
            provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
            escrow_claimer: None,
            fee_payer_pool: None,
            nonce_pool: None,
            db_pool: None,
            faucet: None,
            session_secret: b"test-secret".to_vec(),
            http_client: reqwest::Client::new(),
            replay_set: AppState::new_replay_set(),
            slot_cache: new_slot_cache(),
            escrow_metrics: None,
            admin_token: None,
            api_key_hmac_secret: None,
            auth_provider: None,
            prometheus_handle: None,
            dev_bypass_payment: false,
            free_rate_limiter: crate::middleware::rate_limit::RateLimiter::new(
                crate::middleware::rate_limit::RateLimitConfig::free_default(),
            ),
            receipts_rate_limiter: crate::middleware::rate_limit::RateLimiter::new(
                crate::middleware::rate_limit::RateLimitConfig::receipts_default(),
            ),
            faucet_rate_limiter: crate::middleware::rate_limit::RateLimiter::new(
                crate::middleware::rate_limit::RateLimitConfig::faucet_default(),
            ),
            deposit_tx_rate_limiter: crate::middleware::rate_limit::RateLimiter::new(
                crate::middleware::rate_limit::RateLimitConfig::deposit_tx_default(),
            ),
            free_global_cap: crate::middleware::rate_limit::FreeTierGlobalCap::new(
                crate::middleware::rate_limit::FREE_TIER_GLOBAL_RPM_DEFAULT,
            ),
        }
    }

    fn make_state() -> Arc<AppState> {
        Arc::new(base_state())
    }

    fn make_state_with_provider(provider: Arc<dyn AuthProvider>) -> Arc<AppState> {
        let mut state = base_state();
        state.auth_provider = Some(provider);
        Arc::new(state)
    }

    #[tokio::test]
    async fn test_no_auth_header_passes_through() {
        let state = make_state();
        let app = test_router(state);

        let req = http::Request::builder()
            .uri("/test")
            .body(Body::empty())
            .expect("valid request");

        let resp = app.oneshot(req).await.expect("request should succeed");
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024)
            .await
            .expect("read body");
        assert_eq!(&body[..], b"none", "no OrgContext should be inserted");
    }

    #[tokio::test]
    async fn test_non_solvela_key_ignored() {
        let state = make_state();
        let app = test_router(state);

        let req = http::Request::builder()
            .uri("/test")
            .header("authorization", "Bearer sk-some-openai-key")
            .body(Body::empty())
            .expect("valid request");

        let resp = app.oneshot(req).await.expect("request should succeed");
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024)
            .await
            .expect("read body");
        assert_eq!(&body[..], b"none", "non-solvela_k_ tokens must be ignored");
    }

    #[tokio::test]
    async fn test_require_org_missing_context() {
        // Build Parts with no OrgContext in extensions
        let (mut parts, _body) = http::Request::builder()
            .uri("/test")
            .body(())
            .expect("valid request")
            .into_parts();

        let result = RequireOrg::from_request_parts(&mut parts, &()).await;
        assert!(result.is_err(), "should reject when no OrgContext");

        let (status, _json) = result.unwrap_err();
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_require_org_admin_member_role() {
        let (mut parts, _body) = http::Request::builder()
            .uri("/test")
            .body(())
            .expect("valid request")
            .into_parts();

        parts.extensions.insert(OrgContext {
            org_id: Uuid::new_v4(),
            api_key_id: Some(Uuid::new_v4()),
            role: OrgRole::Member,
        });

        let result = RequireOrgAdmin::from_request_parts(&mut parts, &()).await;
        assert!(result.is_err(), "member role should be rejected");

        let (status, _json) = result.unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_require_org_admin_owner_role() {
        let (mut parts, _body) = http::Request::builder()
            .uri("/test")
            .body(())
            .expect("valid request")
            .into_parts();

        parts.extensions.insert(OrgContext {
            org_id: Uuid::new_v4(),
            api_key_id: Some(Uuid::new_v4()),
            role: OrgRole::Owner,
        });

        let result = RequireOrgAdmin::from_request_parts(&mut parts, &()).await;
        assert!(result.is_ok(), "owner role should be accepted");
    }

    #[tokio::test]
    async fn test_require_org_admin_admin_role() {
        let (mut parts, _body) = http::Request::builder()
            .uri("/test")
            .body(())
            .expect("valid request")
            .into_parts();

        parts.extensions.insert(OrgContext {
            org_id: Uuid::new_v4(),
            api_key_id: Some(Uuid::new_v4()),
            role: OrgRole::Admin,
        });

        let result = RequireOrgAdmin::from_request_parts(&mut parts, &()).await;
        assert!(result.is_ok(), "admin role should be accepted");
    }

    // ── Pluggable AuthProvider seam ────────────────────────────────────────

    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::middleware::auth::{AuthError, AuthRequest};

    /// Test provider with a fixed outcome that counts how many times it ran, so
    /// precedence ("built-in API-key path wins") is assertable by call count.
    struct MockProvider {
        outcome: MockOutcome,
        calls: Arc<AtomicUsize>,
    }

    enum MockOutcome {
        Authenticate(OrgContext),
        Abstain,
        Unavailable,
    }

    #[async_trait::async_trait]
    impl AuthProvider for MockProvider {
        async fn authenticate(
            &self,
            _req: AuthRequest<'_>,
        ) -> Result<Option<OrgContext>, AuthError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match &self.outcome {
                MockOutcome::Authenticate(ctx) => Ok(Some(ctx.clone())),
                MockOutcome::Abstain => Ok(None),
                MockOutcome::Unavailable => Err(AuthError::Unavailable("test backend down".into())),
            }
        }

        fn name(&self) -> &'static str {
            "mock"
        }
    }

    /// An external (SSO-style) identity: authenticated, but with no API key.
    fn sso_ctx() -> OrgContext {
        OrgContext {
            org_id: Uuid::new_v4(),
            api_key_id: None,
            role: OrgRole::Member,
        }
    }

    #[tokio::test]
    async fn provider_authenticates_when_builtin_abstains() {
        let ctx = sso_ctx();
        let org_id = ctx.org_id;
        let calls = Arc::new(AtomicUsize::new(0));
        let state = make_state_with_provider(Arc::new(MockProvider {
            outcome: MockOutcome::Authenticate(ctx),
            calls: calls.clone(),
        }));
        let app = test_router(state);

        // No `solvela_k_` key → built-in abstains → provider runs and injects.
        let req = http::Request::builder()
            .uri("/test")
            .header("authorization", "Bearer some-oidc-jwt")
            .body(Body::empty())
            .expect("valid request");
        let resp = app.oneshot(req).await.expect("request should succeed");
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024)
            .await
            .expect("read body");
        assert_eq!(&body[..], format!("org:{org_id}").as_bytes());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "provider should be consulted exactly once"
        );
    }

    #[tokio::test]
    async fn provider_abstain_passes_through_unauthenticated() {
        let calls = Arc::new(AtomicUsize::new(0));
        let state = make_state_with_provider(Arc::new(MockProvider {
            outcome: MockOutcome::Abstain,
            calls: calls.clone(),
        }));
        let app = test_router(state);

        let req = http::Request::builder()
            .uri("/test")
            .body(Body::empty())
            .expect("valid request");
        let resp = app.oneshot(req).await.expect("request should succeed");
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024)
            .await
            .expect("read body");
        assert_eq!(
            &body[..],
            b"none",
            "abstain must not authenticate the request"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn provider_backend_unavailable_fails_closed_503() {
        let calls = Arc::new(AtomicUsize::new(0));
        let state = make_state_with_provider(Arc::new(MockProvider {
            outcome: MockOutcome::Unavailable,
            calls: calls.clone(),
        }));
        let app = test_router(state);

        let req = http::Request::builder()
            .uri("/test")
            .body(Body::empty())
            .expect("valid request");
        let resp = app.oneshot(req).await.expect("request should succeed");
        assert_eq!(
            resp.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "an auth-backend error must fail closed (503), never pass through as anonymous"
        );
    }

    #[tokio::test]
    async fn builtin_identity_wins_provider_not_consulted() {
        // A request that already carries an OrgContext (as the built-in path
        // inserts) must NOT trigger the provider — built-in is authoritative.
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = MockProvider {
            outcome: MockOutcome::Authenticate(sso_ctx()),
            calls: calls.clone(),
        };
        let existing = OrgContext {
            org_id: Uuid::new_v4(),
            api_key_id: Some(Uuid::new_v4()),
            role: OrgRole::Owner,
        };
        let existing_org = existing.org_id;
        let mut request = http::Request::builder()
            .uri("/test")
            .body(Body::empty())
            .expect("valid request");
        request.extensions_mut().insert(existing);

        let result = apply_fallback_auth_provider(&mut request, &provider).await;
        assert!(result.is_ok());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "provider must not run when an identity is already resolved"
        );
        let ctx = request
            .extensions()
            .get::<OrgContext>()
            .expect("existing context preserved");
        assert_eq!(
            ctx.org_id, existing_org,
            "existing identity must be untouched"
        );
        assert!(matches!(ctx.role, OrgRole::Owner));
    }

    #[tokio::test]
    async fn builtin_key_is_never_handed_to_provider() {
        // A built-in API key presented to a DB-less gateway must NOT reach the
        // fallback provider — the built-in path owns built-in credentials. The
        // request degrades to unauthenticated (graceful, per CLAUDE.md rule 12),
        // and the provider is never consulted (no confused-deputy on our token).
        // Both the current (`solvela_k_`) and legacy (`rcr_k_`) prefixes share
        // the gating logic, so both are exercised.
        for token in ["Bearer solvela_k_deadbeef", "Bearer rcr_k_deadbeef"] {
            let calls = Arc::new(AtomicUsize::new(0));
            let state = make_state_with_provider(Arc::new(MockProvider {
                // If this provider ever ran it would authenticate, so a non-zero
                // call count or an `org:` body would prove the credential leaked.
                outcome: MockOutcome::Authenticate(sso_ctx()),
                calls: calls.clone(),
            }));
            assert!(state.db_pool.is_none(), "test state has no DB pool");
            let app = test_router(state);

            let req = http::Request::builder()
                .uri("/test")
                .header("authorization", token)
                .body(Body::empty())
                .expect("valid request");
            let resp = app.oneshot(req).await.expect("request should succeed");
            assert_eq!(resp.status(), StatusCode::OK, "token={token}");

            let body = axum::body::to_bytes(resp.into_body(), 1024)
                .await
                .expect("read body");
            assert_eq!(
                &body[..],
                b"none",
                "built-in token ({token}) + no DB must degrade to unauthenticated, \
                 not be authenticated by the provider"
            );
            assert_eq!(
                calls.load(Ordering::SeqCst),
                0,
                "the fallback provider must never be consulted for a built-in credential ({token})"
            );
        }
    }
}
