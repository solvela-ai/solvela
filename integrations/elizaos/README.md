# @solvela/elizaos-plugin

[ElizaOS](https://github.com/elizaOS/eliza) plugin that lets agents pay for LLM calls through [Solvela](https://solvela.ai) — Solana-native USDC-SPL payments via the [x402 protocol](https://x402.org).

## Status

**Pre-release (v0.1.0).** The plugin currently handles the free-tier and 402-discovery paths only — see [Limitations](#limitations) below. The `chat` action returns a clear payment-required message instead of attempting an unsigned retry. Wallet signing through ElizaOS's runtime needs more work before this plugin can complete a paid request end-to-end.

If you need a working ElizaOS → paid Solvela integration today, drop down a layer and call `@solvela/sdk` directly from a custom Eliza action — that SDK signs payments automatically.

## What it exposes

| Kind | Name | Description |
|---|---|---|
| Action | `CHAT_VIA_SOLVELA` | Sends a chat completion through Solvela. Triggers on similes like `llm call`, `ai inference`, `model query`, `ask ai`. |
| Provider | `gatewayProvider` | Surfaces Solvela gateway context for the runtime. |

## Install

```bash
npm install @solvela/elizaos-plugin
```

Register the plugin in your ElizaOS agent character / configuration:

```typescript
import { solvelaPlugin } from '@solvela/elizaos-plugin';

export const character = {
  // ...
  plugins: [solvelaPlugin],
};
```

## Settings

Both settings are read via ElizaOS `runtime.getSetting()`, not `process.env` — wire them up the same way you wire any other ElizaOS setting (`secrets`, character settings, etc.).

| Setting | Required | Default | Description |
|---|---|---|---|
| `SOLVELA_GATEWAY_URL` | Yes | — | Solvela gateway base URL (e.g. `https://api.solvela.ai`) |
| `SOLVELA_DEFAULT_MODEL` | No | `auto` | Model ID, alias, or routing profile (`auto`, `eco`, `premium`, `free`) |

## Limitations

- **Wallet signing is not implemented.** When the gateway returns `402 Payment Required`, the action returns a human-readable cost summary instead of signing and retrying. Sign-and-retry support is tracked but unscheduled.
- **No test suite yet.** `npm test` prints a placeholder. Production use should pin the published version and add your own integration tests.

## Development

```bash
cd integrations/elizaos
npm install
npm run build
```

## License

Apache-2.0
