# @solvela/openfang-router

Route OpenFang LLM calls through Solvela for x402 USDC payment.

OpenFang plugin that forwards LLM requests to a Solvela gateway (local sidecar at `127.0.0.1:8402` on Telsi tenant VMs, or a remote `api.solvela.ai`). Handles the full x402 flow transparently — initial request, 402 response, Solana USDC-SPL signing, retry with `payment-signature` header.

> **Looking for OpenClaw?** See [`@solvela/router`](../openclaw/) — same protocol, different runtime.

## Install

```bash
npm install @solvela/openfang-router
```

## Quickstart

```ts
import { createSolvelaRouter } from '@solvela/openfang-router';

// Register with the OpenFang daemon
const router = createSolvelaRouter({
  gatewayUrl: process.env.LLM_ROUTER_API_URL ?? 'http://127.0.0.1:8402',
  walletKey: process.env.LLM_ROUTER_WALLET_KEY,  // base58 Solana key
  defaultModel: 'auto',
});

// Non-streaming completion
const resp = await router.complete({
  messages: [{ role: 'user', content: 'Summarize this thread.' }],
});
console.log(resp.choices[0].message.content);

// Streaming completion
for await (const chunk of router.completeStream({
  messages: [{ role: 'user', content: 'Write a haiku.' }],
  stream: true,
})) {
  process.stdout.write(chunk.raw);
}
```

## Config

| Field | Type | Default | Description |
|---|---|---|---|
| `gatewayUrl` | `string` | `LLM_ROUTER_API_URL` env | Solvela gateway URL (no trailing slash) |
| `walletKey` | `string?` | `LLM_ROUTER_WALLET_KEY` env | Base58 Solana private key (stub signing if absent) |
| `defaultModel` | `string` | `"auto"` | Default model id |
| `profile` | `'eco'\|'auto'\|'premium'\|'free'\|'agentic'` | unset | Routing profile. `agentic` is auto-selected when `tools` is non-empty |
| `timeoutMs` | `number` | `120_000` | Per-request timeout |

Optional env vars: `SOLANA_RPC_URL` (required when `@solana/web3.js` is installed and a real signature is needed).

## What it ships

- `createSolvelaRouter(config)` — factory returning `{ name, version, description, complete, completeStream }`
- `complete(req)` — non-streaming OpenAI-compatible chat completion
- `completeStream(req)` — async iterable of SSE chunks (`{ raw, data }`)
- `ConfigError`, `PaymentError`, `RouterError` — typed errors
- `agentic` profile auto-selection when `req.tools` is non-empty (matches Solvela's server-side router)
- Trust-tag-friendly logging prefixed with `[solvela]`

## Migrating from OpenClaw

Telsi v2 migrated OpenClaw → OpenFang. Replace your dependency:

```diff
- "@solvela/router": "^0.1.0"
+ "@solvela/openfang-router": "^0.1.0"
```

The plugin shape is intentionally compatible at the protocol level — OpenFang's plugin loader exposes a `complete`/`completeStream` interface; OpenClaw's used `intercept`/`interceptStream`. Adjust your daemon registration accordingly.

## License

MIT
