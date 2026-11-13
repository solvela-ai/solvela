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
//! any caller can set. `main` resolves the gate once at startup via
//! [`resolve_at_startup`], so the operator warning (below) lands in the
//! startup log before any traffic — not on the first provider request.
//!
//! # What is safe to log here
//!
//! BODIES ONLY. Headers are never passed in and must never be: the provider API
//! key (`x-api-key` / `Authorization`), the `PAYMENT-SIGNATURE` credential, and
//! the fee-payer key all live outside the body. As defence-in-depth against a
//! credential pasted INTO a body (e.g. a user prompt containing their own API
//! key), every snippet is run through [`redact_secrets`] before it reaches the
//! log line, and truncated to [`MAX_LOGGED_BODY_BYTES`]. JSON bodies are
//! parsed and redacted as DECODED string values — the FULL [`serde_json::Value`]
//! is walked and each string (keys included) redacted before re-serialization,
//! never just the first 4 KiB — so a JSON escape cannot hide a credential, even
//! an escape inside the token itself (`sk\u002dant-…`, `Be\u0061rer …`). The
//! scan has NO escape interpretation and NO token-boundary requirement: a
//! vendor prefix or a `bearer` scheme matches anywhere in the text (glued
//! prefixes like `xnvapi-…` and literal backslashes like `\nvapi-…` are
//! caught), and non-JSON text has its `\uXXXX` escapes decoded
//! ([`decode_unicode_escapes`]) before the scan. The byte cap is enforced on
//! the sanitized OUTPUT, so it holds even when redaction markers expand the
//! text; and a body over [`MAX_DEBUG_PARSE_BYTES`] is never logged at all —
//! the log gets an omission placeholder with the byte count instead of
//! content.
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

/// Bodies larger than this are never parsed for redaction: the log gets an
/// omission placeholder instead of content. 1 MiB bounds the redaction work —
/// parse, walk, and re-serialize all scale with the body — while staying far
/// above the [`MAX_LOGGED_BODY_BYTES`] log-line cap.
const MAX_DEBUG_PARSE_BYTES: usize = 1024 * 1024;

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
/// `main` calls [`resolve_at_startup`] right after tracing is initialized, so
/// the gate resolves during STARTUP rather than lazily on the first provider
/// request.
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

/// Resolve the gate at gateway startup.
///
/// [`enabled`] is deliberately a single atomic load on the request hot path,
/// which is why the gate is a [`OnceLock`] — but that also means the gate (and
/// the operator warning it emits when open) would otherwise first fire on the
/// FIRST provider request. `main` calls this right after tracing is
/// initialized, so the warning lands in the startup log before any traffic.
/// Idempotent with every other [`enabled`] call: the gate resolves at most
/// once per process.
pub fn resolve_at_startup() {
    let _ = enabled();
}

/// Pure gate resolution for a raw environment value: the single decision
/// function [`enabled`] and [`resolve_at_startup`] both route through. Split
/// out so the closed-by-default behaviour is testable directly, without
/// mutating process env (which [`enabled`]'s process-global `OnceLock` would
/// cache) and without the `OnceLock` proving or disproving anything in a
/// parallel test binary.
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
/// request/response). The relay bytes themselves are never mutated — the
/// byte-exactness of the relay (CLAUDE.md rule #4) is preserved. For the LOG
/// LINE only, the body is parsed and re-serialized from a redacted copy, so
/// redaction happens on decoded string values and cannot affect the bytes
/// forwarded upstream.
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

/// Redact credential-shaped tokens, truncate, and return `(snippet,
/// truncated)`. Invalid UTF-8 (a non-UTF-8 body, or a byte-split multibyte
/// char) becomes replacement characters rather than an error — this is a
/// diagnostic, it must never panic or fail a request.
///
/// The FULL body is redacted, never just the first
/// [`MAX_LOGGED_BODY_BYTES`]: a body whose 4 KiB head is an incomplete JSON
/// document is still parsed as a whole, its decoded string values redacted,
/// re-serialized, and only THEN cut to the cap. Redacting a raw truncated
/// head instead would copy JSON escapes (`\uXXXX`) through verbatim and leak
/// any credential they encode (`sk\u002dant-…`).
///
/// Bodies over [`MAX_DEBUG_PARSE_BYTES`] are not parsed at all: the log gets
/// an omission placeholder, never content. The cap is also enforced on the
/// SANITIZED OUTPUT, on a UTF-8 char boundary: redaction markers can expand
/// the text (a repeated `Bearer x`), and the lossy decode can add
/// replacement-char bytes, so cutting the raw input first is not enough to
/// keep the log line within [`MAX_LOGGED_BODY_BYTES`].
fn snippet(body: &[u8]) -> (String, bool) {
    if body.len() > MAX_DEBUG_PARSE_BYTES {
        return (
            format!(
                "[body omitted: {} bytes exceeds debug parse cap]",
                body.len()
            ),
            true,
        );
    }

    let input_truncated = body.len() > MAX_LOGGED_BODY_BYTES;

    // Parse the WHOLE body: if it is JSON, redact the DECODED string values
    // (keys included) before re-serializing, so a JSON escape — however
    // placed, even inside the token prefix itself — cannot merge into or hide
    // a credential. Only non-JSON bytes take the raw-text scan
    // ([`redact_raw`]), which decodes `\uXXXX` escapes into a scan copy
    // first.
    let text = match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(mut value) => {
            redact_json_value(&mut value);
            match serde_json::to_string(&value) {
                Ok(json) => json,
                Err(_) => redact_raw(&String::from_utf8_lossy(body)),
            }
        }
        Err(_) => redact_raw(&String::from_utf8_lossy(body)),
    };

    let (out, output_truncated) = truncate_to_cap(text);
    (out, input_truncated || output_truncated)
}

/// Truncate `text` to at most [`MAX_LOGGED_BODY_BYTES`] bytes on a UTF-8 char
/// boundary. Returns `(text, cut)` — `cut` is true only when bytes were
/// actually dropped.
fn truncate_to_cap(mut text: String) -> (String, bool) {
    if text.len() <= MAX_LOGGED_BODY_BYTES {
        return (text, false);
    }
    let mut end = MAX_LOGGED_BODY_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    (text, true)
}

/// Redact every string (values AND keys) of a parsed JSON document, in place,
/// before it is re-serialized for the log line. Walking the decoded
/// [`serde_json::Value`] means JSON escapes (`\n`, `\t`, `\"`, `\\`, `\uXXXX`)
/// are already resolved, so an escape cannot merge into — and thereby hide —
/// a credential the way scanning the raw serialized text would.
fn redact_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(s) => {
            let redacted = redact_secrets(s);
            *s = redacted;
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_json_value(item);
            }
        }
        serde_json::Value::Object(map) => {
            let mut redacted_map = serde_json::Map::with_capacity(map.len());
            for (key, mut value) in std::mem::take(map) {
                redact_json_value(&mut value);
                redacted_map.insert(redact_secrets(&key), value);
            }
            *map = redacted_map;
        }
        _ => {}
    }
}

/// Replace credential-shaped tokens with [`REDACTED`].
///
/// Two rules, both conservative and boundary-free: every occurrence of a known
/// provider/vendor key prefix is found ANYWHERE in the text — glued to a
/// preceding letter (`xnvapi-…`) or preceded by a literal backslash
/// (`\nvapi-…`) alike — and the credential run starting at the prefix (ASCII
/// alphanumerics plus `-`, `_`, `.`, at least [`MIN_SECRET_LEN`] bytes) is
/// replaced whole; and a case-insensitive `bearer` anywhere, followed by
/// whitespace (a single `:` tolerated), replaces the whole following
/// credential up to whitespace or a quote, so base64/base64url punctuation
/// like `/`, `+`, `~`, `=` and `.` inside it cannot split the credential and
/// leave an unredacted tail. Everything else is passed through.
///
/// There is NO escape interpretation here, and no token-boundary
/// requirement: decoded JSON strings are plain text, and a literal backslash
/// must never swallow the first letter of a vendor prefix or scheme word
/// (`\nvapi-…` must still match `nvapi-`, `\bearer …` must still match
/// `bearer`). Decoding `\uXXXX` escapes in NON-JSON text is
/// [`decode_unicode_escapes`]'s job, applied before this scanner runs.
/// Over-redaction is acceptable; under-redaction is not.
fn redact_secrets(input: &str) -> String {
    let bytes = input.as_bytes();

    /// Length of the credential run starting at byte `i`: ASCII alphanumerics
    /// plus `-`, `_`, `.`. The run is pure ASCII, so byte length equals char
    /// length and every position inside it is a char boundary.
    fn run_len(bytes: &[u8], i: usize) -> usize {
        let mut n = 0;
        while let Some(&b) = bytes.get(i + n) {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.') {
                n += 1;
            } else {
                break;
            }
        }
        n
    }

    /// Length of the credential run after a bearer scheme: every char up to
    /// whitespace or a double quote. No backslash handling — this text is
    /// already decoded, so a backslash is a literal character.
    fn credential_len(s: &str) -> usize {
        let mut n = 0;
        for c in s.chars() {
            if c == '"' || c.is_whitespace() {
                break;
            }
            n += c.len_utf8();
        }
        n
    }

    let mut out = String::with_capacity(input.len());
    let mut i = 0;

    'scan: while i < input.len() {
        // Find the earliest match at or after `i`. Matching is bytewise, and
        // every match starts with an ASCII byte — continuation bytes of
        // multi-byte chars are all >= 0x80 — so a match position is always a
        // char boundary and `input[i..p]` slices safely.
        let mut p = i;
        loop {
            let rest = &bytes[p..];

            if SECRET_PREFIXES
                .iter()
                .any(|prefix| rest.starts_with(prefix.as_bytes()))
            {
                out.push_str(&input[i..p]);
                let end = p + run_len(bytes, p);
                if end - p >= MIN_SECRET_LEN {
                    out.push_str(REDACTED);
                    i = end;
                } else {
                    // Too short to be a key (`sk-1`): keep the first byte and
                    // rescan; the rest of the word is copied verbatim.
                    out.push_str(&input[p..p + 1]);
                    i = p + 1;
                }
                continue 'scan;
            }

            if rest
                .get(..6)
                .is_some_and(|b| b.eq_ignore_ascii_case(b"bearer"))
            {
                // The scheme word must actually introduce a credential:
                // whitespace (or ':' — "Bearer: cred" shapes) right after it.
                let follows = input[p + 6..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_whitespace() || c == ':');
                if follows {
                    out.push_str(&input[i..p]);
                    let mut k = p + 6;
                    let ws = input[k..].len() - input[k..].trim_start().len();
                    k += ws;
                    if input[k..].starts_with(':') {
                        k += 1;
                        let ws = input[k..].len() - input[k..].trim_start().len();
                        k += ws;
                    }
                    let cred = &input[k..];
                    let cend = credential_len(cred);
                    if cend > 0 {
                        out.push_str(&input[p..k]);
                        out.push_str(REDACTED);
                        i = k + cend;
                    } else {
                        out.push_str(&input[p..p + 1]);
                        i = p + 1;
                    }
                    continue 'scan;
                }
            }

            p += 1;
            if p >= input.len() {
                out.push_str(&input[i..]);
                break 'scan;
            }
        }
    }

    out
}

/// Decode `\uXXXX` unicode escapes (valid hex only) in NON-JSON text into a
/// scan copy, so an escape-encoded credential in plain text (`\u0073k-ant-…`,
/// `\u0042earer …`) is redacted in its decoded form and the decoded copy is
/// what gets logged. Surrogate pairs are combined; lone surrogates and
/// malformed escapes are left verbatim. Byte-indexed and panic-free: hex
/// digits are read as BYTES, never as a `&str` slice, so a multi-byte char in
/// the digit positions (`\uabc😀`) can never break a char boundary.
fn decode_unicode_escapes(input: &str) -> String {
    fn hex(b: u8) -> Option<u32> {
        match b {
            b'0'..=b'9' => Some(u32::from(b - b'0')),
            b'a'..=b'f' => Some(u32::from(b - b'a') + 10),
            b'A'..=b'F' => Some(u32::from(b - b'A') + 10),
            _ => None,
        }
    }

    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'\\' || bytes.get(i + 1) != Some(&b'u') {
            let Some(c) = input[i..].chars().next() else {
                break;
            };
            out.push(c);
            i += c.len_utf8();
            continue;
        }

        let mut code = None;
        if let (Some(&a), Some(&b), Some(&c), Some(&d)) = (
            bytes.get(i + 2),
            bytes.get(i + 3),
            bytes.get(i + 4),
            bytes.get(i + 5),
        ) {
            if let (Some(h0), Some(h1), Some(h2), Some(h3)) = (hex(a), hex(b), hex(c), hex(d)) {
                code = Some((h0 << 12) | (h1 << 8) | (h2 << 4) | h3);
            }
        }
        let Some(code) = code else {
            // Not a valid \uXXXX escape — the backslash is literal.
            out.push('\\');
            i += 1;
            continue;
        };

        // High surrogate: pair it with a following \uXXXX low surrogate.
        if (0xD800..=0xDBFF).contains(&code) {
            let mut paired = None;
            if bytes.get(i + 6) == Some(&b'\\') && bytes.get(i + 7) == Some(&b'u') {
                if let (Some(&a), Some(&b), Some(&c), Some(&d)) = (
                    bytes.get(i + 8),
                    bytes.get(i + 9),
                    bytes.get(i + 10),
                    bytes.get(i + 11),
                ) {
                    if let (Some(l0), Some(l1), Some(l2), Some(l3)) =
                        (hex(a), hex(b), hex(c), hex(d))
                    {
                        let low = (l0 << 12) | (l1 << 8) | (l2 << 4) | l3;
                        if (0xDC00..=0xDFFF).contains(&low) {
                            paired =
                                char::from_u32(0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00));
                        }
                    }
                }
            }
            if let Some(c) = paired {
                out.push(c);
                i += 12;
            } else {
                // Lone high surrogate: leave the escape verbatim (six ASCII
                // bytes, so this slice is boundary-safe).
                out.push_str(&input[i..i + 6]);
                i += 6;
            }
            continue;
        }

        match char::from_u32(code) {
            Some(c) => {
                out.push(c);
                i += 6;
            }
            None => {
                // Lone low surrogate: leave the escape verbatim.
                out.push_str(&input[i..i + 6]);
                i += 6;
            }
        }
    }

    out
}

/// Raw-text redaction of non-JSON bodies: decode `\uXXXX` escapes into a scan
/// copy, redact the copy, and log the redacted decoded copy.
fn redact_raw(text: &str) -> String {
    redact_secrets(&decode_unicode_escapes(text))
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

    // ── JSON escapes cannot hide a key (review finding 1) ───────────────────

    /// An escape immediately before a key used to merge into the token
    /// (`\nsk-ant-…` scanned as `nsk-ant-…`) and leak the whole key. Redaction
    /// now happens on DECODED string values, so every escape kind resolves
    /// before the token scan.
    #[test]
    fn test_json_escapes_cannot_hide_a_key() {
        let secret = "sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF";
        for esc in [r"\n", r"\t", r#"\""#, r"\\", r"\u000b"] {
            let body = format!(r#"{{"messages":[{{"content":"prompt{esc}{secret}"}}]}}"#);
            let (out, _) = snippet(body.as_bytes());
            assert!(
                !out.contains(secret),
                "escape {esc}: raw key leaked into log line: {out}"
            );
            assert!(
                out.contains(REDACTED),
                "escape {esc}: redaction marker missing: {out}"
            );
            assert!(
                out.contains("prompt"),
                "escape {esc}: non-secret text must survive: {out}"
            );
        }
    }

    /// Redaction must reach strings at any nesting depth, arrays and objects
    /// alike — the walk cannot stop at the top level.
    #[test]
    fn test_json_redaction_reaches_nested_arrays_and_objects() {
        let secret = "sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF";
        let body = serde_json::json!({
            "messages": [
                {"content": [{"type": "text", "text": format!("nested {secret}")}]},
                {"tools": [{"function": {"arguments": format!(r#"{{"lookup":"{secret} here"}}"#)}}]}
            ]
        });
        let (out, _) = snippet(body.to_string().as_bytes());
        assert!(!out.contains(secret), "key leaked from nested JSON: {out}");
        assert_eq!(
            out.matches(REDACTED).count(),
            2,
            "both occurrences must be redacted: {out}"
        );
    }

    /// Non-JSON bytes take the raw-text scan, where a backslash is a literal
    /// character: `\n` before a key still leaves the key its own token, so no
    /// merge can hide it there either.
    #[test]
    fn test_raw_text_token_boundaries_survive_escapes() {
        let secret = "sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF";
        for esc in [r"\n", r"\t", r#"\""#, r"\\", r"\u000b"] {
            let text = format!("plain text{esc}{secret} tail");
            let (out, _) = snippet(text.as_bytes());
            assert!(
                !out.contains(secret),
                "escape {esc}: key leaked via raw path: {out}"
            );
            assert!(
                out.contains(REDACTED),
                "escape {esc}: marker missing: {out}"
            );
        }
    }

    // ── Large bodies: redact the FULL body, then truncate (re-review finding 1) ──

    /// A VALID JSON body whose first 4096 bytes are an incomplete document.
    /// The old code parsed only the head, so this fell back to scanning the raw
    /// truncated text. The full body must be parsed, redacted as decoded string
    /// values, re-serialized, and only then truncated.
    #[test]
    fn test_large_json_body_redacts_api_key_in_full_document() {
        let secret = "sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF";
        let body = format!(
            r#"{{"content":"{secret}","padding":"{}"}}"#,
            "a".repeat(5000)
        );
        assert!(body.len() > MAX_LOGGED_BODY_BYTES);
        let (out, truncated) = snippet(body.as_bytes());
        assert!(truncated, "body exceeds the cap — truncated must be true");
        assert!(out.len() <= MAX_LOGGED_BODY_BYTES);
        assert!(!out.contains(secret), "full key logged: {out}");
        assert!(out.contains(REDACTED), "redaction marker missing: {out}");
    }

    /// Same shape with a bearer credential: the whole credential must be gone
    /// from the truncated log line even though the head is not parseable JSON.
    #[test]
    fn test_large_json_body_redacts_bearer_in_full_document() {
        let cred = "opaqueSensitiveToken0123456789";
        let body = format!(
            r#"{{"content":"Bearer {cred}","padding":"{}"}}"#,
            "a".repeat(5000)
        );
        assert!(body.len() > MAX_LOGGED_BODY_BYTES);
        let (out, truncated) = snippet(body.as_bytes());
        assert!(truncated, "body exceeds the cap — truncated must be true");
        assert!(out.len() <= MAX_LOGGED_BODY_BYTES);
        assert!(!out.contains(cred), "full bearer token logged: {out}");
        assert!(out.contains(REDACTED), "redaction marker missing: {out}");
    }

    /// `\uXXXX` escapes INSIDE the credential prefix or the bearer scheme word
    /// used to be copied verbatim by the raw-head fallback (`sk\u002dant-…`
    /// scanned as the tokens `sk` and `ant-…`, neither matching a prefix).
    /// Parsing the full document decodes the escapes first, so redaction sees
    /// the plain credential no matter where the escape sits.
    #[test]
    fn test_escapes_inside_prefix_or_scheme_word_cannot_hide_credentials() {
        for content in [
            r"s\u006b-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF",
            r"sk\u002dant-api03-AAAABBBBCCCCDDDDEEEEFFFF",
            r"sk-a\u006et-api03-AAAABBBBCCCCDDDDEEEEFFFF",
        ] {
            let body = format!(
                r#"{{"content":"{content}","padding":"{}"}}"#,
                "a".repeat(5000)
            );
            assert!(body.len() > MAX_LOGGED_BODY_BYTES);
            let (out, _) = snippet(body.as_bytes());
            assert!(
                !out.contains("AAAABBBBCCCCDDDDEEEEFFFF"),
                "{content}: decoded key leaked: {out}"
            );
            assert!(out.contains(REDACTED), "{content}: marker missing: {out}");
        }
        for content in [
            r"\u0042earer opaqueSensitiveToken0123456789",
            r"Be\u0061rer opaqueSensitiveToken0123456789",
            r"Beare\u0072 opaqueSensitiveToken0123456789",
        ] {
            let body = format!(
                r#"{{"content":"{content}","padding":"{}"}}"#,
                "a".repeat(5000)
            );
            assert!(body.len() > MAX_LOGGED_BODY_BYTES);
            let (out, _) = snippet(body.as_bytes());
            assert!(
                !out.contains("opaqueSensitiveToken0123456789"),
                "{content}: decoded bearer leaked: {out}"
            );
            assert!(out.contains(REDACTED), "{content}: marker missing: {out}");
        }
    }

    /// A body over the parse cap gets NO content logged — just a placeholder
    /// with its size. This must hold even when the body is valid JSON holding
    /// a credential.
    #[test]
    fn test_body_over_parse_cap_omits_content_even_when_json() {
        let secret = "sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF";
        let body = format!(
            r#"{{"content":"{secret}","padding":"{}"}}"#,
            "a".repeat(MAX_DEBUG_PARSE_BYTES)
        );
        assert!(body.len() > MAX_DEBUG_PARSE_BYTES);
        let (out, truncated) = snippet(body.as_bytes());
        assert!(truncated, "an omitted body must be flagged");
        assert!(
            !out.contains(secret),
            "key leaked past the parse cap: {out}"
        );
        assert!(
            out.contains("body omitted"),
            "placeholder expected, got: {out}"
        );
    }

    /// Non-JSON text larger than the cap still goes through raw-text redaction
    /// and then truncation.
    #[test]
    fn test_large_non_json_text_is_redacted_and_truncated() {
        let secret = "sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF";
        let body = format!("prompt text {secret} {}", "tail".repeat(5000));
        assert!(body.len() > MAX_LOGGED_BODY_BYTES);
        let (out, truncated) = snippet(body.as_bytes());
        assert!(truncated);
        assert!(out.len() <= MAX_LOGGED_BODY_BYTES);
        assert!(
            !out.contains(secret),
            "key leaked from non-JSON text: {out}"
        );
        assert!(out.contains(REDACTED));
    }

    /// Raw text `\uabc` followed by a multi-byte char used to panic the
    /// escape-aware tokenizer: `digits[..4]` was sliced on a UTF-8 boundary
    /// (review finding 2). Raw text must never panic.
    #[test]
    fn test_raw_text_backslash_u_with_multibyte_char_does_not_panic() {
        let secret = "sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF";
        let body = format!(r"text \uabc😀 then {secret}");
        let (out, _) = snippet(body.as_bytes());
        assert!(!out.contains(secret), "key leaked: {out}");
        assert!(out.contains(REDACTED));
    }

    // ── Decoded strings are plain text; prefixes match anywhere (round-3 review) ──

    /// Decoded JSON strings contain LITERAL backslashes (`\\` in JSON source).
    /// The escape-aware scanner skipped `\n`/`\t`/`\b` as escapes and started
    /// the token at the letter AFTER the escape, dropping the first letter of
    /// the vendor prefix or bearer scheme word and leaking the whole
    /// credential.
    #[test]
    fn test_decoded_json_does_not_treat_backslashes_as_escapes() {
        for (body, secret) in [
            (
                r#"{"content":"key: \\nvapi-AAAABBBBCCCCDDDDEEEEFFFF"}"#,
                "nvapi-AAAABBBBCCCCDDDDEEEEFFFF",
            ),
            (
                r#"{"content":"key: \\tvly-AAAABBBBCCCCDDDDEEEEFFFF"}"#,
                "tvly-AAAABBBBCCCCDDDDEEEEFFFF",
            ),
            (
                r#"{"content":"auth: \\bearer opaqueSensitiveToken0123456789"}"#,
                "opaqueSensitiveToken0123456789",
            ),
        ] {
            let (out, _) = snippet(body.as_bytes());
            assert!(!out.contains(secret), "secret leaked: {out}");
            assert!(out.contains(REDACTED), "marker missing: {out}");
        }
    }

    /// The same three repros as raw (non-JSON) text.
    #[test]
    fn test_raw_text_does_not_treat_backslashes_as_escapes() {
        for (body, secret) in [
            (
                r"key: \nvapi-AAAABBBBCCCCDDDDEEEEFFFF",
                "nvapi-AAAABBBBCCCCDDDDEEEEFFFF",
            ),
            (
                r"key: \tvly-AAAABBBBCCCCDDDDEEEEFFFF",
                "tvly-AAAABBBBCCCCDDDDEEEEFFFF",
            ),
            (
                r"auth: \bearer opaqueSensitiveToken0123456789",
                "opaqueSensitiveToken0123456789",
            ),
        ] {
            let (out, _) = snippet(body.as_bytes());
            assert!(!out.contains(secret), "secret leaked: {out}");
            assert!(out.contains(REDACTED), "marker missing: {out}");
        }
    }

    /// A vendor prefix is found ANYWHERE, not just at a token boundary: glued
    /// to a preceding letter (`xnvapi-…`) it must still redact.
    #[test]
    fn test_prefix_redacts_when_glued_to_preceding_letters() {
        for glued in [
            "xnvapi-AAAABBBBCCCCDDDDEEEEFFFF",
            "tool-tvly-AAAABBBBCCCCDDDDEEEEFFFF",
            "abcsk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF",
        ] {
            let (out, _) = snippet(glued.as_bytes());
            assert!(
                !out.contains("AAAABBBBCCCCDDDDEEEEFFFF"),
                "{glued}: glued key leaked: {out}"
            );
            assert!(out.contains(REDACTED), "{glued}: marker missing: {out}");
        }
    }

    /// Non-JSON text has its `\uXXXX` escapes (valid hex only) decoded into a
    /// scan copy first, so an escape-encoded key or scheme word redacts too —
    /// and the redacted decoded copy is what gets logged.
    #[test]
    fn test_raw_text_decodes_unicode_escapes_before_scanning() {
        for (body, secret) in [
            (
                r"key: \u0073k-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF",
                "AAAABBBBCCCCDDDDEEEEFFFF",
            ),
            (
                r"auth: \u0042earer opaqueSensitiveToken0123456789",
                "opaqueSensitiveToken0123456789",
            ),
        ] {
            let (out, _) = snippet(body.as_bytes());
            assert!(!out.contains(secret), "decoded secret leaked: {out}");
            assert!(out.contains(REDACTED), "marker missing: {out}");
        }
    }

    /// `\uXXXX` decoding is byte-safe: malformed escapes, lone surrogates, and
    /// a multi-byte char in the digit positions must never panic, and valid
    /// surrogate pairs still decode.
    #[test]
    fn test_raw_text_unicode_decode_never_panics() {
        for text in [
            r"text \u12 sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF",
            r"text \uZZZZ sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF",
            r"text \uD800 sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF",
            r"text \uD83D\uDE00 sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF",
            r"text \uabc😀 sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF",
        ] {
            let (out, _) = snippet(text.as_bytes());
            assert!(
                !out.contains("AAAABBBBCCCCDDDDEEEEFFFF"),
                "key leaked for {text:?}: {out}"
            );
            assert!(out.contains(REDACTED));
        }
    }

    // ── Bearer credentials are redacted whole (review finding 2) ────────────

    /// The old scanner redacted only the alphanumeric token after "Bearer", so
    /// base64/base64url punctuation split the credential and logged the tail:
    /// "Bearer [REDACTED]/opaqueSensitiveToken0123456789==".
    #[test]
    fn test_bearer_redacts_the_whole_credential_regardless_of_punctuation() {
        for cred in [
            "abc/opaqueSensitiveToken0123456789==", // base64: '/', '='
            "abc+DEF0123456789+/=",                 // '+'
            "abc-def_ghi0123456789",                // base64url: '-', '_'
            "opaque~token.0123456789",              // '~', '.'
            "abc.def-ghi_jkl=",                     // '.', '-', '_', '='
        ] {
            let body = format!("Authorization: Bearer {cred}");
            let (out, _) = snippet(body.as_bytes());
            assert!(
                !out.contains(cred),
                "credential {cred} leaked into log line: {out}"
            );
            // The tail after the first punctuation char is exactly what the
            // old token-based matcher left behind.
            let tail = cred.trim_start_matches(|c: char| c.is_ascii_alphanumeric());
            assert!(!out.contains(tail), "tail {tail} of {cred} leaked: {out}");
            assert!(
                out.contains(REDACTED),
                "credential {cred}: marker missing: {out}"
            );
        }
    }

    /// The "Bearer" scheme word is case-insensitive, and a colon between the
    /// scheme and the credential is tolerated.
    #[test]
    fn test_bearer_matching_is_case_insensitive_and_tolerates_a_colon() {
        for prefix in ["bearer", "BEARER", "BeArEr"] {
            let body = format!("auth: {prefix} abc/def+ghi==");
            let (out, _) = snippet(body.as_bytes());
            assert!(
                !out.contains("abc/def+ghi=="),
                "{prefix}: credential leaked: {out}"
            );
            assert!(
                !out.contains("/def+ghi=="),
                "{prefix}: split tail leaked: {out}"
            );
        }
        for body in [
            "Authorization: Bearer:abc/def+ghi==",
            "Authorization: Bearer: abc/def+ghi==",
        ] {
            let (out, _) = snippet(body.as_bytes());
            assert!(!out.contains("abc/def+ghi=="), "colon form leaked: {out}");
            assert!(!out.contains("/def+ghi=="), "colon form tail leaked: {out}");
        }
    }

    // ── The byte cap holds on the sanitized output (review finding 3) ───────

    /// 4095 ASCII bytes + one 3-byte char = 4098 bytes. The old code cut the
    /// INPUT at 4096 and lossy-decoded afterwards, so the log line grew back
    /// past the cap (4095 + a 3-byte replacement char). The cut must happen on
    /// the sanitized output, on a char boundary.
    #[test]
    fn test_output_never_exceeds_the_cap_across_a_multibyte_char() {
        let mut body = vec![b'a'; MAX_LOGGED_BODY_BYTES - 1];
        body.extend_from_slice("✓".as_bytes());
        let (out, truncated) = snippet(&body);
        assert!(truncated, "input exceeds the cap — truncated must be true");
        assert!(
            out.len() <= MAX_LOGGED_BODY_BYTES,
            "output is {} bytes, cap is {MAX_LOGGED_BODY_BYTES}",
            out.len()
        );
    }

    /// Redaction can EXPAND text: "Bearer x" (8 bytes) becomes
    /// "Bearer [REDACTED] " (17 bytes). A body of space-separated repeats that
    /// is under the cap as input is well past it once redacted; the sanitized
    /// output must still be cut back to the cap and flagged truncated.
    #[test]
    fn test_output_never_exceeds_the_cap_after_redaction_expansion() {
        let unit = "Bearer x";
        let body = std::iter::repeat_n(unit, MAX_LOGGED_BODY_BYTES / (unit.len() + 1))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(body.len() <= MAX_LOGGED_BODY_BYTES);
        let (out, truncated) = snippet(body.as_bytes());
        assert!(
            truncated,
            "expansion beyond the cap must be marked truncated"
        );
        assert!(
            out.len() <= MAX_LOGGED_BODY_BYTES,
            "output is {} bytes, cap is {MAX_LOGGED_BODY_BYTES}",
            out.len()
        );
        assert!(
            !out.contains("Bearer x"),
            "a bearer credential survived: {out}"
        );
    }

    // ── The gate resolves at startup, not on the first request (review finding 4) ──

    // `main` calls [`resolve_at_startup`] right after tracing is initialized so
    // the operator warning lands in the startup log, not on the first provider
    // request. That call site is deliberately NOT unit-tested here (Codex
    // re-review finding 3): the hook's only observable effect is the
    // process-global [`OnceLock`], which every other `enabled()` test in this
    // binary also touches, so an assertion on `GATE_RESOLUTIONS` proves nothing
    // — it passes whether or not the hook did anything. What is testable
    // without that shared state is covered above: the decision the hook shares
    // with `enabled()` (`env_enables`, tested purely), and that the initializer
    // can never run more than once per process
    // (`test_gate_resolves_at_most_once`, robust under test parallelism because
    // the counter can only ever reach 1).
}
