<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# go

## Purpose
Go SDK for Solvela. Flat package (no nested subpackages) exposing a `Client`, wallet loading, x402 payment construction, and escrow helpers. Signing support is the current active work item — see `docs/superpowers/plans/2026-04-10-go-sdk-signing-support.md`.

## Key Files
| File | Description |
|------|-------------|
| `README.md` | Installation + quickstart |
| `go.mod` / `go.sum` | Module manifest + checksum pins |
| `client.go` | `Client` — gateway HTTP wrapper, handles 402→sign→retry |
| `client_test.go` | Unit tests for the client |
| `config.go` | Client config struct + env-based loader |
| `types.go` | Wire types — chat request/response, payment required, cost breakdown |
| `wallet.go` | Load a Solana keypair from file or env; expose signing helpers |
| `solana.go` | Solana-specific transaction building (SPL TransferChecked, blockhash, fee payer) |
| `solana_test.go` | Unit tests for Solana helpers |
| `x402.go` | x402 header encoding/decoding |
| `x402_test.go` | Unit tests for x402 |
| `exact.go` / `exact_test.go` | `exact` payment scheme — pre-signed USDC transfer |
| `escrow.go` / `escrow_test.go` | Escrow scheme — deposit/claim/refund client |
| `signing_error.go` | Typed errors for the signing subsystem |
| `errors.go` | Top-level typed errors for the SDK |

## Subdirectories
_(none — flat package layout per Go convention)_

## For AI Agents

### Working In This Directory
- Signing work is **in progress** — before changing `wallet.go`, `solana.go`, `x402.go`, or `signing_error.go`, read `docs/superpowers/plans/2026-04-10-go-sdk-signing-support.md` (high-traffic reference).
- Go convention: exported identifiers have doc comments starting with the identifier name.
- Keep the module zero-state — every call takes an explicit `*Client` receiver, no globals.
- Never log or print raw private-key bytes; accept them as `[]byte` and zero out after use where feasible.

### Testing Requirements
```bash
go test ./...
go test ./... -v
go test -run TestClient ./...
```

### Common Patterns
- `context.Context` as the first argument of every network call.
- Typed errors via `errors.Is`/`errors.As`; sentinel errors in `errors.go`.
- `ed25519.Sign` from `crypto/ed25519` for Solana signing.

## Dependencies

### Internal
- Solvela gateway HTTP contract.

### External
- See `go.mod` — includes Solana / Ed25519 / base58 libraries.

<!-- MANUAL: -->
