use serde::{Deserialize, Serialize};

/// An image URL with optional detail level.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageUrl {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub detail: Option<String>,
}

/// A single part of multi-modal content (text or image).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
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
}
