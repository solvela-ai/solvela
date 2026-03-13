//! Debug response headers for routing diagnostics.
//!
//! When a request includes `X-RCR-Debug: true`, the chat handler attaches
//! detailed routing diagnostics to the response. These headers are **never**
//! returned without the explicit debug flag to avoid leaking internal state.

use axum::http::{HeaderMap, HeaderName, HeaderValue, Response};

/// Header name clients send to opt into debug headers.
pub static DEBUG_FLAG_HEADER: HeaderName = HeaderName::from_static("x-rcr-debug");

// Debug response header names
static H_MODEL: HeaderName = HeaderName::from_static("x-rcr-model");
static H_TIER: HeaderName = HeaderName::from_static("x-rcr-tier");
static H_SCORE: HeaderName = HeaderName::from_static("x-rcr-score");
static H_PROFILE: HeaderName = HeaderName::from_static("x-rcr-profile");
static H_PROVIDER: HeaderName = HeaderName::from_static("x-rcr-provider");
static H_CACHE: HeaderName = HeaderName::from_static("x-rcr-cache");
static H_LATENCY: HeaderName = HeaderName::from_static("x-rcr-latency-ms");
static H_PAYMENT: HeaderName = HeaderName::from_static("x-rcr-payment-status");
static H_TOKEN_IN: HeaderName = HeaderName::from_static("x-rcr-token-estimate-in");
static H_TOKEN_OUT: HeaderName = HeaderName::from_static("x-rcr-token-estimate-out");

/// Routing diagnostic data collected during request processing.
#[derive(Debug, Clone)]
pub struct DebugInfo {
    pub model: String,
    pub tier: String,
    pub score: f64,
    pub profile: String,
    pub provider: String,
    pub cache_status: CacheStatus,
    pub latency_ms: u64,
    pub payment_status: PaymentStatus,
    pub token_estimate_in: u32,
    pub token_estimate_out: u32,
}

/// Cache lookup result for the debug header.
#[derive(Debug, Clone, Copy)]
pub enum CacheStatus {
    Hit,
    Miss,
    /// Streaming requests skip cache entirely.
    Skip,
}

impl CacheStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Miss => "miss",
            Self::Skip => "skip",
        }
    }
}

/// Payment verification result for the debug header.
#[derive(Debug, Clone, Copy)]
pub enum PaymentStatus {
    /// Payment signature verified and settled.
    Verified,
    /// Free-tier model, no payment needed.
    Free,
    /// No payment header — 402 will be returned.
    None,
    /// Dev-mode bypass — payment skipped for development/testing.
    DevBypass,
}

impl PaymentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Free => "free",
            Self::None => "none",
            Self::DevBypass => "dev_bypass",
        }
    }
}

/// Check if the request has the debug flag set.
pub fn is_debug_enabled(headers: &HeaderMap) -> bool {
    headers
        .get(&DEBUG_FLAG_HEADER)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

/// Attach debug diagnostic headers to a response.
///
/// Only call this when [`is_debug_enabled`] returns `true`.
pub fn attach_debug_headers<B>(response: &mut Response<B>, info: &DebugInfo) {
    let headers = response.headers_mut();

    let pairs: [(&HeaderName, String); 10] = [
        (&H_MODEL, info.model.clone()),
        (&H_TIER, info.tier.clone()),
        (&H_SCORE, format!("{:.4}", info.score)),
        (&H_PROFILE, info.profile.clone()),
        (&H_PROVIDER, info.provider.clone()),
        (&H_CACHE, info.cache_status.as_str().to_string()),
        (&H_LATENCY, info.latency_ms.to_string()),
        (&H_PAYMENT, info.payment_status.as_str().to_string()),
        (&H_TOKEN_IN, info.token_estimate_in.to_string()),
        (&H_TOKEN_OUT, info.token_estimate_out.to_string()),
    ];

    for (name, value) in &pairs {
        if let Ok(hv) = HeaderValue::from_str(value) {
            headers.insert((*name).clone(), hv);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Response;

    fn sample_debug_info() -> DebugInfo {
        DebugInfo {
            model: "anthropic/claude-sonnet-4-20250514".to_string(),
            tier: "Complex".to_string(),
            score: 0.4237,
            profile: "auto".to_string(),
            provider: "anthropic".to_string(),
            cache_status: CacheStatus::Miss,
            latency_ms: 1847,
            payment_status: PaymentStatus::Verified,
            token_estimate_in: 1200,
            token_estimate_out: 500,
        }
    }

    #[test]
    fn test_attach_debug_headers_sets_all_ten() {
        let mut resp = Response::new(());
        attach_debug_headers(&mut resp, &sample_debug_info());

        assert_eq!(
            resp.headers().get("x-rcr-model").unwrap(),
            "anthropic/claude-sonnet-4-20250514"
        );
        assert_eq!(resp.headers().get("x-rcr-tier").unwrap(), "Complex");
        assert_eq!(resp.headers().get("x-rcr-score").unwrap(), "0.4237");
        assert_eq!(resp.headers().get("x-rcr-profile").unwrap(), "auto");
        assert_eq!(resp.headers().get("x-rcr-provider").unwrap(), "anthropic");
        assert_eq!(resp.headers().get("x-rcr-cache").unwrap(), "miss");
        assert_eq!(resp.headers().get("x-rcr-latency-ms").unwrap(), "1847");
        assert_eq!(
            resp.headers().get("x-rcr-payment-status").unwrap(),
            "verified"
        );
        assert_eq!(
            resp.headers().get("x-rcr-token-estimate-in").unwrap(),
            "1200"
        );
        assert_eq!(
            resp.headers().get("x-rcr-token-estimate-out").unwrap(),
            "500"
        );
    }

    #[test]
    fn test_is_debug_enabled_true() {
        let mut headers = HeaderMap::new();
        headers.insert("x-rcr-debug", HeaderValue::from_static("true"));
        assert!(is_debug_enabled(&headers));
    }

    #[test]
    fn test_is_debug_enabled_case_insensitive() {
        let mut headers = HeaderMap::new();
        headers.insert("x-rcr-debug", HeaderValue::from_static("True"));
        assert!(is_debug_enabled(&headers));

        headers.insert("x-rcr-debug", HeaderValue::from_static("TRUE"));
        assert!(is_debug_enabled(&headers));
    }

    #[test]
    fn test_is_debug_enabled_false() {
        let mut headers = HeaderMap::new();
        headers.insert("x-rcr-debug", HeaderValue::from_static("false"));
        assert!(!is_debug_enabled(&headers));
    }

    #[test]
    fn test_is_debug_enabled_absent() {
        let headers = HeaderMap::new();
        assert!(!is_debug_enabled(&headers));
    }

    #[test]
    fn test_cache_status_as_str() {
        assert_eq!(CacheStatus::Hit.as_str(), "hit");
        assert_eq!(CacheStatus::Miss.as_str(), "miss");
        assert_eq!(CacheStatus::Skip.as_str(), "skip");
    }

    #[test]
    fn test_payment_status_as_str() {
        assert_eq!(PaymentStatus::Verified.as_str(), "verified");
        assert_eq!(PaymentStatus::Free.as_str(), "free");
        assert_eq!(PaymentStatus::None.as_str(), "none");
    }
}
