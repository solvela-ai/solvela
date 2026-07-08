use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use solvela_protocol::{
    ChatChoice, ChatMessage, ChatRequest, ChatResponse, ContentPart, MessageContent,
    ModelRegistration, ParseImageError, ParsedImage, Role, Usage,
};

use super::cache_usage::CacheUsage;
use super::LLMProvider;

/// Provider label for cache-token metering counters.
const PROVIDER_LABEL: &str = "google";

/// Google (Gemini) provider adapter.
///
/// Translates between OpenAI format and Google's Gemini API format.
/// Key differences:
/// - Uses `generateContent` endpoint with `contents` array
/// - System instruction is a separate `system_instruction` field
/// - Parts-based content model instead of string content
/// - Usage is returned as `usageMetadata`
pub struct GoogleProvider {
    api_key: String,
    client: reqwest::Client,
}

impl GoogleProvider {
    pub fn new(client: reqwest::Client, api_key: String) -> Self {
        Self { api_key, client }
    }
}

// ---------------------------------------------------------------------------
// Gemini API request/response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<GeminiPart>,
}

/// A single Gemini content part.
///
/// A part carries exactly ONE payload kind. We model that as an UNTAGGED enum
/// so an empty part or a multi-field part is unrepresentable by construction:
/// a text part serializes to `{"text":...}`, an inline image to
/// `{"inlineData":{...}}`, and a remote image to `{"fileData":{...}}`.
///
/// On the RESPONSE side `serde(untagged)` tries each variant in declaration
/// order; an unrecognized part kind (`thought`, `executableCode`, …) falls
/// through to [`GeminiPart::Other`] (forward-compat) and is dropped by the
/// response reader's `match` rather than failing the whole deserialization.
///
/// Wire schema verified against the Gemini API docs (ai.google.dev, fetched
/// 2026-06-03):
///   text:        {"text":"..."}
///   inline image:{"inlineData":{"mimeType":"image/png","data":"<b64>"}}
///   remote image:{"fileData":{"mimeType":"image/png","fileUri":"https://..."}}
/// Field names are camelCase on the wire (`inlineData`, `mimeType`, `fileUri`).
///
/// VARIANT ORDER IS LOAD-BEARING. `untagged` takes the first variant that
/// deserializes; `Other` (a permissive catch-all) MUST stay last so the typed
/// variants are tried first.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum GeminiPart {
    Text {
        text: String,
    },
    // NOTE: for an UNTAGGED enum, an enum-level `rename_all` does NOT rename the
    // fields inside struct variants — each variant needs its own `rename_all`,
    // or the wire field is the raw snake_case name (`inline_data`) which Gemini
    // rejects. The per-variant `rename_all = "camelCase"` is load-bearing.
    #[serde(rename_all = "camelCase")]
    InlineData {
        inline_data: GeminiInlineData,
    },
    #[serde(rename_all = "camelCase")]
    FileData {
        file_data: GeminiFileData,
    },
    /// Forward-compat catch-all for response part kinds we don't model
    /// (`thought`, `executableCode`, …). Never constructed for requests; the
    /// response reader skips it. The captured value is intentionally unread —
    /// the variant exists only so untagged deserialization of an unknown part
    /// kind succeeds instead of failing the whole response.
    #[serde(skip_serializing)]
    Other(#[allow(dead_code)] serde_json::Value),
}

impl GeminiPart {
    fn text(s: String) -> Self {
        GeminiPart::Text { text: s }
    }
}

/// Outbound-only when constructed for a request; also read back on responses,
/// so it keeps `Deserialize`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiInlineData {
    mime_type: String,
    data: String,
}

/// Constructed only for requests, but `Deserialize` is REQUIRED: it is a
/// payload of the untagged [`GeminiPart`] enum, whose derived `Deserialize`
/// (used to read responses) needs every variant payload to be `Deserialize`.
/// Removing it does not compile.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiFileData {
    mime_type: String,
    file_uri: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: GeminiContent,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsageMetadata {
    prompt_token_count: Option<u32>,
    candidates_token_count: Option<u32>,
    total_token_count: Option<u32>,
    /// Prompt tokens served from Gemini context cache. Maps to the wire field
    /// `cachedContentTokenCount` (verified against ai.google.dev generateContent
    /// docs 2026-06-17). `Option` + camelCase rename: absent on responses
    /// without context caching, yielding 0 for metering. OBSERVABILITY ONLY —
    /// not part of billing (`from_gemini_response` bills off `promptTokenCount`,
    /// which Gemini reports as the FULL prompt size including cached tokens, so
    /// this is purely a discount-visibility read).
    #[serde(default)]
    cached_content_token_count: Option<u32>,
}

// ---------------------------------------------------------------------------
// Format translation
// ---------------------------------------------------------------------------

/// Validate that a Gemini model name is safe to interpolate into the
/// generative-language API URL.
///
/// The handler builds
/// `https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent`
/// — without this guard, an attacker-controlled `req.model` containing `/`,
/// `?`, `#`, or `:` would manipulate the request path, query, or action verb.
/// Gemini model IDs in practice are ASCII alphanumerics plus `-`, `.`, `_`
/// (e.g. `gemini-2.5-flash`, `gemini-1.5-pro-002`).
fn validate_gemini_model_name(model: &str) -> Result<(), String> {
    if model.is_empty() {
        return Err("google model name must not be empty".to_string());
    }
    if !model
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_')
    {
        return Err(format!("invalid google model id: {model:?}"));
    }
    Ok(())
}

/// Infer a concrete image MIME type for Gemini's `fileData.mimeType` from a
/// remote URL's path extension.
///
/// `fileData` requires a real MIME type; `image/*` is rejected by the API. The
/// OpenAI `image_url.url` contract carries no type, so we sniff the path
/// extension (ignoring any query string), restricted to the formats Gemini
/// decodes (png/jpeg/webp). Anything unrecognized falls back to `image/jpeg` —
/// the most common web format — rather than a wildcard. The type is advisory;
/// Gemini ultimately sniffs the fetched bytes, but it must be a valid value.
fn mime_type_from_url(url: &str) -> String {
    // Strip query/fragment so `?v=2` / `#frag` don't defeat the extension match.
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .trim_end_matches('/');
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        // Gemini does not support gif; fall back to jpeg (advisory only).
        _ => "image/jpeg",
    }
    .to_string()
}

/// Translate one OpenAI [`MessageContent`] into Gemini parts.
///
/// Text-only content becomes a single text part. Multimodal content preserves
/// part ORDER and translates each image via [`ImageUrl::parse`]: a `data:` URI
/// → `inlineData` (base64), an http(s) URL → `fileData` (`fileUri`). A
/// malformed image URL returns `Err` so the image is never silently dropped.
fn content_to_gemini_parts(content: &MessageContent) -> Result<Vec<GeminiPart>, String> {
    match content {
        MessageContent::Text(s) => Ok(vec![GeminiPart::text(s.clone())]),
        MessageContent::Parts(parts) => {
            let mut out = Vec::with_capacity(parts.len());
            for part in parts {
                match part {
                    ContentPart::Text { text } => out.push(GeminiPart::text(text.clone())),
                    ContentPart::ImageUrl { image_url } => {
                        let p = match image_url
                            .parse()
                            .map_err(|e: ParseImageError| e.to_string())?
                        {
                            ParsedImage::Base64 { media_type, data } => GeminiPart::InlineData {
                                inline_data: GeminiInlineData {
                                    mime_type: media_type,
                                    data,
                                },
                            },
                            ParsedImage::Url { url } => GeminiPart::FileData {
                                file_data: GeminiFileData {
                                    // Gemini's `fileData` requires a concrete
                                    // MIME type — `image/*` is NOT a valid value
                                    // and the API rejects it. OpenAI image_url
                                    // URLs don't carry a type, so infer it from
                                    // the URL path extension, defaulting to JPEG.
                                    mime_type: mime_type_from_url(&url),
                                    file_uri: url,
                                },
                            },
                        };
                        out.push(p);
                    }
                }
            }
            Ok(out)
        }
    }
}

fn to_gemini_request(req: &ChatRequest) -> Result<GeminiRequest, String> {
    // Extract system instruction. An image in a system/developer message would
    // be silently dropped by `as_text()` while the vision gate still accepts the
    // request — the agent pays, the model never sees it. Reject it explicitly.
    let system_instruction: Option<GeminiContent> = {
        let mut system_text: Vec<String> = Vec::new();
        for m in req
            .messages
            .iter()
            .filter(|m| m.role == Role::System || m.role == Role::Developer)
        {
            if m.content.has_image_parts() {
                return Err(
                    "image content is not supported in system/developer messages; \
                     place images in a user message"
                        .to_string(),
                );
            }
            system_text.push(m.content.as_text().into_owned());
        }

        if system_text.is_empty() {
            None
        } else {
            Some(GeminiContent {
                role: None,
                parts: vec![GeminiPart::text(system_text.join("\n\n"))],
            })
        }
    };

    // Convert messages (excluding system) to Gemini contents
    let mut contents: Vec<GeminiContent> = Vec::new();
    for m in req
        .messages
        .iter()
        .filter(|m| m.role != Role::System && m.role != Role::Developer && m.role != Role::Unknown)
    {
        let role = Some(match m.role {
            Role::User => "user".to_string(),
            Role::Assistant => "model".to_string(),
            Role::System | Role::Developer => "user".to_string(), // filtered above, but safe fallback
            Role::Tool => "user".to_string(), // Gemini uses "user" for tool results
            Role::Unknown => "user".to_string(), // filtered above; safe fallback for forward-compat
        });
        let parts = content_to_gemini_parts(&m.content)?;
        contents.push(GeminiContent { role, parts });
    }

    let generation_config =
        if req.max_tokens.is_some() || req.temperature.is_some() || req.top_p.is_some() {
            Some(GeminiGenerationConfig {
                max_output_tokens: req.max_tokens,
                temperature: req.temperature,
                top_p: req.top_p,
            })
        } else {
            None
        };

    Ok(GeminiRequest {
        contents,
        system_instruction,
        generation_config,
    })
}

fn from_gemini_response(resp: GeminiResponse, original_model: &str) -> ChatResponse {
    let (content, finish_reason) = match resp.candidates.as_ref().and_then(|c| c.first()) {
        Some(c) => {
            let text: String = c
                .content
                .parts
                .iter()
                .filter_map(|p| match p {
                    GeminiPart::Text { text } => Some(text.as_str()),
                    // Non-text response parts (inline/file data, or any
                    // forward-compat `Other` kind like `thought`) contribute no
                    // assistant text and are skipped.
                    GeminiPart::InlineData { .. }
                    | GeminiPart::FileData { .. }
                    | GeminiPart::Other(_) => None,
                })
                .collect::<Vec<_>>()
                .join("");
            let reason = c.finish_reason.as_ref().map(|r| match r.as_str() {
                "STOP" => "stop".to_string(),
                "MAX_TOKENS" => "length".to_string(),
                "SAFETY" => "content_filter".to_string(),
                other => other.to_lowercase(),
            });
            (text, reason)
        }
        None => {
            warn!(
                model = %original_model,
                "Gemini response contained no candidates; likely content filter"
            );
            (String::new(), Some("content_filter".to_string()))
        }
    };

    // OBSERVABILITY ONLY: emit the context-cache read counter (plus the request
    // denominator) before `resp.usage_metadata` is consumed below. Gemini
    // reports cache reads via `cachedContentTokenCount`; there is no separate
    // cache-write count, so write stays 0. Absent metadata / field → 0, which
    // still emits the denominator (the unambiguous-flatline signal). This reads
    // a field billing does not use (Gemini's `promptTokenCount` is the full
    // prompt size), so it cannot affect the agent's charge.
    let cache_read = resp
        .usage_metadata
        .as_ref()
        .and_then(|u| u.cached_content_token_count)
        .unwrap_or(0);
    CacheUsage {
        cache_read_tokens: cache_read,
        cache_write_tokens: 0,
    }
    .emit(PROVIDER_LABEL, original_model);

    let usage = match resp.usage_metadata {
        Some(u) => {
            if u.prompt_token_count.is_none()
                || u.candidates_token_count.is_none()
                || u.total_token_count.is_none()
            {
                warn!(
                    model = %original_model,
                    prompt_tokens = ?u.prompt_token_count,
                    completion_tokens = ?u.candidates_token_count,
                    total_tokens = ?u.total_token_count,
                    "Gemini usage_metadata has missing token count fields; defaulting to 0"
                );
            }
            Some(Usage {
                prompt_tokens: u.prompt_token_count.unwrap_or(0),
                completion_tokens: u.candidates_token_count.unwrap_or(0),
                total_tokens: u.total_token_count.unwrap_or(0),
            })
        }
        None => {
            warn!(
                model = %original_model,
                "Gemini response missing usage_metadata; token counts will be 0"
            );
            None
        }
    };

    ChatResponse {
        id: format!("gemini-{}", uuid::Uuid::new_v4()),
        object: "chat.completion".to_string(),
        created: chrono::Utc::now().timestamp(),
        model: original_model.to_string(),
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage {
                role: Role::Assistant,
                content: content.into(),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
            finish_reason,
        }],
        usage,
    }
}

#[async_trait]
impl LLMProvider for GoogleProvider {
    fn name(&self) -> &str {
        "google"
    }

    fn supported_models(&self) -> Vec<ModelRegistration> {
        vec![]
    }

    async fn chat_completion(
        &self,
        req: ChatRequest,
    ) -> Result<ChatResponse, Box<dyn std::error::Error + Send + Sync>> {
        let original_model = req.model.clone();

        // Extract Gemini model name (e.g., "google/gemini-2.5-flash" → "gemini-2.5-flash")
        let model_name = req.model.strip_prefix("google/").unwrap_or(&req.model);

        // Allowlist the model-name characters before URL interpolation —
        // see `validate_gemini_model_name` for rationale.
        validate_gemini_model_name(model_name)
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;

        // API key sent as a header (not a URL query param) to prevent key leakage
        // in server logs, proxy logs, and browser history.
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
            model_name
        );

        let gemini_req = to_gemini_request(&req)
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;

        let req_body = serde_json::to_value(&gemini_req)?;
        let response = super::retry_with_backoff(2, || {
            self.client
                .post(&url)
                .timeout(super::PROVIDER_REQUEST_TIMEOUT)
                .header("content-type", "application/json")
                .header("x-goog-api-key", &self.api_key)
                .json(&req_body)
                .send()
        })
        .await?;

        let response = response.error_for_status()?;
        let body_text = response.text().await?;
        let gemini_resp: GeminiResponse = serde_json::from_str(&body_text).map_err(|e| {
            // The full body can contain user-prompt-derived content (PII,
            // confidential business data, safety-filtered fragments). Log
            // only structural metadata at WARN; full preview at DEBUG so it
            // is suppressed at default production log levels.
            warn!(
                model = %original_model,
                error = %e,
                body_len = body_text.len(),
                "failed to parse Gemini response"
            );
            debug!(
                model = %original_model,
                body_preview = %&body_text[..body_text.len().min(500)],
                "Gemini response body preview (debug only)"
            );
            e
        })?;

        Ok(from_gemini_response(gemini_resp, &original_model))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text of a `GeminiPart::Text`, else `None`.
    fn part_text(p: &GeminiPart) -> Option<&str> {
        match p {
            GeminiPart::Text { text } => Some(text.as_str()),
            _ => None,
        }
    }

    #[test]
    fn test_gemini_user_text_parts_become_separate_parts() {
        use solvela_protocol::vision::{ContentPart, MessageContent};
        let req = ChatRequest {
            model: "google/gemini-2.5-flash".to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Parts(vec![
                    ContentPart::Text {
                        text: "first".to_string(),
                    },
                    ContentPart::Text {
                        text: "second".to_string(),
                    },
                ]),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: Some(100),
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };

        let gemini_req = to_gemini_request(&req).unwrap();
        assert_eq!(gemini_req.contents.len(), 1);
        // Each text part maps to its own text part, preserving order.
        assert_eq!(gemini_req.contents[0].parts.len(), 2);
        assert_eq!(part_text(&gemini_req.contents[0].parts[0]), Some("first"));
        assert_eq!(part_text(&gemini_req.contents[0].parts[1]), Some("second"));
    }

    #[test]
    fn test_gemini_image_data_uri_becomes_inline_data() {
        use solvela_protocol::vision::{ContentPart, ImageUrl, MessageContent};
        let req = ChatRequest {
            model: "google/gemini-3.1-pro".to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Parts(vec![
                    ContentPart::Text {
                        text: "what is this?".to_string(),
                    },
                    ContentPart::ImageUrl {
                        image_url: ImageUrl {
                            url: "data:image/png;base64,iVBORw0KGgo=".to_string(),
                            detail: None,
                        },
                    },
                ]),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: Some(100),
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };
        let gemini_req = to_gemini_request(&req).unwrap();
        // Exact wire shape per Gemini API (verified 2026-06-03): camelCase
        // inlineData/mimeType.
        let v = serde_json::to_value(&gemini_req.contents[0]).unwrap();
        assert_eq!(
            v["parts"],
            serde_json::json!([
                {"text":"what is this?"},
                {"inlineData":{"mimeType":"image/png","data":"iVBORw0KGgo="}}
            ])
        );
    }

    #[test]
    fn test_gemini_image_http_url_becomes_file_data() {
        use solvela_protocol::vision::{ContentPart, ImageUrl, MessageContent};
        let req = ChatRequest {
            model: "google/gemini-3.1-pro".to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Parts(vec![ContentPart::ImageUrl {
                    image_url: ImageUrl {
                        url: "https://example.com/cat.png".to_string(),
                        detail: None,
                    },
                }]),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: Some(100),
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };
        let gemini_req = to_gemini_request(&req).unwrap();
        let v = serde_json::to_value(&gemini_req.contents[0]).unwrap();
        // MIME inferred from the `.png` path extension (never the invalid
        // `image/*` wildcard).
        assert_eq!(
            v["parts"],
            serde_json::json!([
                {"fileData":{"mimeType":"image/png","fileUri":"https://example.com/cat.png"}}
            ])
        );
    }

    #[test]
    fn test_mime_type_from_url_infers_extension() {
        assert_eq!(mime_type_from_url("https://x/a.png"), "image/png");
        assert_eq!(mime_type_from_url("https://x/a.PNG"), "image/png");
        assert_eq!(mime_type_from_url("https://x/a.jpg"), "image/jpeg");
        assert_eq!(mime_type_from_url("https://x/a.jpeg"), "image/jpeg");
        assert_eq!(mime_type_from_url("https://x/a.webp"), "image/webp");
        // Query string is ignored when sniffing the extension.
        assert_eq!(mime_type_from_url("https://x/a.png?v=2"), "image/png");
        // Unknown / no extension → jpeg fallback, never a wildcard.
        assert_eq!(mime_type_from_url("https://x/image"), "image/jpeg");
        assert_eq!(mime_type_from_url("https://x/a.gif"), "image/jpeg");
        assert!(!mime_type_from_url("https://x/a.png").contains('*'));
    }

    #[test]
    fn test_gemini_image_in_system_message_rejected() {
        use solvela_protocol::vision::{ContentPart, ImageUrl, MessageContent};
        let req = ChatRequest {
            model: "google/gemini-3.1-pro".to_string(),
            messages: vec![
                ChatMessage {
                    role: Role::System,
                    content: MessageContent::Parts(vec![ContentPart::ImageUrl {
                        image_url: ImageUrl {
                            url: "data:image/png;base64,iVBORw0KGgo=".to_string(),
                            detail: None,
                        },
                    }]),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                ChatMessage {
                    role: Role::User,
                    content: "hi".into(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
            ],
            max_tokens: Some(100),
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };
        assert!(
            to_gemini_request(&req).is_err(),
            "an image in a system message must reject, not be silently dropped"
        );
    }

    #[test]
    fn test_gemini_data_uri_without_base64_rejected_at_adapter() {
        use solvela_protocol::vision::{ContentPart, ImageUrl, MessageContent};
        let req = ChatRequest {
            model: "google/gemini-3.1-pro".to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Parts(vec![ContentPart::ImageUrl {
                    image_url: ImageUrl {
                        url: "data:image/png,rawnotbase64".to_string(),
                        detail: None,
                    },
                }]),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: Some(100),
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };
        assert!(
            to_gemini_request(&req).is_err(),
            "a non-base64 data URI must be rejected at the adapter"
        );
    }

    #[test]
    fn test_gemini_response_skips_non_text_parts() {
        // A response with an unknown part kind (`thought`) plus a text part must
        // deserialize (untagged `Other` catch-all) and yield only the text.
        let raw = serde_json::json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [
                        {"thought": true},
                        {"text": "hello"},
                        {"inlineData": {"mimeType": "image/png", "data": "AAAA"}}
                    ]
                },
                "finishReason": "STOP"
            }]
        });
        let resp: GeminiResponse = serde_json::from_value(raw).unwrap();
        let out = from_gemini_response(resp, "google/gemini-3.1-pro");
        assert_eq!(out.choices[0].message.content.as_text(), "hello");
    }

    #[test]
    fn test_gemini_malformed_image_is_rejected_not_dropped() {
        use solvela_protocol::vision::{ContentPart, ImageUrl, MessageContent};
        let req = ChatRequest {
            model: "google/gemini-3.1-pro".to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Parts(vec![ContentPart::ImageUrl {
                    image_url: ImageUrl {
                        url: "ftp://bad/scheme.png".to_string(),
                        detail: None,
                    },
                }]),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: Some(100),
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };
        assert!(
            to_gemini_request(&req).is_err(),
            "a malformed image URL must reject the request, not silently drop the image"
        );
    }

    #[test]
    fn test_gemini_request_translation() {
        let req = ChatRequest {
            model: "google/gemini-2.5-flash".to_string(),
            messages: vec![
                ChatMessage {
                    role: Role::System,
                    content: "Be concise.".into(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                ChatMessage {
                    role: Role::User,
                    content: "Hello!".into(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
            ],
            max_tokens: Some(100),
            temperature: Some(0.7),
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };

        let gemini_req = to_gemini_request(&req).unwrap();

        assert!(gemini_req.system_instruction.is_some());
        assert_eq!(
            part_text(&gemini_req.system_instruction.unwrap().parts[0]),
            Some("Be concise.")
        );
        assert_eq!(gemini_req.contents.len(), 1);
        assert_eq!(gemini_req.contents[0].role.as_deref(), Some("user"));
        assert_eq!(
            gemini_req
                .generation_config
                .as_ref()
                .unwrap()
                .max_output_tokens,
            Some(100)
        );
    }

    #[test]
    fn test_developer_role_extracted_as_system_instruction() {
        let req = ChatRequest {
            model: "google/gemini-2.5-flash".to_string(),
            messages: vec![
                ChatMessage {
                    role: Role::Developer,
                    content: "Always respond in JSON.".into(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                ChatMessage {
                    role: Role::User,
                    content: "Hello!".into(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
            ],
            max_tokens: Some(100),
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };

        let gemini_req = to_gemini_request(&req).unwrap();

        // Developer message should be extracted as system_instruction
        assert!(gemini_req.system_instruction.is_some());
        assert_eq!(
            part_text(&gemini_req.system_instruction.unwrap().parts[0]),
            Some("Always respond in JSON.")
        );
        // Only the User message should remain in contents
        assert_eq!(gemini_req.contents.len(), 1);
        assert_eq!(gemini_req.contents[0].role.as_deref(), Some("user"));
    }

    #[test]
    fn test_gemini_response_translation() {
        let gemini_resp = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: GeminiContent {
                    role: Some("model".to_string()),
                    parts: vec![GeminiPart::text("Hi there!".to_string())],
                },
                finish_reason: Some("STOP".to_string()),
            }]),
            usage_metadata: Some(GeminiUsageMetadata {
                prompt_token_count: Some(5),
                candidates_token_count: Some(3),
                total_token_count: Some(8),
                cached_content_token_count: None,
            }),
        };

        let chat_resp = from_gemini_response(gemini_resp, "google/gemini-2.5-flash");
        assert_eq!(chat_resp.choices[0].message.content.as_text(), "Hi there!");
        assert_eq!(chat_resp.choices[0].finish_reason, Some("stop".to_string()));
        assert_eq!(chat_resp.usage.as_ref().unwrap().total_tokens, 8);
    }

    // ---------------------------------------------------------------------
    // PR-2 cache-token metering (OBSERVABILITY ONLY).
    // ---------------------------------------------------------------------

    /// `cachedContentTokenCount` deserializes into `cached_content_token_count`
    /// via the camelCase rename, and is absent (None) when the wire field is
    /// missing — never a parse error (backward-compat).
    #[test]
    fn gemini_usage_metadata_parses_cached_content_token_count() {
        let raw = r#"{"promptTokenCount":1000,"candidatesTokenCount":20,"totalTokenCount":1020,"cachedContentTokenCount":700}"#;
        let u: GeminiUsageMetadata = serde_json::from_str(raw).unwrap();
        assert_eq!(u.cached_content_token_count, Some(700));

        // Absent → None, not an error.
        let raw_no_cache =
            r#"{"promptTokenCount":1000,"candidatesTokenCount":20,"totalTokenCount":1020}"#;
        let u2: GeminiUsageMetadata = serde_json::from_str(raw_no_cache).unwrap();
        assert_eq!(u2.cached_content_token_count, None);
    }

    /// `from_gemini_response` emits the read counter (by
    /// `cachedContentTokenCount`) and the denominator, and leaves the billed
    /// usage (`prompt_tokens`/`total_tokens`) untouched — Gemini's
    /// `promptTokenCount` is the FULL prompt size, so metering is purely
    /// additive observability.
    #[test]
    fn from_gemini_response_emits_read_counter_without_touching_billing() {
        use crate::cache::test_metrics::{counter_value_filtered, install_test_recorder};

        let handle = install_test_recorder();
        let model = "google/metering-unique-model";
        let key = format!("model=\"{model}\"");
        let read_before =
            counter_value_filtered(&handle, "solvela_provider_cache_read_tokens_total", &key);
        let req_before = counter_value_filtered(&handle, "solvela_provider_requests_total", &key);

        let gemini_resp = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: GeminiContent {
                    role: Some("model".to_string()),
                    parts: vec![GeminiPart::text("ok".to_string())],
                },
                finish_reason: Some("STOP".to_string()),
            }]),
            usage_metadata: Some(GeminiUsageMetadata {
                prompt_token_count: Some(1000),
                candidates_token_count: Some(20),
                total_token_count: Some(1020),
                cached_content_token_count: Some(700),
            }),
        };
        let chat_resp = from_gemini_response(gemini_resp, model);

        // Billing unchanged: full prompt size from Gemini's promptTokenCount.
        assert_eq!(chat_resp.usage.as_ref().unwrap().prompt_tokens, 1000);
        assert_eq!(chat_resp.usage.as_ref().unwrap().total_tokens, 1020);

        let read_after =
            counter_value_filtered(&handle, "solvela_provider_cache_read_tokens_total", &key);
        let req_after = counter_value_filtered(&handle, "solvela_provider_requests_total", &key);
        assert_eq!(read_after - read_before, 700, "read counter delta");
        assert_eq!(req_after - req_before, 1, "denominator delta");
    }

    // ---------------------------------------------------------------------
    // validate_gemini_model_name (URL-injection allowlist)
    // ---------------------------------------------------------------------

    #[test]
    fn validate_gemini_model_name_accepts_real_model_ids() {
        for ok in [
            "gemini-2.5-flash",
            "gemini-1.5-pro-002",
            "gemini-2.0-flash-exp",
            "gemini_pro_v1",
            "AcceptedAlnum123",
        ] {
            assert!(
                validate_gemini_model_name(ok).is_ok(),
                "model id {ok:?} must validate"
            );
        }
    }

    #[test]
    fn validate_gemini_model_name_rejects_url_injection() {
        // Each of these would otherwise alter the request URL.
        for bad in [
            "",
            "gemini/../../../etc/passwd",
            "gemini-2.5-flash:streamGenerateContent",
            "gemini-2.5-flash?api_key=stolen",
            "gemini-2.5-flash#fragment",
            "gemini 2.5 flash",
            "gemini\nflash",
            "../../models/private",
        ] {
            assert!(
                validate_gemini_model_name(bad).is_err(),
                "model id {bad:?} must be rejected"
            );
        }
    }
}
