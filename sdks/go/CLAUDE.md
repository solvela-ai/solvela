# CLAUDE.md

This file provides guidance to Claude Code when working with this repository.

## Build & Test Commands

```bash
go test ./... -v -count=1          # Run all tests
go test ./... -cover               # Run tests with coverage summary
go test ./... -tags=live           # Run live tests (requires running gateway)
go vet ./...                       # Static analysis
go fmt ./...                       # Format all files

# Manual smoke test against a live gateway (catches wire-format drift the
# unit/integration tests can't). Mirrors solvela-python scripts/smoke.py and
# solvela-ts scripts/smoke.ts. Defaults to http://localhost:8402.
SOLVELA_GATEWAY_URL=https://staging.solvela.ai go run ./scripts/smoke
```

Single-file test example:

```bash
go test -v -run TestCacheKey       # Run tests matching pattern
```

## Architecture

Single-package Go SDK (`package solvela`) — no sub-packages. Module: `github.com/solvela-ai/solvela/sdks/go`.

| File | Purpose |
|------|---------|
| `client.go` | `SolvelaClient` — main entry point. `Chat()`, `ChatStream()`, `Models()`. Orchestrates payment, caching, sessions, quality checking. |
| `transport.go` | `Transport` — HTTP layer. `SendChat()`, `SendChatStream()` (SSE), `FetchModels()`. Handles 200/402/error responses. |
| `wallet.go` | `Wallet` — ed25519 keypair. `CreateWallet()`, `WalletFromKeypairB58()`, `WalletFromEnv()`. Uses `mr-tron/base58`. |
| `signer.go` | `Signer` interface + `KeypairSigner` stub. Pluggable payment transaction signing. |
| `cache.go` | `ResponseCache` — thread-safe LRU with TTL and dedup window. Default: 100 entries, 5m TTL, 2s dedup. |
| `session.go` | `SessionStore` — conversation tracking with three-strike model escalation. |
| `quality.go` | `CheckDegraded()` — detects empty, error-phrase, repetitive, or truncated responses. |
| `balance.go` | `BalanceMonitor` — periodic balance polling with low-balance callbacks. |
| `config.go` | `ClientConfig` + functional `Option` pattern (e.g., `WithGatewayURL()`, `WithCache()`). |
| `types.go` | OpenAI-compatible request/response types, x402 payment types. |
| `errors.go` | Typed errors: `PaymentRequiredError`, `GatewayError`, `AmountExceedsMaxError`, etc. |
| `constants.go` | `X402Version`, `USDCMint`, `SolanaNetwork`, `PlatformFeePercent`. |

### Request flow

1. Balance guard — fall back to free model if wallet is empty
2. Session lookup — check for model escalation (three-strike rule)
3. Cache check — return cached response if available
4. `sendWithPayment()` — send request, handle 402 (sign + retry)
5. Quality check — retry if response is degraded
6. Cache store + session update

## Code Conventions

- **Single dependency**: only `github.com/mr-tron/base58` (Go 1.23.7)
- **Functional options**: `NewClient(wallet, signer, WithCache(true), ...)`
- **Thread safety**: `sync.Mutex` on all shared state (`ResponseCache`, `SessionStore`, `BalanceMonitor`)
- **Error types**: concrete structs implementing `error`, not sentinel values
- **No `.unwrap()` equivalent**: all errors returned, never panicked
- **OpenAI-compatible**: types mirror the OpenAI chat completions API
- **x402 protocol**: payment negotiation via HTTP 402 with `Payment-Signature` header
- **Secrets**: `Wallet.String()` and `SolvelaClient.String()` always redact private keys

## CI

GitHub Actions runs `go test` and `go vet` on Go 1.23 (see `.github/workflows/ci.yml`).
