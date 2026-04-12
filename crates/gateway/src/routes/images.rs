use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;

/// POST /v1/images/generations — image generation via x402 payment.
///
/// This endpoint is scaffolded as an x402-paid route. It currently returns a
/// `501 Not Implemented` response until a real image provider adapter is added.
///
/// Before removing the 501, add:
///   1. x402 payment enforcement (same as `routes/chat.rs`)
///   2. An image provider adapter in `crates/gateway/src/providers/`
///   3. Prompt guard middleware coverage
pub async fn image_generations() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({
            "error": {
                "type": "not_implemented",
                "message": "Image generation is scaffolded but requires a provider adapter. \
                            Integrate an image provider (e.g., openai/dall-e-3, \
                            stability-ai/stable-diffusion-xl) in \
                            crates/gateway/src/providers/ to enable this endpoint.",
                "docs": "https://docs.solvela.ai/images",
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
