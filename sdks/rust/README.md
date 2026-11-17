# Solvela Rust client SDK

Rust workspace for paying [Solvela](https://solvela.ai) gateways with USDC-SPL on
Solana via the [x402](https://github.com/solvela-ai/solvela) protocol. Wallet
management, payment signing, payload assembly, and a transparent localhost proxy
for unmodified OpenAI-format clients.

## Crates

All four are published to crates.io at `0.2.5`.

| Crate | crates.io | Purpose |
|---|---|---|
| [`solvela-client`](crates/solvela-client) | [↗](https://crates.io/crates/solvela-client) | Library: `Wallet`, `SolvelaClient`, x402 payment signing, payload assembly |
| [`solvela-client-cli`](crates/solvela-client-cli) | [↗](https://crates.io/crates/solvela-client-cli) | `solvela-client` binary — agent-side payer CLI (wallet, chat, models, doctor) |
| [`solvela-client-cli-args`](crates/solvela-client-cli-args) | [↗](https://crates.io/crates/solvela-client-cli-args) | Shared CLI argument structs + wallet loading (used by the two binaries) |
| [`solvela-client-proxy`](crates/solvela-client-proxy) | [↗](https://crates.io/crates/solvela-client-proxy) | `solvela-client-proxy` binary — localhost reverse proxy that signs payments for unmodified OpenAI clients |

> **Payer vs operator CLI.** The `solvela-client` binary here is the
> **agent-side payer** — it holds a wallet and pays gateways. The
> operator/developer CLI for running and inspecting a gateway lives at
> [`crates/cli`](../../crates/cli) in this repo and is published as `solvela-cli`.

## Which crate do I want?

**Building a Rust agent that pays per call** → depend on the `solvela-client` library:

```bash
cargo add solvela-client solvela-protocol
```

**Want a terminal CLI to send paid chat completions** → install `solvela-client-cli`:

```bash
cargo install solvela-client-cli
solvela-client wallet create          # writes ~/.solvela/wallet.json
solvela-client wallet balance         # check USDC-SPL balance
solvela-client chat "explain x402 in one sentence" --model auto
```

**Have an OpenAI-format tool you want to pay through Solvela with no code changes**
→ run `solvela-client-proxy` as a sidecar and point your tool at it:

```bash
cargo install solvela-client-proxy
solvela-client-proxy --port 8402 --max-payment 0.10
# then point your OpenAI client's base URL at http://localhost:8402
```

## Library quickstart

```rust
use solvela_client::{ClientBuilder, SolvelaClient, Wallet};
use solvela_protocol::{ChatMessage, ChatRequest, Role};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load a wallet from a base58 keypair in the environment.
    // `SOLVELA_WALLET_KEY` is the variable the CLI tooling defaults to; the
    // library lets you pick any name.
    let wallet = Wallet::from_env("SOLVELA_WALLET_KEY")?;

    let config = ClientBuilder::new()
        .gateway_url("https://api.solvela.ai")
        .build_config();
    let client = SolvelaClient::new(wallet, config)?;

    let req = ChatRequest {
        model: "auto".to_string(),
        messages: vec![ChatMessage {
            role: Role::User,
            content: "hello".to_string(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }],
        max_tokens: None,
        temperature: None,
        top_p: None,
        stream: false,
        tools: None,
        tool_choice: None,
    };

    // `chat` performs the full x402 handshake: it sends the request, signs the
    // 402 challenge with the wallet, and retries with the payment signature.
    let resp = client.chat(req).await?;
    println!("{}", resp.choices[0].message.content);
    Ok(())
}
```

`SolvelaClient::chat_stream` runs the same flow as an SSE stream. See
[`crates/solvela-client/src/client.rs`](crates/solvela-client/src/client.rs) for
the full surface, and `ClientBuilder` in
[`config.rs`](crates/solvela-client/src/config.rs) for the tuning knobs
(escrow preference, max-payment cap, response cache, sessions, quality retries,
free-fallback model).

## Nano (XNO) receipts

This SDK signs Solana USDC-SPL payments today; paying with Nano (publishing a
send block and signing the receipt) is a tracked follow-up. The receipt wire
shape it must eventually produce is already defined by the shared protocol
crate, and the SDK's tests round-trip it:

```json
{
  "block_hash": "<64 uppercase hex chars>",
  "account": "nano_<...>",
  "signature": "<128 hex chars>"
}
```

`signature` is the paying account's Ed25519-BLAKE2b signature (Nano's own
variant, **not** Solana's SHA-512 Ed25519) over the **BLAKE2b-256 digest** of
the bytes returned by
[`solvela_protocol::nano::receipt_signing_message`](../../crates/protocol/src/nano.rs):
the `solvela-nano-receipt:v1` domain tag followed by the canonical block hash,
the canonical decimal raw amount from the 402 `accepts[]` entry, the `pay_to`
address, and the request `resource` method and URL, NUL-separated. Signing a
32-byte digest is Nano's native signing shape — a standard Nano signer can sign
it (`signBlock({hash})` in the JS library, or a node's hash-signing RPC) with no
Solvela-specific Ed25519 code. The gateway
checks that signature against the public key encoded in `account` before it
contacts a Nano node, and records the hash permanently so a receipt can be
redeemed exactly once.

## Workspace shape and CI

This SDK is **not** a member of the monorepo's root Cargo workspace. It pins
`solana-sdk ~4.0` and `reqwest 0.13`, while the gateway workspace has no
`solana-sdk` and uses `reqwest 0.12`. Build and test commands run from this
directory:

```bash
cd sdks/rust
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI: [`.github/workflows/sdks-rust.yml`](../../.github/workflows/sdks-rust.yml)
runs fmt, clippy, test, and audit. Its path triggers include
`crates/protocol/**` because the SDK consumes `solvela-protocol` via a local
path dependency — any wire-format change in the protocol crate rebuilds and
retests this SDK in the **same PR**. That same-PR drift coverage is why the SDK
was consolidated into the monorepo (PR
[#316](https://github.com/solvela-ai/solvela/pull/316), 2026-05-16).

## Releases

The four crates are published to crates.io at **`0.2.5`**:
[`solvela-client`](https://crates.io/crates/solvela-client),
[`solvela-client-cli`](https://crates.io/crates/solvela-client-cli),
[`solvela-client-cli-args`](https://crates.io/crates/solvela-client-cli-args),
[`solvela-client-proxy`](https://crates.io/crates/solvela-client-proxy).

Releases are cut by pushing a `sdks/rust/v*` tag, which triggers
[`.github/workflows/sdk-rust-publish.yml`](../../.github/workflows/sdk-rust-publish.yml)
to publish all four crates in dependency order with cargo's
`--token $CARGO_REGISTRY_TOKEN`. The previous tag-triggered workflow lived in
the now-archived `solvela-ai/solvela-client` repo.

## License

[Apache-2.0](../../LICENSE-APACHE) — matches the license under which the four
crates are published to crates.io. The Solvela monorepo uses a per-component
license split: the gateway crate (`crates/gateway`) is BUSL-1.1; every other
crate, the SDKs, and the dashboard are Apache-2.0. The permissive terms plus an
explicit patent grant let agent authors freely embed them. See the
[root README Licensing section](../../README.md#licensing) for the full table.
