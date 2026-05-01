//! A2A `message/send` handler — implements the x402 payment flow.
//!
//! Two paths:
//! 1. **New request** (no `taskId`) → compute cost → return `input-required` task
//!    with payment metadata so the client can submit payment.
//! 2. **Payment submitted** (has `taskId`) → verify payment → proxy to LLM →
//!    return `completed` task with the chat response as an artifact.

use std::sync::Arc;

use axum::http::HeaderMap;
use serde_json::{json, Value};
use tracing::{info, warn};

use solvela_protocol::{ChatMessage, ChatRequest, Role};
use solvela_router::profiles::{self, Profile};
use solvela_router::scorer;

use crate::a2a::task_store::{self, new_task_id, TaskRecord};
use crate::a2a::types::*;
use crate::providers::fallback::chat_with_model_fallback;
use crate::routes::chat::cost::{estimate_input_tokens, usdc_atomic_amount_checked};
use crate::AppState;

/// A2A-specific JSON-RPC error codes.
const ERR_INVALID_PARAMS: i32 = -32602;
const ERR_INTERNAL: i32 = -32603;
const ERR_TASK_NOT_FOUND: i32 = -32000;
const ERR_PAYMENT_FAILED: i32 = -32001;
const ERR_PROVIDER_ERROR: i32 = -32002;
const ERR_MODEL_NOT_FOUND: i32 = -32003;

/// Handle `message/send` JSON-RPC method.
pub async fn handle_message_send(
    state: Arc<AppState>,
    headers: &HeaderMap,
    request: &JsonRpcRequest,
) -> Result<Value, JsonRpcErrorData> {
    let params: MessageSendParams =
        serde_json::from_value(request.params.clone()).map_err(|e| JsonRpcErrorData {
            code: ERR_INVALID_PARAMS,
            message: format!("Invalid params: {e}"),
            data: None,
        })?;

    match params.task_id {
        Some(ref task_id) => handle_payment_submitted(&state, headers, task_id, &params).await,
        None => handle_new_request(&state, &params).await,
    }
}

// ── New request path ────────────────────────────────────────────────────

/// Handle a new message/send without a taskId — compute cost and return
/// an `input-required` task with x402 payment metadata.
async fn handle_new_request(
    state: &Arc<AppState>,
    params: &MessageSendParams,
) -> Result<Value, JsonRpcErrorData> {
    // Extract user text from message parts
    let user_text = extract_text_from_parts(&params.message.parts)?;

    // Resolve model from message metadata, defaulting to "auto"
    let model_hint = params
        .message
        .metadata
        .as_ref()
        .and_then(|m| m.get("model"))
        .and_then(|v| v.as_str())
        .unwrap_or("auto");

    let resolved_model = resolve_model(model_hint, &user_text, state)?;

    // Look up model in registry for pricing
    // Verify model exists in registry before computing cost
    let _model_info =
        state
            .model_registry
            .get(&resolved_model)
            .ok_or_else(|| JsonRpcErrorData {
                code: ERR_MODEL_NOT_FOUND,
                message: format!("Model not found: {resolved_model}"),
                data: None,
            })?;

    // Build a temporary ChatRequest for token estimation
    let chat_req = ChatRequest {
        model: resolved_model.clone(),
        messages: vec![ChatMessage {
            role: Role::User,
            content: user_text.clone(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }],
        max_tokens: None,
        temperature: None,
        top_p: None,
        stream: false,
        tools: None,
        tool_choice: None,
    };

    let input_tokens = estimate_input_tokens(&chat_req);
    let max_tokens = 1000u32;

    let cost = state
        .model_registry
        .estimate_cost(&resolved_model, input_tokens, max_tokens)
        .map_err(|e| JsonRpcErrorData {
            code: ERR_PROVIDER_ERROR,
            message: format!("Failed to estimate cost: {e}"),
            data: None,
        })?;

    let atomic_amount = usdc_atomic_amount_checked(&cost.total).map_err(|e| JsonRpcErrorData {
        code: ERR_PROVIDER_ERROR,
        message: format!("Failed to compute USDC amount: {e}"),
        data: None,
    })?;

    // Build PaymentRequired (same structure as chat route)
    let mut accepts = vec![solvela_x402::types::PaymentAccept {
        scheme: "exact".to_string(),
        network: solvela_x402::types::SOLANA_NETWORK.to_string(),
        amount: atomic_amount.clone(),
        asset: solvela_x402::types::USDC_MINT.to_string(),
        pay_to: state.config.solana.recipient_wallet.clone(),
        max_timeout_seconds: solvela_x402::types::MAX_TIMEOUT_SECONDS,
        escrow_program_id: None,
    }];

    if state.escrow_claimer.is_some() {
        accepts.push(solvela_x402::types::PaymentAccept {
            scheme: "escrow".to_string(),
            network: solvela_x402::types::SOLANA_NETWORK.to_string(),
            amount: atomic_amount,
            asset: solvela_x402::types::USDC_MINT.to_string(),
            pay_to: state.config.solana.recipient_wallet.clone(),
            max_timeout_seconds: solvela_x402::types::MAX_TIMEOUT_SECONDS,
            escrow_program_id: state.config.solana.escrow_program_id.clone(),
        });
    }

    let payment_required = solvela_x402::types::PaymentRequired {
        x402_version: solvela_x402::types::X402_VERSION,
        resource: solvela_x402::types::Resource {
            url: "/v1/chat/completions".to_string(),
            method: "POST".to_string(),
        },
        accepts,
        cost_breakdown: cost,
        error: "Payment required".to_string(),
    };

    let payment_required_json =
        serde_json::to_value(&payment_required).map_err(|e| JsonRpcErrorData {
            code: ERR_PROVIDER_ERROR,
            message: format!("Failed to serialize payment info: {e}"),
            data: None,
        })?;

    // Create and save task record
    let task_id = new_task_id();
    let record = TaskRecord {
        id: task_id.clone(),
        state: TaskState::InputRequired,
        original_message: user_text,
        payment_required: payment_required_json.clone(),
        model: Some(resolved_model.clone()),
        created_at: chrono::Utc::now(),
    };

    task_store::save_task(state, &record).await.map_err(|e| {
        tracing::error!(error = %e, "A2A task store unavailable — cannot issue payment task");
        JsonRpcErrorData {
            code: ERR_INTERNAL,
            message: "Payment flow unavailable: task state store is not configured".to_string(),
            data: None,
        }
    })?;

    info!(task_id, model = %resolved_model, "A2A new request → input-required");

    // Build A2A Task response
    let task = Task {
        id: task_id,
        status: TaskStatus {
            state: TaskState::InputRequired,
            message: Some(Message {
                role: MessageRole::Agent,
                parts: vec![Part::Text {
                    text: "Payment required to process this request.".to_string(),
                }],
                metadata: Some({
                    let mut meta = serde_json::Map::new();
                    meta.insert(
                        x402_meta::STATUS_KEY.to_string(),
                        json!(x402_meta::PAYMENT_REQUIRED),
                    );
                    meta.insert(x402_meta::REQUIRED_KEY.to_string(), payment_required_json);
                    meta
                }),
            }),
        },
        artifacts: None,
    };

    serde_json::to_value(&task).map_err(|e| JsonRpcErrorData {
        code: ERR_PROVIDER_ERROR,
        message: format!("Failed to serialize task: {e}"),
        data: None,
    })
}

// ── Payment submitted path ──────────────────────────────────────────────

/// Handle a message/send with a taskId — verify payment, proxy to LLM,
/// and return a `completed` task.
async fn handle_payment_submitted(
    state: &Arc<AppState>,
    _headers: &HeaderMap,
    task_id: &str,
    params: &MessageSendParams,
) -> Result<Value, JsonRpcErrorData> {
    // Load task record
    let record = task_store::load_task(state, task_id)
        .await
        .map_err(|e| JsonRpcErrorData {
            code: ERR_INTERNAL,
            message: format!("Task store error: {e}"),
            data: None,
        })?
        .ok_or_else(|| JsonRpcErrorData {
            code: ERR_TASK_NOT_FOUND,
            message: format!("Task not found or expired: {task_id}"),
            data: None,
        })?;

    // Extract payment metadata from message
    let metadata = params
        .message
        .metadata
        .as_ref()
        .ok_or_else(|| JsonRpcErrorData {
            code: ERR_INVALID_PARAMS,
            message: "Payment submission must include metadata with x402 payment fields"
                .to_string(),
            data: None,
        })?;

    let status = metadata
        .get(x402_meta::STATUS_KEY)
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if status != x402_meta::PAYMENT_SUBMITTED {
        return Err(JsonRpcErrorData {
            code: ERR_INVALID_PARAMS,
            message: format!(
                "Expected metadata '{}' = '{}', got '{}'",
                x402_meta::STATUS_KEY,
                x402_meta::PAYMENT_SUBMITTED,
                status
            ),
            data: None,
        });
    }

    let payload_value = metadata
        .get(x402_meta::PAYLOAD_KEY)
        .ok_or_else(|| JsonRpcErrorData {
            code: ERR_INVALID_PARAMS,
            message: format!("Missing '{}' in metadata", x402_meta::PAYLOAD_KEY),
            data: None,
        })?;

    let payload: solvela_x402::types::PaymentPayload =
        serde_json::from_value(payload_value.clone()).map_err(|e| JsonRpcErrorData {
            code: ERR_INVALID_PARAMS,
            message: format!("Invalid payment payload: {e}"),
            data: None,
        })?;

    // Verify payment (skip in dev bypass mode)
    let tx_signature = if state.dev_bypass_payment {
        warn!(
            task_id,
            "DEV MODE: payment verification bypassed for A2A task"
        );
        Some("dev_bypass".to_string())
    } else {
        // Replay check
        let tx_raw = match &payload.payload {
            solvela_x402::types::PayloadData::Direct(p) => &p.transaction,
            solvela_x402::types::PayloadData::Escrow(p) => &p.deposit_tx,
        };

        let is_durable_nonce = crate::routes::chat::uses_durable_nonce(tx_raw);

        let replay_detected = if let Some(cache) = &state.cache {
            cache
                .check_and_record_tx(tx_raw, is_durable_nonce)
                .await
                .is_err()
        } else {
            // GHSA-fq3f-c8p7-873f: durable-nonce transactions carry a 24-hour replay window.
            // The in-memory LRU cannot cover that window, so deny rather than accept with
            // degraded protection.
            if is_durable_nonce {
                // Log only the signature prefix, not the full base64 tx.
                tracing::warn!(
                    tx_prefix = &tx_raw[..tx_raw.len().min(88)],
                    "durable-nonce A2A payment rejected: Redis unavailable (GHSA-fq3f-c8p7-873f)"
                );
                return Err(JsonRpcErrorData {
                    code: ERR_PAYMENT_FAILED,
                    message: "Payment service is temporarily degraded; please retry shortly."
                        .to_string(),
                    data: None,
                });
            }
            // In-memory replay check via std::sync::Mutex + LRU with TTL.
            //
            // GHSA-wc9q-wc6q-gwmq: recover from poisoned lock instead of panicking.
            let mut set = state
                .replay_set
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let now = std::time::Instant::now();
            let found = match set.get(tx_raw) {
                Some(&inserted_at)
                    if now.duration_since(inserted_at) < crate::AppState::REPLAY_TTL =>
                {
                    true
                }
                Some(_) => {
                    // Entry expired — remove and treat as not found
                    set.pop(tx_raw);
                    false
                }
                None => false,
            };
            if found {
                true
            } else {
                set.put(tx_raw.to_string(), now);
                warn!(
                    tx = %tx_raw,
                    "A2A payment accepted under degraded in-memory replay protection (no Redis)"
                );
                false
            }
        };

        if replay_detected {
            return Err(JsonRpcErrorData {
                code: ERR_PAYMENT_FAILED,
                message: "Replay attack detected: transaction already processed".to_string(),
                data: None,
            });
        }

        // Verify via facilitator
        let settlement = state
            .facilitator
            .verify_and_settle(&payload)
            .await
            .map_err(|e| {
                // GHSA-cgqx-mg48-949v: do not echo the verifier error to clients.
                tracing::warn!(error = %e, "A2A payment verification failed");
                JsonRpcErrorData {
                    code: ERR_PAYMENT_FAILED,
                    message: "Payment verification failed. Check your transaction and retry."
                        .to_string(),
                    data: None,
                }
            })?;

        if !settlement.success {
            // Settlement detail (tx_signature, RPC error) is logged by the facilitator;
            // do not surface it to the client.
            tracing::warn!(
                error = ?settlement.error,
                "A2A payment settlement returned success=false"
            );
            return Err(JsonRpcErrorData {
                code: ERR_PAYMENT_FAILED,
                message: "Payment settlement failed. Transaction was not confirmed.".to_string(),
                data: None,
            });
        }

        settlement.tx_signature
    };

    // Build ChatRequest from stored original message
    let model = record.model.unwrap_or_else(|| "auto".to_string());
    let resolved_model = resolve_model(&model, &record.original_message, state)?;

    let model_info = state
        .model_registry
        .get(&resolved_model)
        .ok_or_else(|| JsonRpcErrorData {
            code: ERR_MODEL_NOT_FOUND,
            message: format!("Model not found: {resolved_model}"),
            data: None,
        })?;

    let chat_req = ChatRequest {
        model: resolved_model.clone(),
        messages: vec![ChatMessage {
            role: Role::User,
            content: record.original_message.clone(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }],
        max_tokens: None,
        temperature: None,
        top_p: None,
        stream: false,
        tools: None,
        tool_choice: None,
    };

    // Call provider
    let result = chat_with_model_fallback(
        &state.providers,
        &state.provider_health,
        &model_info.provider,
        &model_info.model_id,
        chat_req,
    )
    .await
    .map_err(|e| JsonRpcErrorData {
        code: ERR_PROVIDER_ERROR,
        message: format!("Provider call failed: {e}"),
        data: None,
    })?;

    // Extract response text
    let response_text = result
        .data
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default();

    // Update task state
    if let Err(e) = task_store::update_task_state(state, task_id, TaskState::Completed).await {
        tracing::error!(error = %e, task_id, "failed to update task state after payment settlement");
    }

    info!(
        task_id,
        model = %resolved_model,
        was_fallback = result.was_fallback,
        "A2A payment verified → completed"
    );

    // Build receipt metadata
    let mut receipt_meta = serde_json::Map::new();
    receipt_meta.insert(
        x402_meta::STATUS_KEY.to_string(),
        json!(x402_meta::PAYMENT_COMPLETED),
    );
    if let Some(sig) = &tx_signature {
        receipt_meta.insert(
            x402_meta::RECEIPTS_KEY.to_string(),
            json!({ "tx_signature": sig }),
        );
    }

    let task = Task {
        id: task_id.to_string(),
        status: TaskStatus {
            state: TaskState::Completed,
            message: Some(Message {
                role: MessageRole::Agent,
                parts: vec![Part::Text {
                    text: response_text.clone(),
                }],
                metadata: Some(receipt_meta),
            }),
        },
        artifacts: Some(vec![Artifact {
            parts: vec![Part::Text {
                text: response_text,
            }],
        }]),
    };

    serde_json::to_value(&task).map_err(|e| JsonRpcErrorData {
        code: ERR_PROVIDER_ERROR,
        message: format!("Failed to serialize task: {e}"),
        data: None,
    })
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Extract the first text content from message parts.
fn extract_text_from_parts(parts: &[Part]) -> Result<String, JsonRpcErrorData> {
    for part in parts {
        if let Part::Text { text } = part {
            if !text.is_empty() {
                return Ok(text.clone());
            }
        }
    }
    Err(JsonRpcErrorData {
        code: ERR_INVALID_PARAMS,
        message: "Message must contain at least one non-empty text part".to_string(),
        data: None,
    })
}

/// Resolve a model string through profiles, aliases, and direct lookup.
fn resolve_model(
    model_hint: &str,
    user_text: &str,
    state: &AppState,
) -> Result<String, JsonRpcErrorData> {
    // Check for profile-based routing (e.g., "auto", "eco", "premium")
    if let Some(profile) = Profile::from_alias(model_hint) {
        let messages = vec![ChatMessage {
            role: Role::User,
            content: user_text.to_string(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }];
        let result = scorer::classify(&messages, false);
        let model_id = profiles::resolve_model(profile, result.tier);
        return Ok(model_id.to_string());
    }

    // Check for model aliases (e.g., "sonnet", "gpt5")
    if let Some(canonical) = profiles::resolve_alias(model_hint) {
        return Ok(canonical.to_string());
    }

    // Check if it's a direct model ID
    if state.model_registry.get(model_hint).is_some() {
        return Ok(model_hint.to_string());
    }

    Err(JsonRpcErrorData {
        code: ERR_MODEL_NOT_FOUND,
        message: format!("Unknown model: {model_hint}"),
        data: None,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use tokio::sync::RwLock;

    use super::*;
    use crate::config::AppConfig;
    use crate::providers::health::{CircuitBreakerConfig, ProviderHealthTracker};
    use crate::providers::ProviderRegistry;
    use crate::routes::escrow::new_slot_cache;
    use crate::services::ServiceRegistry;
    use crate::usage::UsageTracker;
    use solvela_router::models::ModelRegistry;
    use solvela_x402::facilitator::Facilitator;

    fn test_state() -> Arc<AppState> {
        Arc::new(AppState {
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
            .expect("valid test model TOML"), // safe: known-good test data
            service_registry: RwLock::new(ServiceRegistry::empty()),
            providers: ProviderRegistry::from_env(reqwest::Client::new()),
            facilitator: Facilitator::new(vec![]),
            usage: UsageTracker::noop(),
            cache: None,
            provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
            escrow_claimer: None,
            fee_payer_pool: None,
            nonce_pool: None,
            db_pool: None,
            session_secret: b"test-secret".to_vec(),
            http_client: reqwest::Client::new(),
            replay_set: AppState::new_replay_set(),
            slot_cache: new_slot_cache(),
            escrow_metrics: None,
            admin_token: None,
            prometheus_handle: None,
            dev_bypass_payment: false,
        })
    }

    #[test]
    fn test_extract_text_from_parts_single_text() {
        let parts = vec![Part::Text {
            text: "Hello world".to_string(),
        }];
        let result = extract_text_from_parts(&parts);
        assert_eq!(result.expect("should extract text"), "Hello world"); // safe: known-good test data
    }

    #[test]
    fn test_extract_text_from_parts_multiple_parts() {
        let parts = vec![
            Part::Data {
                content_type: "application/json".to_string(),
                data: json!({}),
            },
            Part::Text {
                text: "Found me".to_string(),
            },
        ];
        let result = extract_text_from_parts(&parts);
        assert_eq!(result.expect("should find text part"), "Found me"); // safe: known-good test data
    }

    #[test]
    fn test_extract_text_from_parts_empty_text_skipped() {
        let parts = vec![
            Part::Text {
                text: "".to_string(),
            },
            Part::Text {
                text: "Non-empty".to_string(),
            },
        ];
        let result = extract_text_from_parts(&parts);
        assert_eq!(result.expect("should skip empty"), "Non-empty"); // safe: known-good test data
    }

    #[test]
    fn test_extract_text_from_parts_no_text_returns_error() {
        let parts = vec![Part::Data {
            content_type: "image/png".to_string(),
            data: json!("base64data"),
        }];
        let result = extract_text_from_parts(&parts);
        assert!(result.is_err(), "should error when no text parts");
        assert_eq!(result.unwrap_err().code, ERR_INVALID_PARAMS); // safe: just asserted is_err
    }

    #[test]
    fn test_extract_text_from_parts_empty_vec_returns_error() {
        let result = extract_text_from_parts(&[]);
        assert!(result.is_err(), "should error on empty parts");
    }

    #[tokio::test]
    async fn test_new_request_fails_without_redis() {
        // With cache: None, save_task now returns Err — task issuance must be blocked
        // to prevent clients paying USDC against a task that can't be loaded later.
        let state = test_state(); // test_state() has cache: None
        let params = MessageSendParams {
            message: Message {
                role: MessageRole::User,
                parts: vec![Part::Text {
                    text: "What is Solana?".to_string(),
                }],
                metadata: Some({
                    let mut m = serde_json::Map::new();
                    m.insert("model".to_string(), json!("test-model"));
                    m
                }),
            },
            task_id: None,
        };

        let result = handle_new_request(&state, &params).await;
        assert!(
            result.is_err(),
            "should return error when Redis is unavailable"
        );
        assert_eq!(
            result.unwrap_err().code, // safe: just asserted is_err
            ERR_INTERNAL,
            "error code should be ERR_INTERNAL when task store is unavailable"
        );
    }

    #[tokio::test]
    async fn test_new_request_missing_text_returns_error() {
        let state = test_state();
        let params = MessageSendParams {
            message: Message {
                role: MessageRole::User,
                parts: vec![Part::Data {
                    content_type: "image/png".to_string(),
                    data: json!("base64"),
                }],
                metadata: None,
            },
            task_id: None,
        };

        let result = handle_new_request(&state, &params).await;
        assert!(result.is_err(), "should error when no text parts");
        assert_eq!(
            result.unwrap_err().code, // safe: just asserted is_err
            ERR_INVALID_PARAMS,
            "error code should be invalid params"
        );
    }

    #[tokio::test]
    async fn test_new_request_unknown_model_returns_error() {
        let state = test_state();
        let params = MessageSendParams {
            message: Message {
                role: MessageRole::User,
                parts: vec![Part::Text {
                    text: "Hello".to_string(),
                }],
                metadata: Some({
                    let mut m = serde_json::Map::new();
                    m.insert("model".to_string(), json!("nonexistent-model-xyz"));
                    m
                }),
            },
            task_id: None,
        };

        let result = handle_new_request(&state, &params).await;
        assert!(result.is_err(), "should error for unknown model");
        assert_eq!(
            result.unwrap_err().code, // safe: just asserted is_err
            ERR_MODEL_NOT_FOUND,
            "error code should be model not found"
        );
    }

    #[tokio::test]
    async fn test_payment_submitted_unknown_task_returns_error() {
        let state = test_state();
        let headers = HeaderMap::new();
        let params = MessageSendParams {
            message: Message {
                role: MessageRole::User,
                parts: vec![Part::Text {
                    text: "pay".to_string(),
                }],
                metadata: Some({
                    let mut m = serde_json::Map::new();
                    m.insert(
                        x402_meta::STATUS_KEY.to_string(),
                        json!(x402_meta::PAYMENT_SUBMITTED),
                    );
                    m
                }),
            },
            task_id: Some("nonexistent_task_id".to_string()),
        };

        let result =
            handle_payment_submitted(&state, &headers, "nonexistent_task_id", &params).await;
        assert!(result.is_err(), "should error for unknown task");
        assert_eq!(
            result.unwrap_err().code, // safe: just asserted is_err
            ERR_TASK_NOT_FOUND,
            "error code should be task not found"
        );
    }

    #[tokio::test]
    async fn test_handle_message_send_routes_new_request() {
        let state = test_state();
        let headers = HeaderMap::new();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "message/send".to_string(),
            id: json!("req-1"),
            params: json!({
                "message": {
                    "role": "user",
                    "parts": [{"kind": "text", "text": "Hello"}],
                    "metadata": {"model": "test-model"}
                }
            }),
        };

        let result = handle_message_send(state, &headers, &request).await;
        // With cache: None, new requests are rejected — Redis is required to store
        // the task so clients cannot pay against a task that cannot be loaded.
        assert!(result.is_err(), "should error when Redis is unavailable");
        assert_eq!(
            result.unwrap_err().code, // safe: just asserted is_err
            ERR_INTERNAL,
            "should return ERR_INTERNAL when task store is unavailable"
        );
    }

    #[tokio::test]
    async fn test_handle_message_send_routes_payment_submitted() {
        let state = test_state();
        let headers = HeaderMap::new();
        // With a non-existent taskId, should error with task-not-found
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "message/send".to_string(),
            id: json!("req-2"),
            params: json!({
                "message": {
                    "role": "user",
                    "parts": [{"kind": "text", "text": "pay"}],
                    "metadata": {
                        "x402.payment.status": "payment-submitted"
                    }
                },
                "taskId": "nonexistent_task"
            }),
        };

        let result = handle_message_send(state, &headers, &request).await;
        assert!(result.is_err(), "should error for unknown task");
        assert_eq!(result.unwrap_err().code, ERR_TASK_NOT_FOUND); // safe: just asserted is_err
    }

    #[test]
    fn test_resolve_model_direct_id() {
        let state = test_state();
        let result = resolve_model("test-model", "hello", &state);
        assert_eq!(
            result.expect("should resolve direct model"), // safe: known-good test data
            "test-model"
        );
    }

    #[test]
    fn test_resolve_model_unknown() {
        let state = test_state();
        let result = resolve_model("nonexistent-model", "hello", &state);
        assert!(result.is_err(), "unknown model should error");
        assert_eq!(result.unwrap_err().code, ERR_MODEL_NOT_FOUND); // safe: just asserted is_err
    }
}
