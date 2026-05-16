# Solvela Rust client SDK

Rust workspace for paying Solvela gateways with USDC-SPL on Solana via the x402 protocol. Wallet management, payment signing, payload assembly, and a transparent localhost proxy.

| Crate | Purpose |
|---|---|
| [`solvela-client`](crates/solvela-client) | Library: wallet, x402 payment signing, payload assembly |
| [`solvela-client-cli`](crates/solvela-client-cli) | `solvela-client` binary — agent-side payer CLI |
| [`solvela-client-cli-args`](crates/solvela-client-cli-args) | Shared CLI argument structs + wallet loading |
| [`solvela-client-proxy`](crates/solvela-client-proxy) | Localhost reverse proxy that signs payments for unmodified OpenAI-format clients |

## When to use which crate

- **You're building a Rust agent** → depend on `solvela-client` directly.
- **You want a CLI tool** → install `solvela-client-cli` (published to crates.io as `solvela-client-cli`).
- **You have an OpenAI-format tool you want to pay automatically** → run `solvela-client-proxy` as a sidecar; point your tool at `http://localhost:<port>`.

> This is the **agent-side payer** CLI. The operator/dev CLI for talking to a Solvela gateway lives at [`crates/cli`](../../crates/cli) in this repo and is published as `solvela-cli`.

## Workspace shape

This SDK is **not** a member of the monorepo's root Cargo workspace. It pins `solana-sdk ~4.0` and `reqwest 0.13`, while the gateway workspace has no `solana-sdk` and uses `reqwest 0.12`. Build commands run from this directory.

```bash
cd sdks/rust
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI: [`.github/workflows/sdks-rust.yml`](../../.github/workflows/sdks-rust.yml) runs fmt, clippy, test, and audit, and retriggers on `crates/protocol/**` changes so wire-format drift fails CI in the same PR.

## Releases

Published to crates.io as `solvela-client`, `solvela-client-cli`, `solvela-client-cli-args`, `solvela-client-proxy`. The publish workflow lives in the standalone history at `solvela-ai/solvela-client` (tag-triggered); a monorepo-native equivalent will replace it in a follow-up PR.

## License

[MIT](../../LICENSE-MIT). Matches the license under which the four crates are published to crates.io. The monorepo root is BUSL-1.1; client-side SDKs are kept MIT-only so agent authors can freely embed them.
