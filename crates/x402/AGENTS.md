<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# x402

## Purpose
Pure x402 payment-protocol library. Handles Solana SPL-token payment verification, escrow interactions, fee-payer pool, and nonce pool. **No Axum / web framework dependency** — consumed by both the gateway and the CLI. Exposes a chain-agnostic `PaymentVerifier` trait so a future EVM/Base implementation can slot in without touching callers.

## Key Files
| File | Description |
|------|-------------|
| `Cargo.toml` | `postgres` feature gates optional `sqlx` dep for claim-queue persistence |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `src/` | All protocol code (see `src/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- Keep this crate **pure** — no Axum, tower, reqwest-middleware, or framework code. If you need HTTP, use plain `reqwest`.
- The `PaymentVerifier` trait is the public seam for future chains. New chain support = new impl + new module; do not widen the trait signature to assume Solana specifics.
- All crypto operations are constant-time: `ed25519-dalek`, `curve25519-dalek`, `zeroize` for key material.
- Feature gating: `postgres = ["sqlx"]` — new persistent state must be feature-gated or the crate loses its "pure protocol" property.

### Testing Requirements
```bash
cargo test -p x402
cargo test -p x402 -- --nocapture
```

### Common Patterns
- Atomic USDC amounts (u64, 6 decimals) — never f64.
- Errors via `thiserror::Error`.
- RPC calls wrapped with retry + timeout helpers in `solana_rpc.rs`.

## Dependencies

### Internal
- `solvela-protocol` for shared wire-format types.

### External
- `ed25519-dalek`, `curve25519-dalek`, `zeroize`, `base64`, `bs58`, `sha2`, `reqwest`, `tokio`, `serde`, `thiserror`, `tracing`, `metrics`, `sqlx` (optional via `postgres` feature).

<!-- MANUAL: -->
