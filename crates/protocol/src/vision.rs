//! Multimodal content wire-format types.
//!
//! **Status: wired in (vision supported).** [`MessageContent`] is the type
//! of `ChatMessage::content`. It accepts both OpenAI shapes for message
//! content: a plain JSON string (`"hello"`) deserializes to
//! [`MessageContent::Text`], and an array of content parts
//! (`[{"type":"text","text":"hello"}]`) deserializes to
//! [`MessageContent::Parts`]. The enum is `#[serde(untagged)]`, so each
//! variant serializes back to its original wire shape — string content
//! round-trips as a JSON string, preserving backward compatibility for
//! existing string-only clients.
//!
//! Image parts are NOT silently dropped. As of PR #2, vision is supported
//! and gated on the resolved model's `supports_vision` capability: a request
//! carrying image content for a non-vision model is rejected with
//! `415 Unsupported Media Type` in the chat route. For vision-capable models
//! the image parts are translated to each provider's native multimodal wire
//! format (Anthropic image content blocks, Gemini `inlineData`/`fileData`
//! parts) or passed through natively for OpenAI-format providers.
//!
//! For the string-based provider adapters (Anthropic / Google), text
//! parts are flattened to a single string via [`MessageContent::as_text`]
//! when no images are present, and translated to native image blocks when
//! they are. For OpenAI-format providers (OpenAI / xAI / DeepSeek), the
//! request is serialized through directly, so array content (text + images)
//! passes through natively to the upstream API.
//!
//! [`ImageUrl::parse`] decodes an `image_url.url` into a [`ParsedImage`]:
//! either an inline base64 payload (from a `data:` URI) or a remote
//! `http(s)` URL. Malformed inputs return a clear error and never panic.

use serde::{Deserialize, Serialize};

/// Maximum accepted size of an `image_url.url` field (bytes of the raw string).
///
/// This is the single chokepoint that bounds the cost of decoding/splitting a
/// `data:` URI and of folding image bytes into the cache key. A multi-megabyte
/// base64 blob is rejected here before any per-byte work, covering every caller
/// (the Anthropic/Gemini adapters and both cache layers) at once. 10 MiB of
/// base64 decodes to ~7.5 MB of image bytes — comfortably inside both
/// providers' inline-image budgets (Anthropic 32 MB request, Gemini 20 MB
/// request) while keeping a single hostile request from pinning a worker.
pub const MAX_DATA_URI_BYTES: usize = 10 * 1024 * 1024;

/// Errors from [`ImageUrl::parse`]. Typed so the chat route can map a malformed
/// client image to a 4xx (client error) rather than a 500.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseImageError {
    /// The `url` field was empty (or all whitespace).
    #[error("image_url.url is empty")]
    Empty,
    /// A `data:` URI with no `,` separating metadata from payload.
    #[error("malformed data URI: missing ',' separator")]
    MissingComma,
    /// A `data:` URI lacking the `;base64` token (non-base64/URL-encoded data).
    #[error("unsupported data URI: only base64-encoded image data URIs are supported (expected ';base64' before the comma)")]
    NotBase64,
    /// A `data:` URI whose base64 payload was empty.
    #[error("malformed data URI: empty base64 payload")]
    EmptyPayload,
    /// The `url` field exceeded [`MAX_DATA_URI_BYTES`].
    #[error("image_url.url too large: {0} bytes exceeds the {1}-byte limit")]
    TooLarge(usize, usize),
    /// The scheme was neither a `data:` URI nor an `http(s)` URL, OR the
    /// declared media type is not in the supported image allowlist. The
    /// payload is the offending value (URL scheme or media type), already
    /// length-bounded so it is safe to surface.
    #[error("unsupported image_url: {0}")]
    UnsupportedScheme(String),
}

/// The image media types accepted by [`ImageUrl::parse`].
///
/// Restricted to the INTERSECTION of formats every vision provider the gateway
/// routes to can decode, so an allowed `data:` URI never settles payment and
/// then gets rejected by the upstream provider. Verified 2026-06-03 against the
/// live Anthropic vision docs
/// (platform.claude.com/docs/en/build-with-claude/vision — jpeg/png/gif/webp)
/// and the Gemini image-understanding docs
/// (ai.google.dev/gemini-api/docs/image-understanding — png/jpeg/webp/heic/heif).
/// `gif` is deliberately EXCLUDED: it is Anthropic-only (Gemini cannot decode
/// it), so accepting a `data:image/gif` would let an agent pay for a request a
/// Gemini-routed model can never serve. `heic/heif` are likewise excluded
/// (Gemini-only). The kept set (png/jpeg/webp) is the common subset that
/// decodes on every provider — no type is ever billed-then-rejected for being
/// undecodable. (Remote `http(s)` image URLs carry no declared media type and
/// so bypass this allowlist; a `.gif` URL routed to Gemini remains a residual
/// pay-then-fail edge the gateway cannot detect without fetching — see
/// `providers/google.rs::mime_type_from_url`.)
const SUPPORTED_MEDIA_TYPES: [&str; 3] = ["image/png", "image/jpeg", "image/webp"];

/// An image URL with optional detail level.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ImageUrl {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub detail: Option<String>,
}

/// The decoded form of an [`ImageUrl::url`] field.
///
/// OpenAI's `image_url.url` is overloaded: it is either a `data:` URI
/// carrying inline base64 bytes, or an `http(s)` URL pointing at a remote
/// image. Providers need these split apart — Anthropic uses
/// `source.type = "base64"` vs `"url"`, Gemini uses `inlineData` vs
/// `fileData` — so this enum is the single normalized representation both
/// adapters translate from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedImage {
    /// Inline base64 image decoded from a `data:` URI. `media_type` is the
    /// MIME type (e.g. `image/png`); `data` is the raw base64 payload (the
    /// substring AFTER the `,`), passed through verbatim — not re-decoded.
    Base64 { media_type: String, data: String },
    /// A remote `http(s)` image URL, passed through verbatim.
    Url { url: String },
}

impl ImageUrl {
    /// Parse [`Self::url`] into a [`ParsedImage`].
    ///
    /// Accepts two shapes (mirroring the OpenAI `image_url.url` contract):
    /// - `data:[<media-type>][;base64],<data>` → [`ParsedImage::Base64`].
    ///   The media-type defaults to `image/jpeg` when omitted (matching the
    ///   `data:` URI spec's default of `text/plain` being inappropriate for
    ///   image content; image providers require an image MIME type). Only
    ///   base64-encoded data URIs are accepted — a `data:` URI WITHOUT the
    ///   `;base64` token (i.e. percent/URL-encoded inline text) is rejected,
    ///   because the downstream provider image fields require base64 bytes
    ///   and silently forwarding URL-encoded text would corrupt the image.
    ///   The declared media type must be in the supported allowlist
    ///   ([`SUPPORTED_MEDIA_TYPES`]); an unsupported type is rejected rather
    ///   than forwarded to a provider that cannot decode it.
    /// - `http://...` / `https://...` → [`ParsedImage::Url`].
    ///
    /// Returns a typed [`ParseImageError`] for any other shape (empty, oversize,
    /// missing comma in a `data:` URI, non-base64, empty payload, unsupported
    /// scheme/media-type). Never panics: every slice is bounds-checked via
    /// `split_once` / `strip_prefix`.
    ///
    /// Size: the raw `url` is rejected up front if it exceeds
    /// [`MAX_DATA_URI_BYTES`], BEFORE any decode/split, so this method bounds
    /// the work a single request can trigger across every caller.
    pub fn parse(&self) -> Result<ParsedImage, ParseImageError> {
        // Size cap FIRST — before any trim/split/decode — so an oversize blob
        // never reaches per-byte work. Measured on the raw, untrimmed string.
        if self.url.len() > MAX_DATA_URI_BYTES {
            return Err(ParseImageError::TooLarge(
                self.url.len(),
                MAX_DATA_URI_BYTES,
            ));
        }

        let url = self.url.trim();
        if url.is_empty() {
            return Err(ParseImageError::Empty);
        }

        if let Some(rest) = url.strip_prefix("data:") {
            // `data:[<media-type>][;base64],<data>`. Split on the FIRST comma:
            // the metadata (media-type + encoding flags) precedes it, the
            // payload follows. A data URI without a comma is malformed.
            let (meta, data) = rest.split_once(',').ok_or(ParseImageError::MissingComma)?;

            // The metadata is `<media-type>;base64` (media-type optional).
            // We require the `;base64` token; URL-encoded (non-base64) data
            // URIs are rejected rather than forwarded as corrupt image bytes.
            let base64_meta = meta
                .strip_suffix(";base64")
                .ok_or(ParseImageError::NotBase64)?;

            let media_type = if base64_meta.is_empty() {
                // No media-type segment (`data:;base64,...`) → default. Image
                // providers require an image MIME type, so default to JPEG
                // rather than the `data:` spec's text/plain default.
                "image/jpeg".to_string()
            } else {
                base64_meta.to_string()
            };

            // Reject media types no downstream provider can decode.
            if !SUPPORTED_MEDIA_TYPES.contains(&media_type.as_str()) {
                return Err(ParseImageError::UnsupportedScheme(media_type));
            }

            // NOTE: we do NOT trim the payload. base64 contains no internal
            // whitespace in well-formed input, and trimming would silently
            // "repair" a malformed payload (masking a client bug); the payload
            // is passed through verbatim for the provider to validate/decode.
            if data.is_empty() {
                return Err(ParseImageError::EmptyPayload);
            }

            return Ok(ParsedImage::Base64 {
                media_type,
                data: data.to_string(),
            });
        }

        if url.starts_with("http://") || url.starts_with("https://") {
            return Ok(ParsedImage::Url {
                url: url.to_string(),
            });
        }

        // Surface only the scheme prefix (length-bounded), never the full URL.
        let prefix: String = url.chars().take(16).collect();
        Err(ParseImageError::UnsupportedScheme(format!(
            "expected a 'data:' URI or an http(s) URL, got {prefix:?}"
        )))
    }
}

/// A single part of multi-modal content (text or image).
///
/// The ABSENCE of a `#[serde(other)]` / catch-all variant is DELIBERATE
/// policy, not an oversight. An unknown `type` value fails to deserialize
/// and is rejected loudly at the wire boundary (4xx) rather than being
/// silently coerced into an "unknown" variant. This keeps billing and
/// capability gating sound: a client cannot smuggle a content part of an
/// unrecognized type past cost accounting or the image-rejection check by
/// labelling it with a `type` we don't model. Add a real variant here when
/// a new content kind is intentionally supported — never a catch-all.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
}

/// Message content that is either a plain string or an array of content
/// parts, matching the OpenAI chat API's `content` field.
///
/// `#[serde(untagged)]` is essential: a JSON string deserializes to
/// [`MessageContent::Text`], a JSON array to [`MessageContent::Parts`],
/// and each variant serializes back to its original wire shape. This keeps
/// existing string-only clients fully backward-compatible.
///
/// VARIANT ORDER IS LOAD-BEARING. `#[serde(untagged)]` tries variants
/// top-to-bottom and takes the first that deserializes. `Text(String)` MUST
/// stay declared before `Parts(Vec<ContentPart>)` so a bare JSON string is
/// disambiguated as `Text` and a JSON array as `Parts`. Reordering these (or
/// inserting a new string-shaped variant ahead of `Text`) would silently
/// change how the wire `content` field parses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl Default for MessageContent {
    fn default() -> Self {
        MessageContent::Text(String::new())
    }
}

impl From<String> for MessageContent {
    fn from(value: String) -> Self {
        MessageContent::Text(value)
    }
}

impl From<&str> for MessageContent {
    fn from(value: &str) -> Self {
        MessageContent::Text(value.to_string())
    }
}

impl From<Vec<ContentPart>> for MessageContent {
    fn from(value: Vec<ContentPart>) -> Self {
        MessageContent::Parts(value)
    }
}

impl MessageContent {
    /// Flatten this content to a plain string.
    ///
    /// For [`MessageContent::Text`] this borrows the inner string
    /// (zero-copy). For [`MessageContent::Parts`] it collects only the
    /// [`ContentPart::Text`] parts; if there is exactly one text part it
    /// borrows that part's string (zero-copy), and it only allocates when
    /// joining 2+ text parts (separated by a single space `" "`). An empty
    /// `Parts` list, or one with no text parts, yields a borrowed empty
    /// string. Image parts contribute no text here — callers that need the
    /// images (the provider adapters) iterate the parts directly via the
    /// `Parts` variant; `as_text` is the text-only flattening used for prompt
    /// guards, system-message extraction, and routing/scoring.
    ///
    /// The join separator is a single space so the flattened text reads
    /// naturally for guard scanning and string-based provider adapters.
    pub fn as_text(&self) -> std::borrow::Cow<'_, str> {
        match self {
            MessageContent::Text(s) => std::borrow::Cow::Borrowed(s),
            MessageContent::Parts(parts) => {
                // Collect the text-part slices up front so we know the exact
                // count and can size the join allocation precisely.
                let texts: Vec<&str> = parts
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::Text { text } => Some(text.as_str()),
                        ContentPart::ImageUrl { .. } => None,
                    })
                    .collect();
                match texts.as_slice() {
                    // No text parts at all → borrow an empty static str.
                    [] => std::borrow::Cow::Borrowed(""),
                    // Exactly one text part → borrow it (zero-copy).
                    [only] => std::borrow::Cow::Borrowed(only),
                    // 2+ text parts → allocate ONCE for any number of parts and
                    // space-join. Capacity = sum(lengths) + (count - 1)
                    // separators, so the buffer never reallocates mid-join.
                    many => {
                        let total: usize = many.iter().map(|s| s.len()).sum();
                        let mut joined = String::with_capacity(total + many.len() - 1);
                        joined.push_str(many[0]);
                        for part in &many[1..] {
                            joined.push(' ');
                            joined.push_str(part);
                        }
                        std::borrow::Cow::Owned(joined)
                    }
                }
            }
        }
    }

    /// True when the flattened TEXT content is empty.
    ///
    /// Reflects text emptiness only: a `Parts` value whose only members are
    /// image parts is considered empty here (it has no text). The chat route's
    /// empty-prompt check requires at least one non-empty User *text*
    /// message, so an image-only User turn still needs accompanying text to be
    /// accepted. Computed without allocating.
    pub fn is_empty(&self) -> bool {
        match self {
            MessageContent::Text(s) => s.is_empty(),
            MessageContent::Parts(parts) => parts.iter().all(|p| match p {
                ContentPart::Text { text } => text.is_empty(),
                ContentPart::ImageUrl { .. } => true,
            }),
        }
    }

    /// True if any [`ContentPart::ImageUrl`] is present.
    ///
    /// Used by the chat route's capability gate (reject image content for a
    /// non-vision model), by the image cost-estimate path, and by the cache
    /// layers to decide whether to flatten content for the key.
    pub fn has_image_parts(&self) -> bool {
        match self {
            MessageContent::Text(_) => false,
            MessageContent::Parts(parts) => parts
                .iter()
                .any(|p| matches!(p, ContentPart::ImageUrl { .. })),
        }
    }

    /// Iterate the [`ImageUrl`] of every image part, in order.
    ///
    /// Empty for [`MessageContent::Text`] and for any `Parts` list with no
    /// image parts. Used by the cost-estimate path (per-image token
    /// contribution keyed on `detail`) and by the cache image-rep folding.
    pub fn image_urls(&self) -> impl Iterator<Item = &ImageUrl> {
        let slice: &[ContentPart] = match self {
            MessageContent::Text(_) => &[],
            MessageContent::Parts(parts) => parts.as_slice(),
        };
        slice.iter().filter_map(|p| match p {
            ContentPart::ImageUrl { image_url } => Some(image_url),
            ContentPart::Text { .. } => None,
        })
    }
}

impl std::fmt::Display for MessageContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_part_text_and_image() {
        let parts = vec![
            ContentPart::Text {
                text: "What's in this image?".to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "https://example.com/img.png".to_string(),
                    detail: Some("high".to_string()),
                },
            },
        ];
        let json = serde_json::to_string(&parts).unwrap();
        let deser: Vec<ContentPart> = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.len(), 2);
        match &deser[0] {
            ContentPart::Text { text } => assert_eq!(text, "What's in this image?"),
            _ => panic!("expected Text variant"),
        }
        match &deser[1] {
            ContentPart::ImageUrl { image_url } => {
                assert_eq!(image_url.url, "https://example.com/img.png");
                assert_eq!(image_url.detail.as_deref(), Some("high"));
            }
            _ => panic!("expected ImageUrl variant"),
        }
    }

    #[test]
    fn test_message_content_deserializes_string() {
        let mc: MessageContent = serde_json::from_str(r#""Hello!""#).unwrap();
        assert_eq!(mc, MessageContent::Text("Hello!".to_string()));
        assert_eq!(mc.as_text(), "Hello!");
    }

    #[test]
    fn test_message_content_deserializes_parts_array() {
        let mc: MessageContent =
            serde_json::from_str(r#"[{"type":"text","text":"Hello!"}]"#).unwrap();
        assert!(matches!(mc, MessageContent::Parts(_)));
        assert_eq!(mc.as_text(), "Hello!");
    }

    #[test]
    fn test_message_content_variant_order_string_is_text() {
        // VARIANT ORDER REGRESSION GUARD. `#[serde(untagged)]` takes the first
        // variant that deserializes, so `Text` MUST stay declared before
        // `Parts`. A bare JSON string must land as `Text`, never `Parts`.
        // Fails loudly in CI if anyone reorders the variants.
        let mc: MessageContent = serde_json::from_str("\"hello\"").unwrap();
        assert!(
            matches!(mc, MessageContent::Text(_)),
            "a bare JSON string must deserialize to the Text variant"
        );
        assert!(!matches!(mc, MessageContent::Parts(_)));
    }

    #[test]
    fn test_message_content_variant_order_array_is_parts() {
        // VARIANT ORDER REGRESSION GUARD (see above). A JSON array of content
        // parts must land as `Parts`, never `Text`.
        let mc: MessageContent =
            serde_json::from_str("[{\"type\":\"text\",\"text\":\"x\"}]").unwrap();
        assert!(
            matches!(mc, MessageContent::Parts(_)),
            "a JSON array must deserialize to the Parts variant"
        );
        assert!(!matches!(mc, MessageContent::Text(_)));
    }

    #[test]
    fn test_message_content_text_serializes_to_json_string() {
        // Wire-compat regression guard: a Text variant must serialize to a
        // bare JSON string, NOT an array/object.
        let mc = MessageContent::Text("hi".to_string());
        let json = serde_json::to_string(&mc).unwrap();
        assert_eq!(json, r#""hi""#);
    }

    #[test]
    fn test_message_content_parts_round_trip() {
        let mc = MessageContent::Parts(vec![ContentPart::Text {
            text: "hi".to_string(),
        }]);
        let json = serde_json::to_value(&mc).unwrap();
        assert!(json.is_array());
        let back: MessageContent = serde_json::from_value(json).unwrap();
        assert_eq!(back, mc);
    }

    #[test]
    fn test_message_content_multi_text_parts_join_with_space() {
        let mc = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "line one".to_string(),
            },
            ContentPart::Text {
                text: "line two".to_string(),
            },
        ]);
        assert_eq!(mc.as_text(), "line one line two");
    }

    #[test]
    fn test_message_content_three_text_parts_join_with_space() {
        let mc = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "a".to_string(),
            },
            ContentPart::Text {
                text: "b".to_string(),
            },
            ContentPart::Text {
                text: "c".to_string(),
            },
        ]);
        assert_eq!(mc.as_text(), "a b c");
    }

    #[test]
    fn test_message_content_single_text_part_borrows() {
        // Exactly one text part must be borrowed (zero-copy), not allocated.
        let mc = MessageContent::Parts(vec![ContentPart::Text {
            text: "solo".to_string(),
        }]);
        assert!(matches!(mc.as_text(), std::borrow::Cow::Borrowed("solo")));
    }

    #[test]
    fn test_message_content_no_text_parts_borrows_empty() {
        let mc = MessageContent::Parts(vec![ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: "https://example.com/img.png".to_string(),
                detail: None,
            },
        }]);
        assert!(matches!(mc.as_text(), std::borrow::Cow::Borrowed("")));
    }

    #[test]
    fn test_message_content_has_image_parts() {
        let text = MessageContent::Text("hi".to_string());
        assert!(!text.has_image_parts());

        let text_only_parts = MessageContent::Parts(vec![ContentPart::Text {
            text: "hi".to_string(),
        }]);
        assert!(!text_only_parts.has_image_parts());

        let with_image = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "look".to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "https://example.com/img.png".to_string(),
                    detail: None,
                },
            },
        ]);
        assert!(with_image.has_image_parts());
    }

    #[test]
    fn test_message_content_display_delegates_to_as_text() {
        assert_eq!(
            MessageContent::Text("hello".to_string()).to_string(),
            "hello"
        );
        let parts = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "a".to_string(),
            },
            ContentPart::Text {
                text: "b".to_string(),
            },
        ]);
        assert_eq!(parts.to_string(), "a b");
    }

    #[test]
    fn test_message_content_mixed_text_and_image_ignores_image() {
        let mc = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "describe this".to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "https://example.com/img.png".to_string(),
                    detail: None,
                },
            },
        ]);
        assert_eq!(mc.as_text(), "describe this");
    }

    #[test]
    fn test_message_content_empty_parts_is_empty() {
        let mc = MessageContent::Parts(vec![]);
        assert!(mc.is_empty());
        assert_eq!(mc.as_text(), "");

        let only_image = MessageContent::Parts(vec![ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: "https://example.com/img.png".to_string(),
                detail: None,
            },
        }]);
        assert!(only_image.is_empty());
    }

    // =========================================================================
    // ImageUrl::parse — data-URI / http(s) decoding (PR #2)
    // =========================================================================

    fn img(url: &str) -> ImageUrl {
        ImageUrl {
            url: url.to_string(),
            detail: None,
        }
    }

    #[test]
    fn parse_data_uri_base64_with_media_type() {
        let got = img("data:image/png;base64,iVBORw0KGgo=").parse().unwrap();
        assert_eq!(
            got,
            ParsedImage::Base64 {
                media_type: "image/png".to_string(),
                data: "iVBORw0KGgo=".to_string(),
            }
        );
    }

    #[test]
    fn parse_data_uri_base64_defaults_media_type_when_omitted() {
        // `data:;base64,...` — no media-type segment → default to image/jpeg.
        let got = img("data:;base64,QUJD").parse().unwrap();
        assert_eq!(
            got,
            ParsedImage::Base64 {
                media_type: "image/jpeg".to_string(),
                data: "QUJD".to_string(),
            }
        );
    }

    #[test]
    fn parse_http_and_https_urls_pass_through() {
        assert_eq!(
            img("https://example.com/cat.png").parse().unwrap(),
            ParsedImage::Url {
                url: "https://example.com/cat.png".to_string()
            }
        );
        assert_eq!(
            img("http://example.com/dog.jpg").parse().unwrap(),
            ParsedImage::Url {
                url: "http://example.com/dog.jpg".to_string()
            }
        );
    }

    #[test]
    fn parse_rejects_malformed_inputs() {
        // Empty → typed Empty.
        assert_eq!(img("").parse(), Err(ParseImageError::Empty));
        assert_eq!(img("   ").parse(), Err(ParseImageError::Empty));
        // data: URI without a comma separator → MissingComma.
        assert_eq!(
            img("data:image/png;base64").parse(),
            Err(ParseImageError::MissingComma)
        );
        // data: URI that is not base64-encoded (URL-encoded inline text) →
        // NotBase64.
        assert_eq!(
            img("data:text/plain,hello%20world").parse(),
            Err(ParseImageError::NotBase64)
        );
        assert_eq!(
            img("data:image/png,rawbytes").parse(),
            Err(ParseImageError::NotBase64)
        );
        // data: URI with empty base64 payload → EmptyPayload.
        assert_eq!(
            img("data:image/png;base64,").parse(),
            Err(ParseImageError::EmptyPayload)
        );
        // Non-http(s) scheme → UnsupportedScheme.
        assert!(matches!(
            img("ftp://example.com/x.png").parse(),
            Err(ParseImageError::UnsupportedScheme(_))
        ));
        assert!(matches!(
            img("file:///etc/passwd").parse(),
            Err(ParseImageError::UnsupportedScheme(_))
        ));
        assert!(matches!(
            img("javascript:alert(1)").parse(),
            Err(ParseImageError::UnsupportedScheme(_))
        ));
        // Bare string, no scheme.
        assert!(matches!(
            img("just-some-text").parse(),
            Err(ParseImageError::UnsupportedScheme(_))
        ));
    }

    #[test]
    fn parse_rejects_oversize_data_uri_before_decoding() {
        // A url longer than the cap is rejected with TooLarge, regardless of
        // whether the rest of the shape is valid — and BEFORE any split/decode.
        let huge = "data:image/png;base64,".to_string() + &"A".repeat(MAX_DATA_URI_BYTES);
        let err = img(&huge).parse().unwrap_err();
        match err {
            ParseImageError::TooLarge(len, cap) => {
                assert!(len > cap);
                assert_eq!(cap, MAX_DATA_URI_BYTES);
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }

        // A url exactly at the cap is NOT rejected for size (it parses on shape;
        // here the payload makes it a valid base64 data URI).
        let at_cap_payload_len = MAX_DATA_URI_BYTES - "data:image/png;base64,".len();
        let at_cap = "data:image/png;base64,".to_string() + &"A".repeat(at_cap_payload_len);
        assert_eq!(at_cap.len(), MAX_DATA_URI_BYTES);
        assert!(img(&at_cap).parse().is_ok());
    }

    #[test]
    fn parse_rejects_unsupported_media_type() {
        // image/svg+xml is not in the provider allowlist → UnsupportedScheme.
        assert!(matches!(
            img("data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=").parse(),
            Err(ParseImageError::UnsupportedScheme(_))
        ));
        // image/bmp likewise rejected.
        assert!(matches!(
            img("data:image/bmp;base64,Qk0=").parse(),
            Err(ParseImageError::UnsupportedScheme(_))
        ));
        // image/gif is deliberately rejected: it is Anthropic-only and Gemini
        // cannot decode it, so accepting it would let an agent pay for a request
        // a Gemini-routed model can never serve. Excluded from the common subset.
        assert!(matches!(
            img("data:image/gif;base64,R0lGODdh").parse(),
            Err(ParseImageError::UnsupportedScheme(_))
        ));
        // Each allowlisted type is accepted.
        for mt in ["image/png", "image/jpeg", "image/webp"] {
            let uri = format!("data:{mt};base64,QUJD");
            let parsed = img(&uri).parse().expect("allowlisted type must parse");
            assert_eq!(
                parsed,
                ParsedImage::Base64 {
                    media_type: mt.to_string(),
                    data: "QUJD".to_string(),
                }
            );
        }
    }

    #[test]
    fn parse_does_not_re_decode_payload() {
        // The base64 payload is passed through verbatim (providers re-decode).
        // Even an invalid-base64 string is forwarded as-is; validation is the
        // provider's job, and `parse` must not panic or alter the bytes.
        let got = img("data:image/webp;base64,!!!not-valid-b64!!!")
            .parse()
            .unwrap();
        assert_eq!(
            got,
            ParsedImage::Base64 {
                media_type: "image/webp".to_string(),
                data: "!!!not-valid-b64!!!".to_string(),
            }
        );
    }

    #[test]
    fn image_urls_iterates_only_image_parts_in_order() {
        let mc = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "look".to_string(),
            },
            ContentPart::ImageUrl {
                image_url: img("https://example.com/a.png"),
            },
            ContentPart::ImageUrl {
                image_url: img("https://example.com/b.png"),
            },
        ]);
        let urls: Vec<&str> = mc.image_urls().map(|u| u.url.as_str()).collect();
        assert_eq!(
            urls,
            vec!["https://example.com/a.png", "https://example.com/b.png"]
        );

        // Text content and text-only parts yield no images.
        assert_eq!(
            MessageContent::Text("hi".to_string()).image_urls().count(),
            0
        );
    }

    #[test]
    fn test_message_content_from_conversions() {
        assert_eq!(
            MessageContent::from("x"),
            MessageContent::Text("x".to_string())
        );
        assert_eq!(
            MessageContent::from("y".to_string()),
            MessageContent::Text("y".to_string())
        );
        let parts: MessageContent = vec![ContentPart::Text {
            text: "z".to_string(),
        }]
        .into();
        assert!(matches!(parts, MessageContent::Parts(_)));
    }
}
