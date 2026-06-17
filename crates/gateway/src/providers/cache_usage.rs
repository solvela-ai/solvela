//! Cross-provider prompt-cache token metering — OBSERVABILITY ONLY.
//!
//! This module captures per-provider prompt-cache token counts (read/write)
//! and exposes them as Prometheus counters so the discount the gateway already
//! captures via prompt caching becomes observable to operators.
//!
//! ## NOT a billing path
//!
//! Nothing here feeds cost computation, the 402 quote, settlement, `Usage`, or
//! what any agent is charged. The agent's `prompt_tokens` (and therefore its
//! bill) is reconstructed independently in each adapter's response conversion
//! (e.g. `from_anthropic_response` folds the cache fields back into
//! `prompt_tokens` so the charge is cache-invariant). [`CacheUsage`] is a
//! parallel, internal-only read of the same provider fields used purely to
//! drive counters. It is deliberately NOT `Serialize`/`Deserialize`: it never
//! crosses a wire and never reaches the public `solvela_protocol::Usage`.
//!
//! ## Emitted metrics
//!
//! All labelled by `provider` + `model` (model cardinality is bounded — the
//! registry carries ≤ 26 models, so `{provider,model}` is safe):
//!
//! - `solvela_provider_cache_read_tokens_total{provider,model}` — prompt tokens
//!   served from the provider's cache (the captured discount).
//! - `solvela_provider_cache_write_tokens_total{provider,model}` — prompt tokens
//!   written INTO the provider's cache (the one-time write premium). Only
//!   Anthropic reports a distinct cache-write count today; for providers that
//!   do not, this stays 0.
//! - `solvela_provider_requests_total{provider,model}` — a DENOMINATOR. Emitted
//!   once per metered response so that a flat cache-read counter is
//!   unambiguous: if `requests_total` keeps climbing while
//!   `cache_read_tokens_total` goes to zero, an operator can distinguish
//!   "cacheable traffic continues but cache reads dropped to zero" (a possible
//!   silent field rename — see below) from "no traffic at all". Without a
//!   denominator a bare cache-token counter flatlining is ambiguous (could be
//!   legitimate cache misses).
//!
//! ## Why the denominator matters (PR-1 review follow-up)
//!
//! The provider cache fields are parsed with `#[serde(default)]`, so a future
//! provider-side rename of a cache field would silently read 0. For Anthropic
//! that would also under-bill (the billing path folds those fields into
//! `prompt_tokens`); this metering is the mitigation, and pairing the read
//! counter with `requests_total` makes the flatline detectable on a dashboard
//! rather than invisible.

use metrics::counter;

/// Internal per-response prompt-cache token counts.
///
/// `cache_read_tokens` — prompt tokens served from the provider's cache.
/// `cache_write_tokens` — prompt tokens written into the provider's cache
/// (only Anthropic reports this distinctly today; 0 elsewhere).
///
/// Internal-only: no `Serialize`/`Deserialize`. It must never reach the frozen
/// public `solvela_protocol::Usage` / `ChatResponse` wire types.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CacheUsage {
    pub(crate) cache_read_tokens: u32,
    pub(crate) cache_write_tokens: u32,
}

impl CacheUsage {
    /// Emit the cache-token counters plus the request denominator for one
    /// metered response, labelled by `provider` + `model`.
    ///
    /// Always emits `solvela_provider_requests_total` (the denominator), even
    /// when both token counts are 0 — that is the whole point: a zero
    /// cache-read against a climbing request count is the signal an operator
    /// needs. Pure observability; touches no money path.
    pub(crate) fn emit(&self, provider: &str, model: &str) {
        // Denominator first: one increment per metered response regardless of
        // whether any cache tokens were reported.
        counter!(
            "solvela_provider_requests_total",
            "provider" => provider.to_string(),
            "model" => model.to_string(),
        )
        .increment(1);

        counter!(
            "solvela_provider_cache_read_tokens_total",
            "provider" => provider.to_string(),
            "model" => model.to_string(),
        )
        .increment(u64::from(self.cache_read_tokens));

        counter!(
            "solvela_provider_cache_write_tokens_total",
            "provider" => provider.to_string(),
            "model" => model.to_string(),
        )
        .increment(u64::from(self.cache_write_tokens));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reuse the single process-wide test recorder owned by the `cache` module.
    // The `metrics` crate allows exactly one global recorder install; that
    // module's `OnceLock` is the crate's single source of truth, so importing
    // it here (rather than installing a second recorder) avoids the
    // `FailedToSetGlobalRecorder` panic when both test trees run in one binary.
    use crate::cache::test_metrics::{counter_value_filtered, install_test_recorder};

    #[test]
    fn default_is_zero_zero() {
        let cu = CacheUsage::default();
        assert_eq!(cu.cache_read_tokens, 0);
        assert_eq!(cu.cache_write_tokens, 0);
    }

    /// `emit` increments the read, write, and denominator counters by the
    /// parsed amounts for the labelled `{provider,model}` family.
    ///
    /// The single process-wide recorder is shared across all tests, so we
    /// isolate this test with a UNIQUE `model` label and read only that series
    /// via `counter_value_filtered` — making the before/after deltas immune to
    /// sibling tests incrementing the same families in parallel. Falsifiable:
    /// the deltas must equal the struct's fields (read/write) and 1
    /// (denominator).
    #[test]
    fn emit_increments_counters_by_parsed_amounts() {
        let handle = install_test_recorder();

        let provider = "test-emit-provider";
        let model = "test-emit-model-unique-a";
        let key = format!("model=\"{model}\"");

        let read_before =
            counter_value_filtered(&handle, "solvela_provider_cache_read_tokens_total", &key);
        let write_before =
            counter_value_filtered(&handle, "solvela_provider_cache_write_tokens_total", &key);
        let req_before = counter_value_filtered(&handle, "solvela_provider_requests_total", &key);

        CacheUsage {
            cache_read_tokens: 1800,
            cache_write_tokens: 500,
        }
        .emit(provider, model);

        let read_after =
            counter_value_filtered(&handle, "solvela_provider_cache_read_tokens_total", &key);
        let write_after =
            counter_value_filtered(&handle, "solvela_provider_cache_write_tokens_total", &key);
        let req_after = counter_value_filtered(&handle, "solvela_provider_requests_total", &key);

        assert_eq!(
            read_after - read_before,
            1800,
            "read counter must increment by cache_read_tokens"
        );
        assert_eq!(
            write_after - write_before,
            500,
            "write counter must increment by cache_write_tokens"
        );
        assert_eq!(
            req_after - req_before,
            1,
            "request denominator must increment exactly once per emit"
        );
    }

    /// A zero-token `emit` still increments the denominator — the unambiguous
    /// flatline signal. Falsifiable: if `emit` skipped the denominator when
    /// tokens were 0, `req_after - req_before` would be 0.
    #[test]
    fn emit_with_zero_tokens_still_increments_denominator() {
        let handle = install_test_recorder();
        let key = "model=\"test-zero-model-unique-b\"";
        let req_before = counter_value_filtered(&handle, "solvela_provider_requests_total", key);

        CacheUsage::default().emit("test-zero-provider", "test-zero-model-unique-b");

        let req_after = counter_value_filtered(&handle, "solvela_provider_requests_total", key);
        assert_eq!(
            req_after - req_before,
            1,
            "denominator must increment even when no cache tokens were reported"
        );
    }
}
