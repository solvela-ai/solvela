//! A2A (Agent-to-Agent) protocol types for JSON-RPC 2.0.
//!
//! Implements the subset needed for the x402 payment extension:
//! message/send method with payment-required/submitted/completed flow.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── JSON-RPC 2.0 envelope ────────────────────────────────────────────────

/// Inbound JSON-RPC 2.0 request.
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    pub id: Value,
    #[serde(default)]
    pub params: Value,
}

/// Outbound JSON-RPC 2.0 success response.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    pub result: Value,
}

impl JsonRpcResponse {
    pub fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result,
        }
    }
}

/// Outbound JSON-RPC 2.0 error response.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub jsonrpc: String,
    pub id: Value,
    pub error: JsonRpcErrorData,
}

impl JsonRpcError {
    pub fn new(id: Value, error: JsonRpcErrorData) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            error,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcErrorData {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

// ── A2A message types ────────────────────────────────────────────────────

/// Role of a message sender per the A2A spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Agent,
}

/// A message part — text or data.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Part {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "data")]
    Data {
        #[serde(rename = "contentType")]
        content_type: String,
        data: Value,
    },
}

/// Constant `"message"` discriminator required on every Message by A2A v0.3
/// (a2aproject/A2A tag v0.3.0, `specification/json/a2a.json`). A single-variant
/// enum so the wire value is enforced by construction, never a free string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MessageKind {
    #[default]
    #[serde(rename = "message")]
    Message,
}

/// An A2A message (user or agent).
///
/// The v0.3 identity fields (`messageId`, `kind`) are REQUIRED on the wire for
/// agent-authored messages; they carry `serde(default)` so inbound client
/// messages that omit them still deserialize (the gateway never re-serializes
/// an inbound message, so the empty default never reaches the wire).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub parts: Vec<Part>,
    /// Unique message id (UUID v4), minted at construction for agent messages
    /// (A2A v0.3 REQUIRED). Defaults to `""` only for inbound messages.
    #[serde(rename = "messageId", default)]
    pub message_id: String,
    /// Constant `"message"` discriminator (A2A v0.3 REQUIRED).
    #[serde(default)]
    pub kind: MessageKind,
    /// Task this message belongs to (optional in v0.3; set on agent messages
    /// where the task id is in scope).
    #[serde(rename = "taskId", default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Context this message belongs to (optional in v0.3; set on agent
    /// messages where the task's contextId is in scope).
    #[serde(rename = "contextId", default, skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Map<String, Value>>,
}

/// Parameters for message/send.
#[derive(Debug, Clone, Deserialize)]
pub struct MessageSendParams {
    pub message: Message,
    /// Present when continuing a payment flow.
    #[serde(rename = "taskId")]
    #[serde(default)]
    pub task_id: Option<String>,
}

/// Parameters for `tasks/get` (A2A v0.3 `TaskQueryParams`).
///
/// The spec's optional `historyLength` field is ACCEPTED AND IGNORED via
/// serde's default unknown-field tolerance (deliberately not declared): the
/// gateway keeps no per-task message history, so there is nothing to
/// truncate — pinned by `task_query_params_accepts_and_ignores_history_length`.
#[derive(Debug, Clone, Deserialize)]
pub struct TaskQueryParams {
    pub id: String,
}

/// Parameters for `tasks/cancel` (A2A v0.3 `TaskIdParams`).
#[derive(Debug, Clone, Deserialize)]
pub struct TaskIdParams {
    pub id: String,
}

// ── A2A task types ───────────────────────────────────────────────────────

/// Task state in the A2A lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskState {
    InputRequired,
    Working,
    Completed,
    Failed,
    /// Canceled via `tasks/cancel` (A2A v0.3; wire value `"canceled"`).
    /// Terminal. Only reachable from `InputRequired` — `Working` means a
    /// settlement is IN FLIGHT, and canceling it would race funds
    /// (conformance plan D4-a).
    Canceled,
}

impl TaskState {
    /// Check if transitioning from `self` to `next` is valid.
    ///
    /// `Working` is the persisted settle-in-progress marker (conformance plan
    /// D9-a): every paid path writes it under the settlement lock BEFORE any
    /// funds move. The direct `InputRequired→Completed/Failed` arms were
    /// REMOVED in the same change that added the marker write — the state
    /// machine itself now enforces marker discipline: a path that skips the
    /// marker cannot reach a terminal state. `Working→InputRequired` is the
    /// pre-settle failure REVERT (verify error, settle `success=false`,
    /// replay, offer mismatch — no funds moved).
    /// `InputRequired→Canceled` is `tasks/cancel` (D4-a); `Working→Canceled`
    /// is deliberately ABSENT (settlement in flight — canceling would race
    /// funds). `Completed`/`Failed`/`Canceled` are terminal: no outbound arms.
    pub fn can_transition_to(self, next: TaskState) -> bool {
        matches!(
            (self, next),
            (TaskState::InputRequired, TaskState::Working)
                | (TaskState::InputRequired, TaskState::Canceled)
                | (TaskState::Working, TaskState::InputRequired)
                | (TaskState::Working, TaskState::Completed)
                | (TaskState::Working, TaskState::Failed)
        )
    }
}

/// A2A task status.
#[derive(Debug, Clone, Serialize)]
pub struct TaskStatus {
    pub state: TaskState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
    /// ISO-8601 (RFC 3339) UTC time this status was set (A2A v0.3, optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

/// An artifact produced by a completed task.
#[derive(Debug, Clone, Serialize)]
pub struct Artifact {
    /// Unique artifact id (UUID v4), minted at construction (A2A v0.3 REQUIRED).
    #[serde(rename = "artifactId")]
    pub artifact_id: String,
    pub parts: Vec<Part>,
}

/// Constant `"task"` discriminator required on every Task by A2A v0.3.
/// Single-variant enum: the wire value is enforced by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum TaskKind {
    #[default]
    #[serde(rename = "task")]
    Task,
}

/// A2A task — the core unit of work.
#[derive(Debug, Clone, Serialize)]
pub struct Task {
    pub id: String,
    /// Server-generated context id grouping related interactions (A2A v0.3
    /// REQUIRED). Minted at task creation, persisted on `TaskRecord`, and
    /// carried by every Task response for the task's lifetime.
    #[serde(rename = "contextId")]
    pub context_id: String,
    /// Constant `"task"` discriminator (A2A v0.3 REQUIRED).
    pub kind: TaskKind,
    pub status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Vec<Artifact>>,
}

// ── x402 payment extension metadata keys ─────────────────────────────────

/// x402 payment status values for message metadata.
pub mod x402_meta {
    pub const STATUS_KEY: &str = "x402.payment.status";
    pub const REQUIRED_KEY: &str = "x402.payment.required";
    pub const PAYLOAD_KEY: &str = "x402.payment.payload";
    pub const RECEIPTS_KEY: &str = "x402.payment.receipts";

    pub const PAYMENT_REQUIRED: &str = "payment-required";
    pub const PAYMENT_SUBMITTED: &str = "payment-submitted";
    pub const PAYMENT_COMPLETED: &str = "payment-completed";
    pub const PAYMENT_FAILED: &str = "payment-failed";
}

// ── x402 extension error codes ───────────────────────────────────────────

/// Standard x402 error codes per the a2a-x402 spec.
pub mod x402_errors {
    pub const INSUFFICIENT_FUNDS: &str = "INSUFFICIENT_FUNDS";
    pub const INVALID_SIGNATURE: &str = "INVALID_SIGNATURE";
    pub const EXPIRED_PAYMENT: &str = "EXPIRED_PAYMENT";
    pub const DUPLICATE_NONCE: &str = "DUPLICATE_NONCE";
    pub const NETWORK_MISMATCH: &str = "NETWORK_MISMATCH";
    pub const INVALID_AMOUNT: &str = "INVALID_AMOUNT";
    pub const SETTLEMENT_FAILED: &str = "SETTLEMENT_FAILED";
}

// ── A2A extension header ────────────────────────────────────────────────

/// The A2A extension URI for x402.
pub const X402_EXTENSION_URI: &str = "https://github.com/google-a2a/a2a-x402/v0.1";
/// The A2A extension URI for AP2.
pub const AP2_EXTENSION_URI: &str = "https://github.com/google-agentic-commerce/ap2/tree/v0.1";
/// HTTP header for A2A extension negotiation.
pub const A2A_EXTENSIONS_HEADER: &str = "x-a2a-extensions";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_rpc_request_deserialization() {
        let json = r#"{
            "jsonrpc": "2.0",
            "method": "message/send",
            "id": 1,
            "params": {
                "message": {
                    "role": "user",
                    "parts": [{"kind": "text", "text": "Hello"}]
                }
            }
        }"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap(); // safe: known-good test data
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.method, "message/send");
        assert_eq!(req.id, serde_json::json!(1));
    }

    #[test]
    fn test_json_rpc_request_default_params() {
        let json = r#"{"jsonrpc": "2.0", "method": "ping", "id": "abc"}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap(); // safe: known-good test data
        assert_eq!(req.params, Value::Null);
    }

    #[test]
    fn test_task_serialization_input_required() {
        let task = Task {
            id: "task-123".to_string(),
            context_id: "ctx-123".to_string(),
            kind: TaskKind::Task,
            status: TaskStatus {
                state: TaskState::InputRequired,
                message: None,
                timestamp: None,
            },
            artifacts: None,
        };
        let json = serde_json::to_value(&task).unwrap(); // safe: infallible for known struct
        assert_eq!(json["id"], "task-123");
        assert_eq!(json["status"]["state"], "input-required");
        assert!(json["artifacts"].is_null());
        // A2A v0.3 identity fields: camelCase names, constant kind.
        assert_eq!(json["contextId"], "ctx-123");
        assert!(json.get("context_id").is_none(), "wire name is contextId");
        assert_eq!(json["kind"], "task");
        // Optional timestamp is skipped when None, never serialized as null.
        assert!(json.get("timestamp").is_none());
    }

    /// Inbound leniency: clients written against the pre-v0.3 shape omit
    /// `messageId`/`kind`/`taskId`/`contextId` — those messages must keep
    /// deserializing (additive-only contract).
    #[test]
    fn message_inbound_without_v03_identity_fields_deserializes() {
        let msg: Message =
            serde_json::from_str(r#"{"role":"user","parts":[{"kind":"text","text":"hi"}]}"#)
                .expect("legacy inbound message must deserialize");
        assert_eq!(msg.message_id, "");
        assert_eq!(msg.kind, MessageKind::Message);
        assert!(msg.task_id.is_none());
        assert!(msg.context_id.is_none());
    }

    /// Agent-message wire shape: `messageId`/`kind` serialize camelCase with
    /// the constant `"message"` kind; `taskId`/`contextId` are skipped when
    /// absent (never `null`).
    #[test]
    fn agent_message_serializes_v03_identity_fields() {
        let msg = Message {
            role: MessageRole::Agent,
            parts: vec![Part::Text {
                text: "ok".to_string(),
            }],
            message_id: "msg-1".to_string(),
            kind: MessageKind::Message,
            task_id: Some("task-1".to_string()),
            context_id: None,
            metadata: None,
        };
        let json = serde_json::to_value(&msg).unwrap(); // safe: infallible for known struct
        assert_eq!(json["messageId"], "msg-1");
        assert_eq!(json["kind"], "message");
        assert_eq!(json["taskId"], "task-1");
        assert!(json.get("contextId").is_none(), "None contextId is skipped");
        assert!(json.get("message_id").is_none(), "wire name is messageId");
    }

    #[test]
    fn test_part_text_serde_roundtrip() {
        let part = Part::Text {
            text: "hello world".to_string(),
        };
        let json = serde_json::to_string(&part).unwrap(); // safe: infallible for known struct
        let decoded: Part = serde_json::from_str(&json).unwrap(); // safe: just serialized above
        match decoded {
            Part::Text { text } => assert_eq!(text, "hello world"),
            Part::Data { .. } => panic!("expected Text variant"),
        }
    }

    #[test]
    fn test_part_data_serde_roundtrip() {
        let part = Part::Data {
            content_type: "application/json".to_string(),
            data: serde_json::json!({"key": "value"}),
        };
        let json = serde_json::to_string(&part).unwrap(); // safe: infallible for known struct
        let decoded: Part = serde_json::from_str(&json).unwrap(); // safe: just serialized above
        match decoded {
            Part::Data { content_type, data } => {
                assert_eq!(content_type, "application/json");
                assert_eq!(data["key"], "value");
            }
            Part::Text { .. } => panic!("expected Data variant"),
        }
    }

    #[test]
    fn test_message_send_params_with_task_id() {
        let json = r#"{
            "message": {
                "role": "user",
                "parts": [{"kind": "text", "text": "pay and continue"}]
            },
            "taskId": "task-abc"
        }"#;
        let params: MessageSendParams = serde_json::from_str(json).unwrap(); // safe: known-good test data
        assert_eq!(params.task_id.as_deref(), Some("task-abc"));
    }

    #[test]
    fn test_message_send_params_without_task_id() {
        let json = r#"{
            "message": {
                "role": "user",
                "parts": [{"kind": "text", "text": "start"}]
            }
        }"#;
        let params: MessageSendParams = serde_json::from_str(json).unwrap(); // safe: known-good test data
        assert!(params.task_id.is_none());
    }

    #[test]
    fn message_role_serde_lowercase() {
        assert_eq!(
            serde_json::to_value(MessageRole::User).unwrap(), // safe: infallible for enum
            "user"
        );
        assert_eq!(
            serde_json::to_value(MessageRole::Agent).unwrap(), // safe: infallible for enum
            "agent"
        );
        let user: MessageRole = serde_json::from_str("\"user\"").unwrap(); // safe: known-good literal
        assert_eq!(user, MessageRole::User);
        let agent: MessageRole = serde_json::from_str("\"agent\"").unwrap(); // safe: known-good literal
        assert_eq!(agent, MessageRole::Agent);
    }

    #[test]
    fn message_role_invalid_string_fails() {
        let result: Result<MessageRole, _> = serde_json::from_str("\"moderator\"");
        assert!(
            result.is_err(),
            "unknown role string should fail to deserialize"
        );
    }

    #[test]
    fn task_state_can_transition_to_valid() {
        assert!(TaskState::InputRequired.can_transition_to(TaskState::Working));
        // Slice 2b (D4-a): tasks/cancel — the only arm into Canceled.
        assert!(TaskState::InputRequired.can_transition_to(TaskState::Canceled));
        // LOCKSTEP FLIP (conformance plan Slice 2a): Working→InputRequired is
        // the pre-settle failure REVERT arm — previously pinned invalid below.
        assert!(TaskState::Working.can_transition_to(TaskState::InputRequired));
        assert!(TaskState::Working.can_transition_to(TaskState::Completed));
        assert!(TaskState::Working.can_transition_to(TaskState::Failed));
    }

    #[test]
    fn task_state_can_transition_to_invalid() {
        assert!(!TaskState::Completed.can_transition_to(TaskState::InputRequired));
        assert!(!TaskState::Completed.can_transition_to(TaskState::Working));
        assert!(!TaskState::Completed.can_transition_to(TaskState::Failed));
        assert!(!TaskState::Failed.can_transition_to(TaskState::InputRequired));
        assert!(!TaskState::Failed.can_transition_to(TaskState::Working));
        assert!(!TaskState::Failed.can_transition_to(TaskState::Completed));
        assert!(!TaskState::InputRequired.can_transition_to(TaskState::InputRequired));
        // LOCKSTEP FLIP (conformance plan Slice 2a): the direct
        // InputRequired→Completed/Failed arms are REMOVED — every paid path
        // must pass through the `Working` settle-marker first, so the state
        // machine itself rejects a marker-bypassing terminal write.
        assert!(!TaskState::InputRequired.can_transition_to(TaskState::Completed));
        assert!(!TaskState::InputRequired.can_transition_to(TaskState::Failed));
        // Slice 2b (D4-a): `Working→Canceled` is DELIBERATELY absent —
        // `Working` means settlement in flight; canceling it would race
        // funds. Canceled is terminal: no outbound arms.
        assert!(!TaskState::Working.can_transition_to(TaskState::Canceled));
        assert!(!TaskState::Completed.can_transition_to(TaskState::Canceled));
        assert!(!TaskState::Failed.can_transition_to(TaskState::Canceled));
        assert!(!TaskState::Canceled.can_transition_to(TaskState::InputRequired));
        assert!(!TaskState::Canceled.can_transition_to(TaskState::Working));
        assert!(!TaskState::Canceled.can_transition_to(TaskState::Completed));
        assert!(!TaskState::Canceled.can_transition_to(TaskState::Failed));
        assert!(!TaskState::Canceled.can_transition_to(TaskState::Canceled));
    }

    /// Params for the Slice-2b methods: `tasks/get` accepts (and ignores) the
    /// spec's optional `historyLength`; `tasks/cancel` takes the bare id.
    #[test]
    fn task_query_params_accepts_and_ignores_history_length() {
        let with_history: TaskQueryParams =
            serde_json::from_str(r#"{"id": "a2a_abc", "historyLength": 5}"#)
                .expect("historyLength must be accepted (and ignored)");
        assert_eq!(with_history.id, "a2a_abc");
        let bare: TaskQueryParams =
            serde_json::from_str(r#"{"id": "a2a_abc"}"#).expect("bare id must deserialize");
        assert_eq!(bare.id, "a2a_abc");
        let cancel: TaskIdParams =
            serde_json::from_str(r#"{"id": "a2a_abc"}"#).expect("TaskIdParams must deserialize");
        assert_eq!(cancel.id, "a2a_abc");
    }

    #[test]
    fn test_task_state_kebab_case_serialization() {
        assert_eq!(
            serde_json::to_value(TaskState::InputRequired).unwrap(), // safe: infallible for enum
            "input-required"
        );
        assert_eq!(
            serde_json::to_value(TaskState::Working).unwrap(), // safe: infallible for enum
            "working"
        );
        assert_eq!(
            serde_json::to_value(TaskState::Completed).unwrap(), // safe: infallible for enum
            "completed"
        );
        assert_eq!(
            serde_json::to_value(TaskState::Failed).unwrap(), // safe: infallible for enum
            "failed"
        );
        // A2A v0.3 spelling: "canceled" (one l), never "cancelled".
        assert_eq!(
            serde_json::to_value(TaskState::Canceled).unwrap(), // safe: infallible for enum
            "canceled"
        );
    }
}
