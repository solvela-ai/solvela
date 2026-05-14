# @solvela/router

OpenClaw plugin that routes outbound LLM requests through the [Solvela](https://solvela.ai) gateway. Every routed call is paid for in USDC-SPL on Solana via the [x402 protocol](https://x402.org) — no API keys, no accounts, just a wallet.

## Two OpenClaw plugins — which one do you want?

The Solvela monorepo ships **two** OpenClaw plugins with different shapes. Pick the one that matches your OpenClaw version and how you want to integrate.

| Plugin | Path | OpenClaw hook | Status |
|---|---|---|---|
| **`@solvela/router`** (this package) | `integrations/openclaw/` | `intercept` / `interceptStream` | Stable v0.1.0 OSS plugin. Works against older OpenClaw versions and supports the classic intercept pattern. |
| **`@solvela/openclaw-provider`** | `sdks/openclaw-provider/` | `wrapStreamFn` | Newer first-class **provider** plugin — registers Solvela in OpenClaw's model picker so users select it like any other LLM. Built on OpenClaw's post-refactor `wrapStreamFn` contract. `1.0.0-draft`, not yet on npm; publish gated on the OpenClaw LTS announcement. |

If you're on a recent OpenClaw and want users to see "Solvela" in the model picker, use **`@solvela/openclaw-provider`** (once it ships). If you want a drop-in plugin that intercepts existing LLM calls without UI changes, use this package.

## Install

```bash
openclaw plugins install @solvela/router
```

Or as a library:

```bash
npm install @solvela/router
```

## Required environment variables

| Variable | Description |
|---|---|
| `LLM_ROUTER_API_URL` | Solvela gateway base URL (e.g. `https://api.solvela.ai`) |
| `LLM_ROUTER_WALLET_KEY` | Base58-encoded Solana private key used to sign x402 payments |

## Optional environment variables

| Variable | Description |
|---|---|
| `SOLANA_RPC_URL` | Solana RPC endpoint. Required when `@solana/web3.js` is installed locally and the plugin needs to sign on-chain transactions. |

## Usage as a library

```typescript
import { createRouter } from '@solvela/router';

const router = createRouter();
const response = await router.chat([
  { role: 'user', content: 'Hello!' },
]);
console.log(response.choices[0].message.content);
```

`createRouter()` returns a `SolvelaClient` instance. The class also exposes `chatStream()` for SSE streaming.

## Usage as an OpenClaw plugin

The default export is an `OpenClawPlugin` factory. OpenClaw loads it and calls `intercept` on every outbound LLM request; returning a response short-circuits the default provider and routes the call through Solvela instead.

```typescript
import createPlugin from '@solvela/router';

const plugin = createPlugin();
// OpenClaw calls plugin.intercept(request) / plugin.interceptStream(request)
```

## Development

```bash
cd integrations/openclaw
npm install
npm run build
npm test
```

## License

MIT
