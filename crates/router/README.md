# solvela-router

15-dimension rule-based request scorer, routing profiles, and model registry
for the [Solvela](https://solvela.ai) LLM payment gateway.

The router classifies each incoming chat request across 15 weighted
dimensions (code presence, reasoning markers, technical terms, etc.) into
tiers — Simple / Medium / Complex / Reasoning — then maps each tier to a
specific model under the chosen profile (eco / auto / premium / free).
Scoring is pure and makes zero external calls (measured 2026-07-08: ~7.7 us
per classification, release build, keyword-dense prompt).

## Install

```toml
[dependencies]
solvela-router = "0.2"
```

## What's inside

- `scorer` — 15-dimension request scorer
- `profiles` — eco/auto/premium/free routing profiles
- `models` — model registry loader (consumes `config/models.toml`, configured by the host application)

## Links

- Gateway: <https://api.solvela.ai>
- Docs: <https://docs.solvela.ai>
- Source: [solvela-ai/solvela `crates/router/`](https://github.com/solvela-ai/solvela/tree/main/crates/router)

License: Apache-2.0
