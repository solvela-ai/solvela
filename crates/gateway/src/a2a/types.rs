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

/// Outbound JSON-RPC 2.0 error response.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub jsonrpc: String,
    pub id: Value,
    pub error: JsonRpcErrorData,
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcErrorData {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

// ── A2A message types ────────────────────────────────────────────────────

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

/// An A2A message (user or agent).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub parts: Vec<Part>,
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

// ── A2A task types ───────────────────────────────────────────────────────

/// Task state in the A2A lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskState {
    InputRequired,
    Working,
    Completed,
    Failed,
}

/// A2A task status.
#[derive(Debug, Clone, Serialize)]
pub struct TaskStatus {
    pub state: TaskState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
}

/// An artifact produced by a completed task.
#[derive(Debug, Clone, Serialize)]
pub struct Artifact {
    pub parts: Vec<Part>,
}

/// A2A task — the core unit of work.
#[derive(Debug, Clone, Serialize)]
pub struct Task {
    pub id: String,
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
            status: TaskStatus {
                state: TaskState::InputRequired,
                message: None,
            },
            artifacts: None,
        };
        let json = serde_json::to_value(&task).unwrap(); // safe: infallible for known struct
        assert_eq!(json["id"], "task-123");
        assert_eq!(json["status"]["state"], "input-required");
        assert!(json["artifacts"].is_null());
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
    }
}
