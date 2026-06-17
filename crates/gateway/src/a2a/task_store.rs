//! Redis-backed task state for A2A payment flows.
//!
//! Each A2A payment flow creates a task that tracks the lifecycle:
//! input-required → completed/failed (Working state reserved for future async processing).

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::a2a::types::TaskState;
use crate::AppState;

/// Default TTL for task records (10 minutes — allows retries after payment-required).
const TASK_TTL: Duration = Duration::from_secs(600);

/// Stored task state in Redis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: String,
    pub state: TaskState,
    /// The original user message text (used to replay to chat endpoint after payment).
    pub original_message: String,
    /// Serialized PaymentRequired JSON (so payment-submitted can be validated).
    pub payment_required: serde_json::Value,
    /// Model hint from the original request (if provided).
    pub model: Option<String>,
    /// Output-token cap that was used to compute the quoted cost in step 2.
    /// Step 3 (payment-submitted) MUST re-apply this cap when calling the
    /// provider so actual output cannot exceed what the agent has already
    /// paid for. `serde(default)` keeps deserialization backward-compatible
    /// with task records persisted before this field was added — those
    /// records will be processed with the call-site fallback rather than
    /// silently uncapped.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Create a new task ID.
pub fn new_task_id() -> String {
    format!("a2a_{}", Uuid::new_v4().simple())
}

/// Save a task record to Redis. Returns `Err` if Redis is absent or the write fails.
pub async fn save_task(state: &Arc<AppState>, record: &TaskRecord) -> Result<(), String> {
    let Some(ref cache) = state.cache else {
        return Err("A2A task store requires Redis — no Redis configured".to_string());
    };

    let key = format!("a2a_task:{}", record.id);
    let json = serde_json::to_string(record).map_err(|e| e.to_string())?;

    cache
        .set_raw(&key, &json, TASK_TTL)
        .await
        .map_err(|e| format!("Redis save failed: {e}"))
}

/// Load a task record from Redis.
///
/// Returns:
/// - `Ok(Some(record))` — found
/// - `Ok(None)` — not recoverable with Redis present: the task expired, never
///   existed, or its stored record was corrupt and has been purged (see below)
/// - `Err(msg)` — Redis absent, or a Redis read error
///
/// When no Redis is configured this returns `Err`, NOT `Ok(None)`: the A2A task
/// store REQUIRES Redis (`save_task` already errors without it), so an absent
/// cache is an infrastructure failure, not a missing task. Conflating the two
/// mapped a Redis outage to `ERR_TASK_NOT_FOUND` ("task not found or expired") —
/// a misleading terminal-looking error for an agent that already signed a
/// payment, which it cannot distinguish from genuine expiry and may not retry.
/// The handler maps this `Err` to `ERR_INTERNAL`, the correct retry signal
/// (issue #532). This mirrors `save_task`, which already fails closed.
///
/// A *corrupt* stored record (Redis present, value un-deserializable) is treated
/// as `Ok(None)`, not `Err`: unlike a transient Redis outage, the record is
/// unrecoverable, so we purge it and report it as a genuine miss rather than a
/// retryable error (a retry would only re-find nothing). This is deliberately
/// distinct from the no-Redis case above.
pub async fn load_task(state: &Arc<AppState>, task_id: &str) -> Result<Option<TaskRecord>, String> {
    let Some(ref cache) = state.cache else {
        return Err("A2A task store requires Redis — no Redis configured".to_string());
    };
    let key = format!("a2a_task:{task_id}");

    match cache.get_raw(&key).await {
        Ok(Some(json)) => match serde_json::from_str(&json) {
            Ok(record) => Ok(Some(record)),
            Err(e) => {
                tracing::warn!(task_id, error = %e, "A2A task store: corrupt record, deleting");
                // Don't swallow the purge failure: if the delete fails the corrupt
                // record persists and every subsequent load re-hits this arm, so the
                // failed cleanup must be observable rather than silently dropped.
                if let Err(del_err) = cache.del_raw(&key).await {
                    tracing::warn!(
                        task_id,
                        error = %del_err,
                        "A2A task store: failed to delete corrupt record (it will be re-encountered)"
                    );
                }
                Ok(None)
            }
        },
        Ok(None) => Ok(None),
        Err(e) => {
            tracing::warn!(task_id, error = %e, "A2A task store: Redis read error");
            Err(format!("Redis read error: {e}"))
        }
    }
}

/// Update task state in Redis.
pub async fn update_task_state(
    state: &Arc<AppState>,
    task_id: &str,
    new_state: TaskState,
) -> Result<(), String> {
    let record = load_task(state, task_id)
        .await?
        .ok_or_else(|| format!("task not found: {task_id}"))?;

    if !record.state.can_transition_to(new_state) {
        return Err(format!(
            "invalid state transition: {:?} -> {:?}",
            record.state, new_state
        ));
    }

    let updated = TaskRecord {
        state: new_state,
        ..record
    };
    save_task(state, &updated).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::types::TaskState;

    #[test]
    fn task_state_valid_transitions() {
        assert!(TaskState::InputRequired.can_transition_to(TaskState::Working));
        assert!(TaskState::InputRequired.can_transition_to(TaskState::Completed));
        assert!(TaskState::InputRequired.can_transition_to(TaskState::Failed));
        assert!(TaskState::Working.can_transition_to(TaskState::Completed));
        assert!(TaskState::Working.can_transition_to(TaskState::Failed));
    }

    #[test]
    fn task_state_invalid_transitions() {
        // Terminal states cannot transition to anything
        assert!(!TaskState::Completed.can_transition_to(TaskState::InputRequired));
        assert!(!TaskState::Completed.can_transition_to(TaskState::Working));
        assert!(!TaskState::Completed.can_transition_to(TaskState::Failed));
        assert!(!TaskState::Failed.can_transition_to(TaskState::InputRequired));
        assert!(!TaskState::Failed.can_transition_to(TaskState::Working));
        assert!(!TaskState::Failed.can_transition_to(TaskState::Completed));
        // Cannot go backwards
        assert!(!TaskState::Working.can_transition_to(TaskState::InputRequired));
        // Self-transitions are invalid
        assert!(!TaskState::InputRequired.can_transition_to(TaskState::InputRequired));
        assert!(!TaskState::Completed.can_transition_to(TaskState::Completed));
    }

    #[test]
    fn test_new_task_id_format() {
        let id = new_task_id();
        assert!(id.starts_with("a2a_"), "task ID should start with 'a2a_'");
        // UUID simple format is 32 hex chars
        assert_eq!(id.len(), 4 + 32, "task ID should be 'a2a_' + 32 hex chars");
    }

    #[test]
    fn test_new_task_id_unique() {
        let id1 = new_task_id();
        let id2 = new_task_id();
        assert_ne!(id1, id2, "task IDs should be unique");
    }

    #[test]
    fn test_task_record_serde_roundtrip() {
        let record = TaskRecord {
            id: new_task_id(),
            state: TaskState::InputRequired,
            original_message: "Hello, what is Solana?".to_string(),
            payment_required: serde_json::json!({"x402_version": 2}),
            model: Some("auto".to_string()),
            max_tokens: Some(1000),
            created_at: chrono::Utc::now(),
        };

        let json = serde_json::to_string(&record).expect("serialize"); // safe: known struct
        let deserialized: TaskRecord = serde_json::from_str(&json).expect("deserialize"); // safe: just serialized

        assert_eq!(deserialized.id, record.id);
        assert_eq!(deserialized.state, TaskState::InputRequired);
        assert_eq!(deserialized.original_message, "Hello, what is Solana?");
        assert_eq!(deserialized.model, Some("auto".to_string()));
        assert_eq!(deserialized.max_tokens, Some(1000));
    }

    #[test]
    fn test_task_record_without_model() {
        let record = TaskRecord {
            id: new_task_id(),
            state: TaskState::Completed,
            original_message: "test".to_string(),
            payment_required: serde_json::json!({}),
            model: None,
            max_tokens: None,
            created_at: chrono::Utc::now(),
        };

        let json = serde_json::to_string(&record).expect("serialize"); // safe: known struct
        let deserialized: TaskRecord = serde_json::from_str(&json).expect("deserialize"); // safe: just serialized
        assert_eq!(deserialized.model, None);
        assert_eq!(deserialized.max_tokens, None);
        assert_eq!(deserialized.state, TaskState::Completed);
    }

    /// Regression: TaskRecord JSON written before the H1 fix did not include
    /// `max_tokens`. Deserialization must succeed with `max_tokens = None`
    /// rather than failing on an unknown-field/missing-field error, so
    /// in-flight tasks across a deploy boundary still resolve.
    #[test]
    fn test_task_record_legacy_payload_without_max_tokens_deserializes() {
        let legacy = serde_json::json!({
            "id": "a2a_legacy",
            "state": "input-required",
            "original_message": "hi",
            "payment_required": {"x402_version": 2},
            "model": "auto",
            "created_at": "2026-05-09T00:00:00Z"
        })
        .to_string();

        let deserialized: TaskRecord =
            serde_json::from_str(&legacy).expect("legacy record must deserialize");
        assert_eq!(deserialized.max_tokens, None);
        assert_eq!(deserialized.model, Some("auto".to_string()));
    }
}
