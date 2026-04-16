<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# providers

## Purpose
LLM provider adapters. Each file translates between the gateway's OpenAI-compatible wire format and the upstream provider's native format (request + streaming + response). Also includes liveness / health-check logic and a fallback strategy when a primary provider fails.

## Key Files
| File | Description |
|------|-------------|
| `mod.rs` | Module root; provider registry + `Provider` trait |
| `openai.rs` | OpenAI chat completions adapter — passthrough for request shape; normalizes streaming deltas |
| `anthropic.rs` | Anthropic Messages API adapter — maps OpenAI `messages` ↔ Anthropic `messages` with system split, tool-use conversion |
| `google.rs` | Google Gemini adapter — maps to `generateContent` / `streamGenerateContent` |
| `xai.rs` | xAI (Grok) adapter — mostly OpenAI-compatible with minor field differences |
| `deepseek.rs` | DeepSeek adapter — OpenAI-compatible endpoint, custom base URL |
| `health.rs` | Per-provider health probing (periodic `/models` or equivalent) |
| `heartbeat.rs` | Background task emitting provider liveness metrics |
| `fallback.rs` | Fallback chain: if primary fails, try the next provider in the chain |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- **Cardinal rule**: the gateway always speaks OpenAI-compat externally. Adapters translate to/from that shape. Never leak native field names in the response.
- API keys come from env vars (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GOOGLE_API_KEY`, `XAI_API_KEY`, `DEEPSEEK_API_KEY`) only — never config files.
- Streaming: map the upstream chunk to an OpenAI SSE delta (`data: {…}\n\n`, final `data: [DONE]\n\n`).
- Retries / timeouts live in each adapter; don't rely on global middleware for provider-specific errors.

### Testing Requirements
```bash
cargo test -p gateway providers
```
Tests use recorded/mocked HTTP fixtures.

### Common Patterns
- Each adapter exposes `async fn complete(req, key) -> Result<ChatResponse, GatewayError>` and `async fn stream(req, key) -> Result<impl Stream<Item = Chunk>, GatewayError>`.
- Prefer plain `reqwest::Client` (no SDK) so adapters stay small.

## Dependencies

### Internal
- `crate::error::GatewayError`, `solvela-protocol` types.

### External
- `reqwest`, `serde`, `serde_json`, `tokio`, `futures`, `tracing`.

<!-- MANUAL: -->
