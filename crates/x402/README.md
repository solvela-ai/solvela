# solvela-x402

Rust implementation of the [x402 payment protocol](https://x402.org) for
Solana — payment verification, escrow integration, fee-payer pool, and
nonce pool. Powers the [Solvela](https://solvela.ai) LLM gateway.

This crate is HTTP-framework-agnostic. The `PaymentVerifier` trait is
chain-agnostic by design so EVM verifiers can be added later without
disturbing the Solana implementation.

## Install

```toml
[dependencies]
solvela-x402 = "0.2"

# Optional: PostgreSQL-backed nonce store for replay protection.
solvela-x402 = { version = "0.2", features = ["postgres"] }
```

## What's inside

- Solana USDC-SPL `exact`-scheme verification
- Escrow-scheme integration with the Solvela escrow program
- Fee-payer pool — pre-funded keys that sign on behalf of agents
- Nonce pool — durable nonces for offline signing
- `PaymentVerifier` trait — chain-agnostic verifier surface

## Links

- Gateway: <https://api.solvela.ai>
- Docs: <https://docs.solvela.ai>
- Source: [solvela-ai/solvela `crates/x402/`](https://github.com/solvela-ai/solvela/tree/main/crates/x402)

License: MIT
