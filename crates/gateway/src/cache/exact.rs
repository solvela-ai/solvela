//! Tier 1 — exact-match response cache.
//!
//! Keys are `SHA-256(model ‖ serialised_messages ‖ temperature)` over Redis.
//! See the [`super`] module docs for the wallet-agnostic key design and its
//! cost/margin trade-off. These methods hang off [`ResponseCache`]; the struct
//! and its shared infra (connection handling, replay protection) live in
//! `mod.rs`.

use sha2::{Digest, Sha256};
use tracing::{info, warn};

use solvela_protocol::{ChatRequest, ChatResponse, ContentPart, MessageContent, ParsedImage};

use super::{ResponseCache, CACHE_KEY_PREFIX};

/// First 32 hex chars (16 bytes / 128 bits) of the SHA-256 of `bytes`. Stable
/// and compact. Deliberately mirrors `semantic.rs::short_hash_hex` (same 16-byte
/// prefix) so the exact and semantic tiers fold an image's `data:` payload into
/// their keys with byte-identical representations — the two caches reason about
/// "same image" the same way. Replicated here (rather than sharing) to keep this
/// change contained to `exact.rs` and avoid restructuring module boundaries.
/// 128 bits keeps birthday-collision probability negligible in the shared,
/// wallet-agnostic cache.
fn short_hash_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    hex::encode(&digest[..16])
}

/// A short, stable representation of an image URL for the cache key.
///
/// Substituted for the raw `ImageUrl.url` BEFORE serialising a message with
/// image parts, so the SHA-256 key never folds in a multi-megabyte `data:`
/// payload (up to `MAX_DATA_URI_BYTES` = 10 MiB) verbatim — matching what
/// `semantic.rs::image_rep_suffix` does for its embedded text.
///
/// - `http(s)` URL → kept verbatim (already short and stable).
/// - `data:` URI ([`ParsedImage::Base64`]) → `sha256:<first-32-hex>` of the raw
///   base64 payload string bytes (verbatim — NOT decoded first), identical to
///   `semantic.rs`'s convention so the two tiers agree on "same image".
/// - unparseable url → `rawsha256:<first-32-hex>` of the raw url string bytes.
///
/// NOTE: the unparseable arm deliberately DIFFERS from `semantic.rs`, which
/// returns `Skip` (bypassing the cache entirely) to avoid building an
/// embedding-divergent key. The exact tier has no embedding and no
/// divergent-key risk — the key is a literal SHA-256 over the substituted
/// content — so substituting a stable hash of the raw url is safe and strictly
/// better than skipping: two DISTINCT malformed urls must never collide onto
/// one key (which would serve one input's paid response for the other). The
/// route rejects malformed images pre-settlement, so this arm is defensive; the
/// `rawsha256:` prefix keeps it in a distinct namespace from a real payload
/// hash so a raw-url hash can never alias a base64-payload hash.
fn image_key_rep(image_url: &solvela_protocol::ImageUrl) -> String {
    match image_url.parse() {
        Ok(ParsedImage::Url { url }) => url,
        Ok(ParsedImage::Base64 { data, .. }) => {
            // Hash the payload (not the media-type) so the same bytes sent under
            // different declared types still distinguish by content.
            format!("sha256:{}", short_hash_hex(data.as_bytes()))
        }
        Err(_) => {
            // Defensive (unreachable past the route's pre-settlement parse gate):
            // hash the raw url so two distinct malformed values still differ.
            format!("rawsha256:{}", short_hash_hex(image_url.url.as_bytes()))
        }
    }
}

/// Rewrite a message that carries image parts so each image's `url` is replaced
/// by its short [`image_key_rep`], bounding the bytes hashed into the cache key
/// while preserving exact-match correctness (distinct images still differ; the
/// same image is stable). Text parts and message ordering are preserved
/// verbatim — only the image-url field is substituted.
fn message_with_bounded_images(
    msg: &solvela_protocol::ChatMessage,
) -> solvela_protocol::ChatMessage {
    let MessageContent::Parts(parts) = &msg.content else {
        // Only `Parts` can carry images; `has_image_parts()` already guaranteed
        // we are in the image branch, so this is defensive.
        return msg.clone();
    };
    let bounded: Vec<ContentPart> = parts
        .iter()
        .map(|part| match part {
            ContentPart::ImageUrl { image_url } => ContentPart::ImageUrl {
                image_url: solvela_protocol::ImageUrl {
                    url: image_key_rep(image_url),
                    detail: image_url.detail.clone(),
                },
            },
            ContentPart::Text { text } => ContentPart::Text { text: text.clone() },
        })
        .collect();
    let mut rewritten = msg.clone();
    rewritten.content = MessageContent::Parts(bounded);
    rewritten
}

impl ResponseCache {
    /// Generate a cache key from a request.
    /// Key = SHA256(model ‖ messages_json ‖ temperature ‖ tools_json ‖
    /// tool_choice_json). Message order is significant (it's part of the
    /// conversation), so messages are NOT sorted — see
    /// `cache_key_is_sensitive_to_message_order`.
    ///
    /// `tools` and `tool_choice` are part of the key because they materially
    /// change the response shape: with tools available the model may emit a
    /// `tool_calls` completion instead of prose, and `tool_choice` can force
    /// or forbid a specific function. Two otherwise-identical requests that
    /// differ only in their tool spec would otherwise collide on the same
    /// SHA-256 key and serve each other's responses — a wrong-answer money
    /// loss for the agent that paid for a tool-capable response and received
    /// prose from cache. The fields are serialised on a stable, sorted-key
    /// JSON form to make `Some(...)` distinct from `None` regardless of map
    /// ordering quirks.
    ///
    /// Returns `None` if any of the JSON-serialised inputs cannot be
    /// serialised. **This must never be coerced to a default key**: silently
    /// omitting any field from the hash would collapse the key, making every
    /// prompt sharing the remaining fields collide — a wallet A request
    /// served wallet B's response. Callers treat `None` as a guaranteed cache
    /// miss and fall through to the upstream provider.
    ///
    /// ## Content normalization (wire-shape independence)
    ///
    /// `ChatMessage::content` is a `#[serde(untagged)]` [`MessageContent`]:
    /// the same text reaches us either as a bare JSON string
    /// (`Text("hello")`) or as an array of parts
    /// (`Parts([{"type":"text","text":"hello"}])`) depending on the client
    /// (curl sends strings, array-shaped agents send parts). Those two shapes
    /// serialise to *different* bytes, so hashing `req.messages` verbatim
    /// would yield different keys for text-identical prompts — an exact-cache
    /// miss that charges the operator for a duplicate upstream call and
    /// violates CLAUDE.md rule #16 (identical prompts share a cached
    /// response). To make the key wire-shape-independent, each message whose
    /// content carries no image parts is normalised to its canonical
    /// `Text(as_text())` form *for key computation only* (the caller's `req`
    /// is never mutated). Messages that contain image parts keep their
    /// `Parts` structure (flattening would discard the image and let two
    /// genuinely different multimodal prompts collide) BUT each image's `url`
    /// is first substituted with a short, stable representation via
    /// [`message_with_bounded_images`]: an `http(s)` URL verbatim, or a
    /// `sha256:<prefix>` of a `data:` URI's base64 payload. This bounds the
    /// bytes hashed into the key — a `data:` URI can be up to
    /// `MAX_DATA_URI_BYTES` (10 MiB) — without weakening exact-match
    /// correctness (distinct images still produce distinct keys; the same image
    /// is stable). It mirrors the fold `semantic.rs::image_rep_suffix` already
    /// applies. (Image content is validated upstream before settlement, so the
    /// unparseable arm is defensive.)
    pub fn cache_key(req: &ChatRequest) -> Option<String> {
        // Normalise message content to a canonical text form so the same
        // prompt sent as a JSON string and as a text-parts array hashes
        // identically. Clone-and-swap is fine here: this is the cache-key
        // path, not the hottest request loop, and we must not mutate `req`.
        let normalized_messages: Vec<_> = req
            .messages
            .iter()
            .map(|msg| {
                if msg.content.has_image_parts() {
                    // Keep the `Parts` structure (flattening would drop the
                    // image and let distinct multimodal prompts collide) but
                    // substitute each image url with a short, stable rep so a
                    // 10 MiB `data:` payload is not hashed verbatim. Distinct
                    // images still differ; the same image stays stable.
                    message_with_bounded_images(msg)
                } else {
                    let mut normalized = msg.clone();
                    normalized.content = MessageContent::Text(msg.content.as_text().into_owned());
                    normalized
                }
            })
            .collect();

        let msgs_json = match serde_json::to_string(&normalized_messages) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, model = %req.model, "cache_key: failed to serialise messages; treating as guaranteed miss");
                return None;
            }
        };
        // Tool spec affects response shape (`tool_calls` vs prose). Serialise
        // `Option` directly so `None` and `Some([])` produce distinct bytes.
        let tools_json = match serde_json::to_string(&req.tools) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, model = %req.model, "cache_key: failed to serialise tools; treating as guaranteed miss");
                return None;
            }
        };
        let tool_choice_json = match serde_json::to_string(&req.tool_choice) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, model = %req.model, "cache_key: failed to serialise tool_choice; treating as guaranteed miss");
                return None;
            }
        };
        let mut hasher = Sha256::new();
        hasher.update(req.model.as_bytes());
        // Domain-separator bytes guard against the (theoretical) collision
        // where one field's serialised tail concatenates with the next field's
        // head into an identical byte stream as a different split. SHA-256
        // doesn't need this for security, but it makes the encoding
        // unambiguous and the test-side reasoning trivial.
        hasher.update(b"|msgs|");
        hasher.update(msgs_json.as_bytes());
        hasher.update(b"|tools|");
        hasher.update(tools_json.as_bytes());
        hasher.update(b"|tool_choice|");
        hasher.update(tool_choice_json.as_bytes());
        if let Some(temp) = req.temperature {
            hasher.update(b"|temp|");
            hasher.update(temp.to_le_bytes());
        }
        let hash = hasher.finalize();
        Some(format!("{}{}", CACHE_KEY_PREFIX, hex::encode(hash)))
    }

    /// Try to get a cached response.
    pub async fn get(&self, req: &ChatRequest) -> Option<ChatResponse> {
        if !self.config.enabled || req.stream {
            return None;
        }
        let key = Self::cache_key(req)?;

        let mut conn = self.client.get_multiplexed_async_connection().await.ok()?;
        let cached: Option<String> = redis::cmd("GET")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .ok()?;

        match cached {
            Some(json_str) => match serde_json::from_str::<ChatResponse>(&json_str) {
                Ok(response) => {
                    // Defense-in-depth: the write-side guard in `set` blocks
                    // new caching of usage-less responses, but entries written
                    // before this guard (or via a future bypass) could still
                    // be in Redis. Serving them would skip both `log_spend`
                    // reconciliation branches in `chat/mod.rs` and strand the
                    // `check_budget` reservation. Treat as a miss → upstream
                    // call, real settlement.
                    if response.usage.is_none() {
                        metrics::counter!("solvela_exact_cache_read_evicted_no_usage_total")
                            .increment(1);
                        warn!(
                            key = %key,
                            "cached response has no usage block; evicting and \
                             treating as miss so the wallet's budget reservation \
                             can settle"
                        );
                        // Fire-and-forget DEL: without this, the stale entry
                        // survives until TTL (default 600s) and every read in
                        // that window re-fires the counter, re-emits the warn,
                        // and re-calls upstream. The counter is named
                        // `..._evicted_..._total` to communicate one-shot
                        // remediation to on-call — the DEL makes that honest.
                        let client = self.client.clone();
                        let evict_key = key.clone();
                        tokio::spawn(async move {
                            if let Ok(mut conn) = client.get_multiplexed_async_connection().await {
                                let _: Result<(), redis::RedisError> = redis::cmd("DEL")
                                    .arg(&evict_key)
                                    .query_async(&mut conn)
                                    .await;
                            }
                        });
                        return None;
                    }
                    info!(key = %key, "cache hit");
                    Some(response)
                }
                Err(e) => {
                    warn!(error = %e, key = %key, "failed to deserialize cached response");
                    None
                }
            },
            None => None,
        }
    }

    /// Store a response in the cache.
    ///
    /// Refuses to cache responses that lack a `usage` block. On a future hit
    /// the downstream `log_spend` reconciliation branches both require either
    /// `usage` or a semantic discount; an exact hit returns `cost_outcome: None`
    /// and threads `cached.usage` through, so a `None` usage would skip
    /// settlement and permanently strand the `check_budget` reservation on the
    /// wallet's counter (drains budget by `estimated_cost` per repeat).
    pub async fn set(&self, req: &ChatRequest, response: &ChatResponse) {
        if !self.config.enabled || req.stream {
            return;
        }
        if response.usage.is_none() {
            metrics::counter!("solvela_exact_cache_write_skipped_no_usage_total").increment(1);
            warn!(
                model = %req.model,
                "refusing to cache response with no usage block; \
                 settlement requires usage to reconcile the budget reservation"
            );
            return;
        }
        let key = match Self::cache_key(req) {
            Some(k) => k,
            None => return, // serialisation failure already logged inside `cache_key`
        };

        let json_str = match serde_json::to_string(response) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "failed to serialize response for cache");
                return;
            }
        };

        // Spawn async — never block the request path
        let client = self.client.clone();
        let ttl = self.config.default_ttl_secs;
        tokio::spawn(async move {
            match client.get_multiplexed_async_connection().await {
                Ok(mut conn) => {
                    let result: Result<(), redis::RedisError> = redis::cmd("SETEX")
                        .arg(&key)
                        .arg(ttl)
                        .arg(&json_str)
                        .query_async(&mut conn)
                        .await;

                    if let Err(e) = result {
                        warn!(error = %e, key = %key, "failed to write to cache");
                    } else {
                        info!(key = %key, ttl_secs = ttl, "cached response");
                    }
                }
                Err(e) => {
                    warn!(error = %e, "failed to connect to Redis for caching");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::CacheConfig;
    use solvela_protocol::{ChatMessage, ContentPart, Role};

    use crate::cache::test_metrics::{counter_value, install_test_recorder};

    /// Helper to build a ChatRequest for testing.
    fn make_request(
        model: &str,
        messages: Vec<ChatMessage>,
        temperature: Option<f32>,
    ) -> ChatRequest {
        ChatRequest {
            model: model.to_string(),
            messages,
            max_tokens: None,
            temperature,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        }
    }

    fn user_message(content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: content.into(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// Build a user message whose content is a `Parts` array carrying a single
    /// text part — the wire shape array-based clients send. `as_text()` on
    /// this equals the equivalent `Text(content)` message.
    fn user_message_parts(content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Parts(vec![ContentPart::Text {
                text: content.to_string(),
            }]),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn test_cache_key_deterministic() {
        let req = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(0.7));
        let key1 = ResponseCache::cache_key(&req).expect("test request must serialise");
        let key2 = ResponseCache::cache_key(&req).expect("test request must serialise");
        assert_eq!(key1, key2);
        assert!(key1.starts_with("solvela:cache:"));
    }

    #[test]
    fn test_cache_key_different_for_different_models() {
        let req_a = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(0.7));
        let req_b = make_request(
            "anthropic/claude-3.5-sonnet",
            vec![user_message("Hello")],
            Some(0.7),
        );
        assert_ne!(
            ResponseCache::cache_key(&req_a).expect("test request must serialise"),
            ResponseCache::cache_key(&req_b).expect("test request must serialise"),
        );
    }

    #[test]
    fn test_cache_key_different_for_different_messages() {
        let req_a = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(0.7));
        let req_b = make_request("openai/gpt-4o", vec![user_message("Goodbye")], Some(0.7));
        assert_ne!(
            ResponseCache::cache_key(&req_a).expect("test request must serialise"),
            ResponseCache::cache_key(&req_b).expect("test request must serialise"),
        );
    }

    /// Wire-shape independence (CLAUDE.md rule #16). The same text prompt sent
    /// as a bare JSON string (`Text("hello")`) and as a text-parts array
    /// (`Parts([{type:text, text:"hello"}])`) MUST hash to the same cache key;
    /// otherwise an array-shaped client (e.g. an agent that sends content
    /// arrays) misses the cache populated by a string-shaped client (e.g.
    /// curl) for a text-identical prompt, charging the operator for a
    /// duplicate upstream call. A genuinely different prompt must still hash
    /// differently — proving the normalization collapses *shape* but not
    /// *content*.
    ///
    /// Falsifiability: removing the content normalization from `cache_key`
    /// (hashing `req.messages` verbatim) makes the first assertion fail,
    /// because the two wire shapes serialise to different bytes.
    #[test]
    fn cache_key_is_wire_shape_independent_for_text() {
        let req_string = make_request("openai/gpt-4o", vec![user_message("hello")], Some(0.7));
        let req_parts = make_request(
            "openai/gpt-4o",
            vec![user_message_parts("hello")],
            Some(0.7),
        );
        let key_string =
            ResponseCache::cache_key(&req_string).expect("test request must serialise");
        let key_parts = ResponseCache::cache_key(&req_parts).expect("test request must serialise");
        assert_eq!(
            key_string, key_parts,
            "Text(\"hello\") and Parts([Text(\"hello\")]) are the same prompt and \
             must share a cache key"
        );

        // A genuinely different prompt (in either shape) must NOT collide.
        let req_other = make_request(
            "openai/gpt-4o",
            vec![user_message_parts("goodbye")],
            Some(0.7),
        );
        let key_other = ResponseCache::cache_key(&req_other).expect("test request must serialise");
        assert_ne!(
            key_string, key_other,
            "different prompt text must produce a different cache key"
        );
    }

    /// The exact-cache key must distinguish two prompts that share text but
    /// carry different images — otherwise distinct multimodal prompts collide
    /// and serve each other's responses. The `cache_key` content-normalization
    /// deliberately keeps the ORIGINAL content (does not flatten) when image
    /// parts are present, so the images are part of the hashed bytes.
    #[test]
    fn cache_key_distinguishes_distinct_images() {
        use solvela_protocol::ImageUrl;
        fn image_message(text: &str, url: &str) -> ChatMessage {
            ChatMessage {
                role: Role::User,
                content: MessageContent::Parts(vec![
                    ContentPart::Text {
                        text: text.to_string(),
                    },
                    ContentPart::ImageUrl {
                        image_url: ImageUrl {
                            url: url.to_string(),
                            detail: None,
                        },
                    },
                ]),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }
        }
        let a = make_request(
            "openai/gpt-4o",
            vec![image_message("describe", "https://example.com/a.png")],
            None,
        );
        let b = make_request(
            "openai/gpt-4o",
            vec![image_message("describe", "https://example.com/b.png")],
            None,
        );
        assert_ne!(
            ResponseCache::cache_key(&a).expect("must serialise"),
            ResponseCache::cache_key(&b).expect("must serialise"),
            "distinct images with identical text must produce distinct cache keys"
        );

        // Same image + same text → same key (a legitimate hit is reachable).
        let a2 = make_request(
            "openai/gpt-4o",
            vec![image_message("describe", "https://example.com/a.png")],
            None,
        );
        assert_eq!(
            ResponseCache::cache_key(&a).expect("must serialise"),
            ResponseCache::cache_key(&a2).expect("must serialise"),
        );
    }

    /// Build a user message carrying one text part + one image part. Shared by
    /// the bounded-image-key tests below.
    fn image_message(text: &str, url: &str) -> ChatMessage {
        use solvela_protocol::ImageUrl;
        ChatMessage {
            role: Role::User,
            content: MessageContent::Parts(vec![
                ContentPart::Text {
                    text: text.to_string(),
                },
                ContentPart::ImageUrl {
                    image_url: ImageUrl {
                        url: url.to_string(),
                        detail: None,
                    },
                },
            ]),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// A valid `data:` URI with a base64 payload of `len` `A` characters.
    /// `image/png;base64` is in the supported allowlist so `ImageUrl::parse`
    /// returns `ParsedImage::Base64` (the path we want to fold to a short hash).
    fn data_uri(payload: &str) -> String {
        format!("data:image/png;base64,{payload}")
    }

    /// A `data:` URI image produces a stable key (same payload → same key, a
    /// legitimate hit is reachable) and a different payload → a different key.
    /// AND the key-input is BOUNDED: two payloads sharing a 10 KiB prefix that
    /// differ only in a late byte must still produce distinct keys via the
    /// payload hash — proving we hash the whole payload (through `sha256`),
    /// not a truncated prefix, while never folding the multi-megabyte raw bytes
    /// into the serialised key input verbatim.
    #[test]
    fn cache_key_data_uri_image_is_stable_distinct_and_bounded() {
        // Same data URI → same key.
        let payload_a = "A".repeat(10_000);
        let a = make_request(
            "openai/gpt-4o",
            vec![image_message("describe", &data_uri(&payload_a))],
            None,
        );
        let a2 = make_request(
            "openai/gpt-4o",
            vec![image_message("describe", &data_uri(&payload_a))],
            None,
        );
        assert_eq!(
            ResponseCache::cache_key(&a).expect("must serialise"),
            ResponseCache::cache_key(&a2).expect("must serialise"),
            "identical data-URI image + text must produce the same key (hit reachable)"
        );

        // Different data URI → different key.
        let payload_b = "B".repeat(10_000);
        let b = make_request(
            "openai/gpt-4o",
            vec![image_message("describe", &data_uri(&payload_b))],
            None,
        );
        assert_ne!(
            ResponseCache::cache_key(&a).expect("must serialise"),
            ResponseCache::cache_key(&b).expect("must serialise"),
            "distinct data-URI payloads must produce distinct keys"
        );

        // Bounded but lossless: two payloads sharing a 10 KiB prefix differing
        // only in the FINAL byte must still produce distinct keys. A truncated
        // (prefix-only) hash would collide them and serve one paid response for
        // the other; hashing the full payload via sha256 keeps them distinct.
        let shared_prefix = "C".repeat(10_240);
        let late_diff_x = format!("{shared_prefix}X");
        let late_diff_y = format!("{shared_prefix}Y");
        let x = make_request(
            "openai/gpt-4o",
            vec![image_message("describe", &data_uri(&late_diff_x))],
            None,
        );
        let y = make_request(
            "openai/gpt-4o",
            vec![image_message("describe", &data_uri(&late_diff_y))],
            None,
        );
        assert_ne!(
            ResponseCache::cache_key(&x).expect("must serialise"),
            ResponseCache::cache_key(&y).expect("must serialise"),
            "payloads differing only in a late byte must still produce distinct keys"
        );
    }

    /// A `data:` URI and an `http(s)` URL of the "same logical image" must NOT
    /// collide onto one key. We construct the degenerate case where the http
    /// URL's text equals the data URI's payload-hash representation namespace
    /// boundary — i.e. confirm the two reps live in different shapes
    /// (`sha256:<hex>` for a data URI vs the verbatim URL for http) so a crafted
    /// URL cannot be made to alias a payload hash. Serving an http-image
    /// response for a data-URI request (or vice versa) would be a wrong-answer
    /// money loss.
    #[test]
    fn cache_key_data_uri_and_http_url_do_not_collide() {
        let data = make_request(
            "openai/gpt-4o",
            vec![image_message("describe", &data_uri("AAAA"))],
            None,
        );
        let http = make_request(
            "openai/gpt-4o",
            vec![image_message("describe", "https://example.com/a.png")],
            None,
        );
        assert_ne!(
            ResponseCache::cache_key(&data).expect("must serialise"),
            ResponseCache::cache_key(&http).expect("must serialise"),
            "a data-URI image and an http URL must never share a cache key"
        );

        // Adversarial: an http URL whose literal text is the SAME string we'd
        // emit for the data-URI rep would still not collide, because the data
        // URI is replaced by `sha256:<hash-of-payload>`, not by its raw url. So
        // even if a client sends `https://.../sha256:<hex>` it hashes its own
        // (verbatim) url, never aliasing the payload hash of a different image.
        let crafted = make_request(
            "openai/gpt-4o",
            vec![image_message(
                "describe",
                "https://x/sha256:0123456789abcdef0123456789abcdef",
            )],
            None,
        );
        assert_ne!(
            ResponseCache::cache_key(&data).expect("must serialise"),
            ResponseCache::cache_key(&crafted).expect("must serialise"),
        );
    }

    #[test]
    fn test_cache_key_different_for_different_temperatures() {
        let req_a = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(0.7));
        let req_b = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(1.0));
        let req_c = make_request("openai/gpt-4o", vec![user_message("Hello")], None);
        let key_a = ResponseCache::cache_key(&req_a).expect("test request must serialise");
        let key_b = ResponseCache::cache_key(&req_b).expect("test request must serialise");
        let key_c = ResponseCache::cache_key(&req_c).expect("test request must serialise");
        assert_ne!(key_a, key_b);
        assert_ne!(key_a, key_c);
        assert_ne!(key_b, key_c);
    }

    #[tokio::test]
    async fn test_streaming_requests_not_cached() {
        // Use a bogus Redis URL — we should never connect because stream=true
        // causes an early return.
        let cache = ResponseCache::new("redis://127.0.0.1:1", CacheConfig::default())
            .expect("client creation should not connect");

        let req = ChatRequest {
            model: "openai/gpt-4o".to_string(),
            messages: vec![user_message("Hello")],
            max_tokens: None,
            temperature: None,
            top_p: None,
            stream: true,
            tools: None,
            tool_choice: None,
        };
        assert!(cache.get(&req).await.is_none());
    }

    #[tokio::test]
    async fn test_cache_disabled() {
        let config = CacheConfig {
            default_ttl_secs: 600,
            enabled: false,
        };
        let cache = ResponseCache::new("redis://127.0.0.1:1", config)
            .expect("client creation should not connect");

        let req = make_request("openai/gpt-4o", vec![user_message("Hello")], None);
        assert!(cache.get(&req).await.is_none());
    }

    /// CLAUDE.md rule #16: cache keys are wallet-agnostic. Two payers with
    /// identical (model, messages, temperature) MUST produce the same key,
    /// regardless of any other field on the request. We prove this structurally
    /// by varying every non-key field and asserting the hash is unchanged.
    #[test]
    fn cache_key_ignores_non_key_request_fields() {
        let base = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(0.7));
        let key_base = ResponseCache::cache_key(&base).expect("test request must serialise");

        // max_tokens differs — must not affect the key.
        let mut variant = base.clone();
        variant.max_tokens = Some(2048);
        assert_eq!(
            ResponseCache::cache_key(&variant).expect("test request must serialise"),
            key_base,
            "max_tokens must not be part of the cache key"
        );

        // top_p differs — must not affect the key.
        let mut variant = base.clone();
        variant.top_p = Some(0.9);
        assert_eq!(
            ResponseCache::cache_key(&variant).expect("test request must serialise"),
            key_base,
            "top_p must not be part of the cache key"
        );

        // stream differs — cache_key itself ignores it (the get/set methods
        // gate on stream separately).
        let mut variant = base.clone();
        variant.stream = true;
        assert_eq!(
            ResponseCache::cache_key(&variant).expect("test request must serialise"),
            key_base,
            "stream flag must not be part of the cache key"
        );
    }

    /// Message order is part of the conversation; two prompts with the same
    /// content in different order are NOT the same prompt.
    #[test]
    fn cache_key_is_sensitive_to_message_order() {
        let req_a = make_request(
            "openai/gpt-4o",
            vec![user_message("first"), user_message("second")],
            Some(0.7),
        );
        let req_b = make_request(
            "openai/gpt-4o",
            vec![user_message("second"), user_message("first")],
            Some(0.7),
        );
        assert_ne!(
            ResponseCache::cache_key(&req_a).expect("test request must serialise"),
            ResponseCache::cache_key(&req_b).expect("test request must serialise"),
            "message order must affect the cache key"
        );
    }

    /// Tool spec is part of the response shape (`tool_calls` vs prose). Two
    /// requests with the same prompt + temperature but different tools MUST
    /// produce different keys; otherwise an agent that paid for a tool-capable
    /// response would receive a prose response cached by a tool-less peer
    /// (a wrong-answer money loss). Falsifiability: removing `req.tools` from
    /// `cache_key` would make this test fail.
    #[test]
    fn cache_key_is_sensitive_to_tools() {
        use solvela_protocol::{FunctionDefinitionInner, ToolDefinition};
        let base = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(0.7));
        let with_tools = {
            let mut r = base.clone();
            r.tools = Some(vec![ToolDefinition {
                r#type: "function".to_string(),
                function: FunctionDefinitionInner {
                    name: "get_weather".to_string(),
                    description: Some("Get weather".to_string()),
                    parameters: Some(serde_json::json!({"type":"object"})),
                },
            }]);
            r
        };
        let with_other_tools = {
            let mut r = base.clone();
            r.tools = Some(vec![ToolDefinition {
                r#type: "function".to_string(),
                function: FunctionDefinitionInner {
                    name: "search_web".to_string(),
                    description: Some("Search the web".to_string()),
                    parameters: Some(serde_json::json!({"type":"object"})),
                },
            }]);
            r
        };
        let key_base = ResponseCache::cache_key(&base).expect("must serialise");
        let key_tools = ResponseCache::cache_key(&with_tools).expect("must serialise");
        let key_other = ResponseCache::cache_key(&with_other_tools).expect("must serialise");
        assert_ne!(
            key_base, key_tools,
            "request with tools must hash differently from request with no tools"
        );
        assert_ne!(
            key_tools, key_other,
            "different tool definitions must produce different keys"
        );
    }

    /// `tool_choice` (auto vs none vs a specific function) forces the response
    /// shape. Two requests with identical prompts + tools but different
    /// `tool_choice` MUST produce different keys.
    #[test]
    fn cache_key_is_sensitive_to_tool_choice() {
        let base = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(0.7));
        let auto = {
            let mut r = base.clone();
            r.tool_choice = Some(serde_json::json!("auto"));
            r
        };
        let none = {
            let mut r = base.clone();
            r.tool_choice = Some(serde_json::json!("none"));
            r
        };
        let key_base = ResponseCache::cache_key(&base).expect("must serialise");
        let key_auto = ResponseCache::cache_key(&auto).expect("must serialise");
        let key_none = ResponseCache::cache_key(&none).expect("must serialise");
        assert_ne!(key_base, key_auto);
        assert_ne!(key_auto, key_none);
        assert_ne!(key_base, key_none);
    }

    /// Adding a message must produce a different key.
    #[test]
    fn cache_key_is_sensitive_to_message_count() {
        let req_a = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(0.7));
        let req_b = make_request(
            "openai/gpt-4o",
            vec![user_message("Hello"), user_message("again")],
            Some(0.7),
        );
        assert_ne!(
            ResponseCache::cache_key(&req_a).expect("test request must serialise"),
            ResponseCache::cache_key(&req_b).expect("test request must serialise"),
            "additional messages must affect the cache key"
        );
    }

    /// `set` early-exits for streaming requests without spawning a Redis writer.
    /// We use a bogus Redis URL — if `set` tried to connect, the spawned task
    /// would log a warning, but the function itself must return immediately.
    #[tokio::test]
    async fn set_short_circuits_for_streaming_requests() {
        let cache = ResponseCache::new("redis://127.0.0.1:1", CacheConfig::default())
            .expect("client creation should not connect");

        let req = ChatRequest {
            model: "openai/gpt-4o".to_string(),
            messages: vec![user_message("Hello")],
            max_tokens: None,
            temperature: None,
            top_p: None,
            stream: true,
            tools: None,
            tool_choice: None,
        };
        let response = ChatResponse {
            id: "test".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "gpt-4o".to_string(),
            choices: vec![],
            usage: None,
        };
        // Should return without panic; nothing to assert besides "doesn't try to connect".
        cache.set(&req, &response).await;
    }

    /// `set` refuses to cache a response with no `usage` block — serving it on a
    /// future hit would skip both `log_spend` reconciliation branches in
    /// `chat/mod.rs` (one needs `usage`, the other needs a semantic
    /// `cost_outcome`), permanently stranding the `check_budget` reservation
    /// and draining the wallet's budget by `estimated_cost` per repeat.
    ///
    /// Falsifiability: asserts the `solvela_exact_cache_write_skipped_no_usage_total`
    /// counter incremented. A regression that deletes the guard would let
    /// execution continue past the counter call into the Redis path — the
    /// counter would NOT increment and this test would fail. (Earlier draft
    /// of this test passed vacuously because the bad-port Redis connection
    /// failed at step 2, observationally identical to the guard firing.)
    #[tokio::test]
    async fn set_refuses_to_cache_response_without_usage() {
        let handle = install_test_recorder();
        let before = counter_value(&handle, "solvela_exact_cache_write_skipped_no_usage_total");

        let cache = ResponseCache::new("redis://127.0.0.1:1", CacheConfig::default())
            .expect("client creation should not connect");
        let req = make_request("openai/gpt-4o", vec![user_message("Hello")], None);
        let response = ChatResponse {
            id: "test".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "gpt-4o".to_string(),
            choices: vec![],
            usage: None,
        };
        cache.set(&req, &response).await;

        let after = counter_value(&handle, "solvela_exact_cache_write_skipped_no_usage_total");
        assert_eq!(
            after,
            before + 1,
            "guard must increment the skip counter; otherwise a regression \
             that removed the guard would slip past this test"
        );
    }

    /// `set` early-exits when caching is disabled.
    #[tokio::test]
    async fn set_short_circuits_when_disabled() {
        let config = CacheConfig {
            default_ttl_secs: 600,
            enabled: false,
        };
        let cache = ResponseCache::new("redis://127.0.0.1:1", config)
            .expect("client creation should not connect");

        let req = make_request("openai/gpt-4o", vec![user_message("Hello")], None);
        let response = ChatResponse {
            id: "test".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "gpt-4o".to_string(),
            choices: vec![],
            usage: None,
        };
        cache.set(&req, &response).await;
    }

    /// Read-side defense-in-depth: `get` must evict any cached entry whose
    /// `usage` is `None`, even though the write-side guard now prevents new
    /// such entries from being stored. Verifies the read-side branch via a
    /// direct `SET` of a malformed entry that the write guard would have
    /// rejected — simulating either a pre-guard legacy entry or a future
    /// bypass.
    ///
    /// Gated on a reachable Redis (any Redis works; this test does not need
    /// RediSearch). The counter assertion makes this falsifiable: removing
    /// the read-side `usage.is_none()` check would surface as a non-incremented
    /// `solvela_exact_cache_read_evicted_no_usage_total` and a returned
    /// `Some(_)` instead of `None`.
    #[tokio::test]
    async fn get_evicts_cached_response_without_usage() {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".to_string());

        // Probe the connection first so we can skip cleanly if Redis is down.
        let client = match redis::Client::open(redis_url.clone()) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skipping: cannot open Redis client ({e})");
                return;
            }
        };
        let mut conn = match client.get_multiplexed_async_connection().await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skipping: Redis unreachable ({e})");
                return;
            }
        };

        let cache = ResponseCache::new(&redis_url, CacheConfig::default())
            .expect("client creation should not connect");

        // Build a request whose key we can compute, then SET a usage-less
        // ChatResponse JSON under that key — bypassing the write guard.
        let req = make_request(
            "openai/gpt-4o",
            vec![user_message("read-side guard probe — usage-less seed")],
            None,
        );
        let key = ResponseCache::cache_key(&req).expect("test request must serialise");
        let usage_less = ChatResponse {
            id: "legacy-no-usage".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "gpt-4o".to_string(),
            choices: vec![],
            usage: None,
        };
        let json = serde_json::to_string(&usage_less).unwrap();
        let _: () = redis::cmd("SETEX")
            .arg(&key)
            .arg(60_u64)
            .arg(&json)
            .query_async(&mut conn)
            .await
            .expect("seed write must succeed");

        let handle = install_test_recorder();
        let before = counter_value(&handle, "solvela_exact_cache_read_evicted_no_usage_total");

        let result = cache.get(&req).await;
        assert!(
            result.is_none(),
            "cached entry with usage:None must be treated as miss; got {result:?}"
        );

        let after = counter_value(&handle, "solvela_exact_cache_read_evicted_no_usage_total");
        assert_eq!(
            after,
            before + 1,
            "read-side guard must increment the eviction counter; otherwise \
             a regression that removed the guard would slip past this test"
        );

        // Clean up the seeded key so a second run of this test starts clean.
        let _: () = redis::cmd("DEL")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .unwrap_or(());
    }
}
