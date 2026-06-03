//! Multimodal content wire-format types.
//!
//! **Status: wired in.** [`MessageContent`] is now the type of
//! `ChatMessage::content`. It accepts both OpenAI shapes for message
//! content: a plain JSON string (`"hello"`) deserializes to
//! [`MessageContent::Text`], and an array of content parts
//! (`[{"type":"text","text":"hello"}]`) deserializes to
//! [`MessageContent::Parts`]. The enum is `#[serde(untagged)]`, so each
//! variant serializes back to its original wire shape — string content
//! round-trips as a JSON string, preserving backward compatibility for
//! existing string-only clients.
//!
//! Image parts are NOT silently dropped here. They are rejected upstream:
//! the chat route returns `415 Unsupported Media Type` for any request
//! carrying image content, and `reject_image_content` is a backstop at
//! provider dispatch. Because image content never reaches this layer in
//! practice, [`MessageContent::as_text`] simply ignores any image parts it
//! sees rather than treating that case as an error.
//!
//! For the string-based provider adapters (Anthropic / Google), text
//! parts are flattened to a single string via
//! [`MessageContent::as_text`]. For OpenAI-format providers (OpenAI / xAI
//! / DeepSeek), the request is serialized through directly, so array
//! content passes through natively to the upstream API.
//!
//! Native image-block translation for Anthropic / Google, model
//! capability gating in `models.toml`, and image cost accounting are not
//! yet implemented; image content is rejected at the boundary until they
//! are.

use serde::{Deserialize, Serialize};

/// An image URL with optional detail level.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ImageUrl {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub detail: Option<String>,
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
    /// string. Any image parts are ignored here because image content is
    /// rejected upstream (415 on the chat route + `reject_image_content`
    /// backstop) and so never reaches this method in practice; see module
    /// docs.
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
    /// image parts is considered empty here. That is acceptable because image
    /// content is rejected upstream (415 on the chat route; see the chat
    /// route validation), so `is_empty()` callers never observe an
    /// image-only message in practice. Computed without allocating.
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
    /// Used by the chat route to reject image/multimodal content (415):
    /// native image-block translation, capability gating, and image cost
    /// accounting are not yet implemented, so image content is rejected at
    /// the boundary until they are.
    pub fn has_image_parts(&self) -> bool {
        match self {
            MessageContent::Text(_) => false,
            MessageContent::Parts(parts) => parts
                .iter()
                .any(|p| matches!(p, ContentPart::ImageUrl { .. })),
        }
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
