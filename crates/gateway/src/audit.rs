//! Audit logging for organization management actions.
//!
//! All writes are fire-and-forget (`tokio::spawn`) so they never block the hot path.

use sqlx::PgPool;
use uuid::Uuid;

/// An audit event to be recorded in the `audit_logs` table.
#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub org_id: Option<Uuid>,
    pub actor_wallet: Option<String>,
    pub actor_api_key: Option<Uuid>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub details: Option<serde_json::Value>,
    pub ip_address: Option<String>,
}

/// Log an audit event asynchronously (fire-and-forget).
///
/// Clones the pool handle and spawns a background task. Failures are logged as
/// warnings but never surface to the caller.
pub fn log_audit(pool: &PgPool, entry: AuditEntry) {
    let pool = pool.clone();
    tokio::spawn(async move {
        let result = sqlx::query(
            r#"INSERT INTO audit_logs
               (org_id, actor_wallet, actor_api_key, action, resource_type, resource_id, details, ip_address)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"#,
        )
        .bind(entry.org_id)
        .bind(&entry.actor_wallet)
        .bind(entry.actor_api_key)
        .bind(&entry.action)
        .bind(&entry.resource_type)
        .bind(&entry.resource_id)
        .bind(&entry.details)
        .bind(&entry.ip_address)
        .execute(&pool)
        .await;

        if let Err(e) = result {
            tracing::error!(error = %e, action = %entry.action, resource_type = %entry.resource_type, "failed to write audit log — compliance event lost");
        }
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_entry_can_be_constructed() {
        let entry = AuditEntry {
            org_id: Some(Uuid::new_v4()),
            actor_wallet: None,
            actor_api_key: None,
            action: "org.created".to_string(),
            resource_type: "organization".to_string(),
            resource_id: Some(Uuid::new_v4().to_string()),
            details: None,
            ip_address: None,
        };

        assert_eq!(entry.action, "org.created");
        assert_eq!(entry.resource_type, "organization");
        assert!(entry.actor_wallet.is_none());
    }

    #[test]
    fn audit_entry_clone_is_independent() {
        let original = AuditEntry {
            org_id: None,
            actor_wallet: Some("wallet123".to_string()),
            actor_api_key: None,
            action: "api_key.revoked".to_string(),
            resource_type: "api_key".to_string(),
            resource_id: None,
            details: None,
            ip_address: Some("127.0.0.1".to_string()),
        };

        let cloned = original.clone();
        assert_eq!(cloned.action, original.action);
        assert_eq!(cloned.actor_wallet, original.actor_wallet);
    }
}
