//! Pluggable authentication seam.
//!
//! The gateway ships **one built-in authentication path**: org-scoped API keys
//! (`solvela_k_…` / `rcr_k_…`), resolved in [`super::api_key::extract_api_key`].
//! This module adds an *additive* extension point — [`AuthProvider`] — so an
//! operator can plug in a second credential type (OIDC/SAML bearer token, a
//! session cookie, mTLS, …) and resolve it to the same [`OrgContext`] the rest
//! of the gateway already understands.
//!
//! This exists so the enterprise build (SSO/SCIM/RBAC) can layer on top of the
//! public gateway **without forking** the request pipeline: it constructs an
//! `AppState`, installs its provider on [`crate::AppState::auth_provider`], and
//! reuses `build_router` unchanged. Self-hosters get the same extensibility for
//! free (LDAP, custom JWT, mTLS).
//!
//! ## Security model (read before implementing a provider)
//!
//! - **The built-in API-key path is authoritative and runs first.** A provider
//!   is consulted *only* when the built-in path cleanly abstains (no
//!   `solvela_k_`/`rcr_k_` token present and verifiable). It is never consulted
//!   when the built-in path resolved an identity or failed closed. A provider
//!   therefore cannot override, downgrade, or impersonate an API-key identity.
//! - **Fail closed, not open.** Return [`AuthError`] *only* when the auth
//!   backend is unavailable or misconfigured. It maps to `503`, mirroring the
//!   built-in path's behavior on a database error. An absent, malformed, or
//!   expired credential is **not** an error — return `Ok(None)` to abstain,
//!   exactly as an invalid API key does, and let the route's
//!   [`super::api_key::RequireOrg`] extractor reject with `401`.
//! - **The returned role is trusted verbatim.** The gateway's authorization
//!   layer ([`super::api_key::RequireOrgAdmin`],
//!   [`crate::orgs::models::OrgRole::is_admin_or_owner`]) acts on
//!   [`OrgContext::role`] with no second-guessing — a provider that returns
//!   `Owner`/`Admin` grants those privileges. Providers MUST map IdP
//!   claims/groups to roles defensively and never default to an elevated role.
//! - **Validate fully before trusting.** A provider MUST verify signature,
//!   issuer, audience, and expiry before returning `Ok(Some(_))`.
//! - **Never log token material** — only [`AuthProvider::name`].

use axum::http::HeaderMap;

use crate::middleware::api_key::OrgContext;

/// Read-only view of an incoming request handed to an [`AuthProvider`].
///
/// Deliberately a struct (rather than a bare `&HeaderMap`) and
/// `#[non_exhaustive]` so future inputs — peer address, TLS client certificate,
/// request path — can be added as fields without breaking existing provider
/// implementations. This is a permanent public API: prefer adding a field here
/// over changing the [`AuthProvider::authenticate`] signature.
///
/// Borrows from the in-flight request. Implementations MUST NOT stash an
/// `AuthRequest` (or its borrowed `headers`) across an internal `.await` and
/// reuse it afterward — clone what is needed first.
#[non_exhaustive]
pub struct AuthRequest<'a> {
    /// Request headers — `Authorization`, `Cookie`, custom IdP headers, etc.
    pub headers: &'a HeaderMap,
}

impl std::fmt::Debug for AuthRequest<'_> {
    /// Redacts header contents — headers routinely carry bearer tokens and
    /// cookies, which must never reach logs or test output.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthRequest")
            .field("headers", &"<redacted>")
            .finish()
    }
}

/// Failure from an [`AuthProvider`] that must fail the request **closed**.
///
/// Return a variant only for backend/config problems — never for a bad
/// credential (use `Ok(None)` to abstain). Every variant maps to `503 Service
/// Unavailable`, matching the built-in API-key path's response when its
/// database lookup errors. `#[non_exhaustive]` so variants can be added without
/// a breaking change (the gateway only matches `Err(_)` as a whole).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AuthError {
    /// The authentication backend could not be reached, or errored, so the
    /// provider cannot make a trustworthy allow/deny decision.
    #[error("authentication backend unavailable: {0}")]
    Unavailable(String),
    /// The provider is misconfigured (e.g. missing/invalid JWKS URL or signing
    /// key). Distinct from [`Self::Unavailable`] for operator diagnostics;
    /// still fails closed.
    #[error("authentication provider misconfigured: {0}")]
    Misconfigured(String),
}

/// A pluggable authentication backend that resolves a request to an
/// [`OrgContext`].
///
/// At most one provider is installed (on [`crate::AppState::auth_provider`]);
/// it runs after — and only if — the built-in API-key path abstains. See the
/// [module docs](self) for the full security contract.
///
/// ## Contract
/// - `Ok(Some(ctx))` — authenticated. `ctx` is inserted into the request
///   extensions; downstream extractors and routes treat it identically to an
///   API-key-derived context. Set `api_key_id = None` (the principal has no
///   API key) and supply the appropriate org and [`OrgContext::role`].
/// - `Ok(None)` — abstain. The request continues unauthenticated (additive);
///   any route requiring auth rejects it with `401`.
/// - `Err(_)` — backend unavailable/misconfigured; the request fails closed
///   with `503`.
#[async_trait::async_trait]
pub trait AuthProvider: Send + Sync {
    /// Attempt to authenticate `req`. See the trait/module docs for the
    /// required `Ok(Some)` / `Ok(None)` / `Err` semantics.
    async fn authenticate(&self, req: AuthRequest<'_>) -> Result<Option<OrgContext>, AuthError>;

    /// Stable identifier for logs and metrics (e.g. `"oidc"`, `"saml"`). MUST
    /// NOT contain secrets — it is emitted in tracing fields and may be used as
    /// a metrics label, so keep it low-cardinality and collision-free.
    fn name(&self) -> &'static str;
}
