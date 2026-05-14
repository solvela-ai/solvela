# solvela-protocol

Shared wire-format types for the Solvela ecosystem — x402 payment envelopes,
OpenAI-compatible chat types, model info, and constants.

This crate has zero Solana, HTTP, or async dependencies on purpose: SDKs,
clients, and the gateway all depend on it so that one canonical struct
defines each wire shape.

## Install

```toml
[dependencies]
solvela-protocol = "0.2"
```

## What's inside

- `PaymentRequired`, `PaymentPayload`, `Accept` — x402 wire types
- `ChatRequest`, `ChatResponse` — OpenAI-compatible chat types
- `ModelInfo` — model registry entry shape
- USDC mint, Solana network IDs, and other shared constants

## Links

- Gateway: <https://api.solvela.ai>
- Docs: <https://docs.solvela.ai>
- Source: [solvela-ai/solvela `crates/protocol/`](https://github.com/solvela-ai/solvela/tree/main/crates/protocol)

License: MIT
