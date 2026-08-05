//! Env-gated, ephemeral debug logging of provider request/response bodies.
//!
//! A dev/ops observability aid, NOT a product feature: nothing here is
//! persisted (no Postgres, no Redis, no response header) and nothing here can
//! change what the client receives or what the agent is billed. Every emitted
//! line is a `tracing::debug!` that vanishes with the log buffer.
//!
//! # Gate
//!
//! OFF unless `SOLVELA_DEBUG_BODIES` is set to a truthy value (`1`/`true`/`yes`
//! /`on`, case-insensitive). The env var is read exactly ONCE per process into
//! a [`OnceLock`], so a running gateway cannot be flipped on by a request — in
//! particular this is deliberately NOT wired to the client-controlled
//! `X-Solvela-Debug` header (`routes::debug_headers::is_debug_enabled`), which
//! any caller can set.
//!
//! # What is safe to log here
//!
//! BODIES ONLY. Headers are never passed in and must never be: the provider API
//! key (`x-api-key` / `Authorization`), the `PAYMENT-SIGNATURE` credential, and
//! the fee-payer key all live outside the body. As defence-in-depth against a
//! credential pasted INTO a body (e.g. a user prompt containing their own API
//! key), every snippet is run through [`redact_secrets`] before it reaches the
//! log line, and truncated to [`MAX_LOGGED_BODY_BYTES`].
//!
//! Known gap: an UNPREFIXED high-entropy secret (a raw base58 Solana secret
//! key, a bare password) is not detectable by the prefix scan and would be
//! logged verbatim if a caller pasted one into a prompt. The gate being
//! operator-only and off by default is the control for that.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

use serde::Serialize;
use tracing::{debug, warn};

/// Env var that opens the gate. Read once per process.
const ENV_VAR: &str = "SOLVELA_DEBUG_BODIES";

/// Hard cap on the bytes of any single body emitted to a log line, so one large
/// prompt (the body limit is 10MB) cannot blow up the log.
const MAX_LOGGED_BODY_BYTES: usize = 4096;

/// Placeholder substituted for anything that looks like a credential.
const REDACTED: &str = "[REDACTED]";

/// Shortest token length considered for redaction — below this a prefix match
/// is noise (`sk-1`), not a key.
const MIN_SECRET_LEN: usize = 12;

/// Credential prefixes across the providers this gateway talks to, plus the
/// common developer-token shapes most likely to be pasted into a prompt.
const SECRET_PREFIXES: &[&str] = &[
    "sk-",         // OpenAI, Anthropic (sk-ant-), DeepSeek
    "xai-",        // xAI
    "nvapi-",      // NVIDIA
    "AIza",        // Google
    "gsk_",        // Groq
    "pplx-",       // Perplexity
    "tvly-",       // Tavily
    "ghp_",        // GitHub PAT (classic)
    "github_pat_", // GitHub PAT (fine-grained)
    "glpat-",      // GitLab PAT
    "xoxb-",       // Slack bot token
    "AKIA",        // AWS access key id
    "hf_",         // Hugging Face
];

/// How many times the gate initializer has actually run. The operator warning
/// below lives inside that initializer, so this doubles as the emitted-warning
/// count — it exists so `test_gate_resolves_at_most_once` can prove the warning
/// cannot fire per request without installing a global tracing subscriber
/// (which would race the rest of the test binary).
static GATE_RESOLUTIONS: AtomicUsize = AtomicUsize::new(0);

/// Is body logging on? Decided once, from the environment, at first call.
///
/// When the gate resolves OPEN this emits a single `warn!` — bodies in the logs
/// are a deliberate, operator-only diagnostic and an operator who forgets the
/// flag is set has prompts and completions landing in their log sink. The
/// warning is inside the `OnceLock` initializer, so it fires exactly once per
/// process no matter how many requests flow through.
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        GATE_RESOLUTIONS.fetch_add(1, Ordering::Relaxed);
        let on = env_enables(std::env::var(ENV_VAR).ok().as_deref());
        if on {
            warn!(
                env_var = ENV_VAR,
                "DEBUG BODY LOGGING IS ON: provider request and response bodies \
                 (prompts and completions) are being written to the logs. This is a \
                 dev/ops diagnostic — unset {ENV_VAR} in production.",
            );
        }
        on
    })
}

/// Pure gate decision, split out so the closed-by-default behaviour is testable
/// without mutating process env (which [`enabled`]'s `OnceLock` would cache).
///
/// Anything other than an explicit truthy value — including an unset var, an
/// empty string, and `false`/`0` — keeps the gate CLOSED.
fn env_enables(value: Option<&str>) -> bool {
    value.map(str::trim).is_some_and(|v| {
        ["1", "true", "yes", "on"]
            .iter()
            .any(|t| v.eq_ignore_ascii_case(t))
    })
}

/// Log a body that is already bytes (the Anthropic native relay's verbatim
/// request/response). Reads the slice only — never mutates or re-serializes it,
/// so the byte-exactness of the relay (CLAUDE.md rule #4) is preserved.
///
/// `direction` is `"request"` or `"response"`; `stage` names the choke point.
pub fn log_bytes(direction: &str, stage: &str, model: &str, body: &[u8]) {
    if !enabled() {
        return;
    }
    let (snippet, truncated) = snippet(body);
    debug!(
        direction,
        stage,
        model,
        bytes = body.len(),
        truncated,
        body = %snippet,
        "debug body"
    );
}

/// Log a serializable body (the OpenAI-shaped `ChatRequest`/`ChatResponse` at
/// the reshape choke point). Serialization happens ONLY after the gate check,
/// so a closed gate costs one atomic load.
pub fn log_json(direction: &str, stage: &str, model: &str, value: &impl Serialize) {
    if !enabled() {
        return;
    }
    match serde_json::to_vec(value) {
        Ok(bytes) => log_bytes(direction, stage, model, &bytes),
        Err(e) => debug!(direction, stage, model, error = %e, "debug body not serializable"),
    }
}

/// Log that a body exists but was deliberately NOT captured (streaming). Keeps
/// the operator from reading an absent response line as "nothing came back".
pub fn log_omitted(direction: &str, stage: &str, model: &str, reason: &str) {
    if !enabled() {
        return;
    }
    debug!(direction, stage, model, reason, "debug body omitted");
}

/// Truncate to [`MAX_LOGGED_BODY_BYTES`] and redact credential-shaped tokens.
///
/// Returns `(snippet, truncated)`. Invalid UTF-8 (a byte-split multibyte char at
/// the cap, or a non-UTF-8 body) becomes replacement characters rather than an
/// error — this is a diagnostic, it must never panic or fail a request.
fn snippet(body: &[u8]) -> (String, bool) {
    let truncated = body.len() > MAX_LOGGED_BODY_BYTES;
    let head = &body[..body.len().min(MAX_LOGGED_BODY_BYTES)];
    (redact_secrets(&String::from_utf8_lossy(head)), truncated)
}

/// Replace credential-shaped tokens with [`REDACTED`].
///
/// Two rules, both conservative: a token starting with a known provider/vendor
/// key prefix, and the token following a `Bearer` (any bearer credential,
/// prefix or not). Everything else is passed through — over-redaction would
/// make the tool useless for reading prompts, which is its whole purpose.
fn redact_secrets(input: &str) -> String {
    fn is_token_char(c: char) -> bool {
        c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')
    }

    let mut out = String::with_capacity(input.len());
    let mut after_bearer = false;
    let mut rest = input;

    while !rest.is_empty() {
        // Copy the non-token run verbatim.
        let token_start = rest.find(is_token_char).unwrap_or(rest.len());
        out.push_str(&rest[..token_start]);
        rest = &rest[token_start..];
        if rest.is_empty() {
            break;
        }

        let token_end = rest.find(|c| !is_token_char(c)).unwrap_or(rest.len());
        let token = &rest[..token_end];
        rest = &rest[token_end..];

        let looks_like_key =
            token.len() >= MIN_SECRET_LEN && SECRET_PREFIXES.iter().any(|p| token.starts_with(p));
        if after_bearer || looks_like_key {
            out.push_str(REDACTED);
        } else {
            out.push_str(token);
        }
        after_bearer = token.eq_ignore_ascii_case("bearer");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── The gate is closed by default ────────────────────────────────────────

    #[test]
    fn test_gate_closed_when_env_unset() {
        assert!(!env_enables(None), "unset SOLVELA_DEBUG_BODIES must be OFF");
    }

    #[test]
    fn test_gate_closed_for_empty_and_falsey_values() {
        for v in ["", "  ", "0", "false", "FALSE", "no", "off", "maybe"] {
            assert!(!env_enables(Some(v)), "{v:?} must not open the gate");
        }
    }

    #[test]
    fn test_gate_opens_only_for_explicit_truthy_values() {
        for v in ["1", "true", "TRUE", "True", "yes", "on", " true "] {
            assert!(env_enables(Some(v)), "{v:?} should open the gate");
        }
    }

    /// The REAL wiring, not just the pure helper: `cargo test` runs with
    /// `SOLVELA_DEBUG_BODIES` unset, so the live gate must report closed.
    #[test]
    fn test_live_gate_is_closed_in_a_default_environment() {
        assert!(
            std::env::var(ENV_VAR).is_err(),
            "test env must not preset {ENV_VAR}"
        );
        assert!(!enabled(), "body logging must be OFF by default");
    }

    /// The operator warning lives inside the gate initializer, so it must be
    /// impossible for it to fire per request. Hammering `enabled()` must not
    /// re-run that initializer even once more.
    #[test]
    fn test_gate_resolves_at_most_once() {
        for _ in 0..1_000 {
            let _ = enabled();
        }
        assert!(
            GATE_RESOLUTIONS.load(Ordering::Relaxed) <= 1,
            "gate initializer ran {} times — the operator warning would fire per request",
            GATE_RESOLUTIONS.load(Ordering::Relaxed)
        );
    }

    // ── Secrets never reach the log line ─────────────────────────────────────

    #[test]
    fn test_snippet_redacts_provider_api_keys() {
        let secret = "sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF";
        let body = format!(r#"{{"messages":[{{"content":"my key is {secret} ok"}}]}}"#);
        let (out, _) = snippet(body.as_bytes());
        assert!(!out.contains(secret), "raw key leaked into log line: {out}");
        assert!(out.contains(REDACTED));
        // Non-secret content survives — the tool has to stay useful.
        assert!(out.contains("my key is"));
    }

    #[test]
    fn test_snippet_redacts_bearer_credentials() {
        let (out, _) = snippet(b"Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig");
        assert!(!out.contains("eyJhbGciOiJIUzI1NiJ9"));
        assert!(out.contains(REDACTED));
    }

    #[test]
    fn test_snippet_redacts_every_known_vendor_prefix() {
        for prefix in SECRET_PREFIXES {
            let secret = format!("{prefix}0123456789abcdefghij");
            let (out, _) = snippet(secret.as_bytes());
            assert!(!out.contains(&secret), "{prefix} not redacted: {out}");
        }
    }

    #[test]
    fn test_snippet_truncates_to_the_cap() {
        let body = vec![b'a'; MAX_LOGGED_BODY_BYTES * 3];
        let (out, truncated) = snippet(&body);
        assert!(truncated);
        assert_eq!(out.len(), MAX_LOGGED_BODY_BYTES);
    }

    #[test]
    fn test_snippet_does_not_flag_short_or_ordinary_text() {
        let (out, truncated) = snippet(b"explain sk-1 and the ok-token please");
        assert_eq!(out, "explain sk-1 and the ok-token please");
        assert!(!truncated);
    }

    #[test]
    fn test_snippet_handles_non_utf8_without_panicking() {
        let (_out, truncated) = snippet(&[0xff, 0xfe, 0x00, 0x41]);
        assert!(!truncated);
    }
}
