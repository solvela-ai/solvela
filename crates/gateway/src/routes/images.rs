use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::AppState;

/// Image generation request (OpenAI-compatible subset).
#[derive(Debug, Deserialize)]
pub struct ImageGenerationRequest {
    pub prompt: String,
    pub model: Option<String>,
    pub n: Option<u8>,
    pub size: Option<String>,
    pub response_format: Option<String>,
}

/// POST /v1/images/generations — image generation via x402 payment.
///
/// This endpoint is scaffolded as an x402-paid route. It currently returns a
/// `501 Not Implemented` response until a real image provider adapter is added.
/// The 402 payment flow structure is complete — integrating a provider adapter
/// (e.g., OpenAI DALL·E, Stability AI) is the remaining step.
pub async fn image_generations(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<ImageGenerationRequest>,
) -> impl IntoResponse {
    let model = req.model.as_deref().unwrap_or("dall-e-3");

    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({
            "error": {
                "type": "not_implemented",
                "message": format!(
                    "Image generation is scaffolded but requires a provider adapter. \
                     Model requested: '{}'. Integrate an image provider adapter \
                     (e.g., openai/dall-e-3, stability-ai/stable-diffusion-xl) \
                     in crates/gateway/src/providers/ to enable this endpoint.",
                    model
                ),
                "docs": "https://docs.rustyclawrouter.com/images",
                "roadmap_phase": "Phase 3 extension",
            }
        })),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_image_route_registered() {
        // Presence test: verifies this module compiles and the handler exists.
        // Full HTTP-level tests are in the integration test suite.
        let _ = super::image_generations;
    }
}
