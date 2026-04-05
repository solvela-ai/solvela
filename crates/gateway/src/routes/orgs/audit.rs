//! Audit log handler.

use super::*;

/// `GET /v1/orgs/:id/audit-logs` — List audit log entries for an organization.
///
/// Query parameters:
/// - `action`  — filter by action string (exact match)
/// - `since`   — ISO 8601 timestamp; return only events after this time
/// - `limit`   — max results (default 100, capped at 1000)
pub async fn list_audit_logs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    org_ctx: Option<Extension<OrgContext>>,
    Path(org_id): Path<Uuid>,
    Query(params): Query<AuditLogQuery>,
) -> Response {
    let auth = match require_auth(&state, &headers, org_ctx.as_ref().map(|e| &e.0)) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_org_access(&auth, org_id) {
        return resp;
    }
    let pool = require_db!(state);

    let limit = params.limit.unwrap_or(100).clamp(1, 1000);

    // Build query dynamically based on optional filters.
    // Using a fixed-shape query avoids sqlx macro limitations with optional clauses.
    let entries = sqlx::query_as::<_, AuditLogEntry>(
        r#"SELECT id, org_id, actor_wallet, actor_api_key, action, resource_type,
                  resource_id, details, ip_address, created_at
           FROM audit_logs
           WHERE org_id = $1
             AND ($2::TEXT IS NULL OR action = $2)
             AND ($3::TIMESTAMPTZ IS NULL OR created_at > $3)
           ORDER BY created_at DESC
           LIMIT $4"#,
    )
    .bind(org_id)
    .bind(&params.action)
    .bind(params.since)
    .bind(limit)
    .fetch_all(pool)
    .await;

    match entries {
        Ok(rows) => (StatusCode::OK, Json(rows)).into_response(),
        Err(e) => {
            tracing::warn!(org_id = %org_id, error = %e, "failed to list audit logs");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to list audit logs" })),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::super::test_helpers::test_router;

    #[tokio::test]
    async fn audit_logs_missing_token_returns_401() {
        let app = test_router(Some("secret-token"));
        let org_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/orgs/{org_id}/audit-logs"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn audit_logs_valid_token_no_db_returns_503() {
        let app = test_router(Some("mytoken"));
        let org_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/orgs/{org_id}/audit-logs"))
                    .header("authorization", "Bearer mytoken")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // db_pool is None -> 503
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
