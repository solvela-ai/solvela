# @solvela/openclaw-provider

OpenClaw Provider Plugin that registers Solvela as a first-class LLM provider.
Users pick "Solvela" in OpenClaw's model picker; the plugin signs every call
transparently via the x402 protocol using USDC on Solana.

## Install

> **⚠️ Not yet published to npm.** `@solvela/openclaw-provider` is at `0.1.0`
> in `package.json` (not published; intended to ship under
> `@solvela/openclaw-provider` when distribution is greenlit). Until first
> publish, install from a local checkout:
>
> ```bash
> npm install /path/to/sdks/openclaw-provider
> ```
>
> The `@solvela/signer-core` dependency (shared x402 primitives) is wired as
> `file:../signer-core` in the lockfile during development; the manifest
> declares `^0.1.0`, which will resolve from npm once `signer-core` ships.

Once published, the canonical install will be:

```bash
npm install @solvela/openclaw-provider
```

## Environment variables

All plugin configuration is via environment variables. There is no configSchema
(removed in Phase 3 hardening pass — env vars are simpler and match the
Phase 1 MCP server pattern).

| Variable | Required | Description |
|---|---|---|
| `SOLANA_WALLET_KEY` | Yes | Base58-encoded 64-byte Solana keypair secret key |
| `SOLANA_RPC_URL` | Yes | Solana RPC endpoint (e.g. `https://api.mainnet-beta.solana.com`) |
| `SOLVELA_API_URL` | No | Gateway URL (default: `https://api.solvela.ai`). Must be `https://` in production. |
| `SOLVELA_SIGNING_MODE` | No | `auto` \| `escrow` \| `direct` \| `off` (default: `direct` — see Signing modes below) |
| `SOLVELA_SESSION_BUDGET` | No | Max USDC to spend per session (e.g. `5.00`) |
| `SOLVELA_ALLOW_DEV_BYPASS` | No | Set to `1` to allow probe-200 passthrough (dev only — see Dev bypass) |
| `SOLVELA_PROBE_TIMEOUT_MS` | No | Probe fetch timeout in ms (default: `5000`) |

## Signing modes

- **`direct`** (default): Forces direct USDC TransferChecked only. Recommended
  for v1 — no client-side escrow accounting needed.
- **`auto`**: Prefers the escrow payment scheme when the gateway advertises it,
  falls back to direct TransferChecked.
- **`escrow`**: Forces escrow-only. If the gateway doesn't advertise escrow for
  the route, the call fails.
- **`off`**: Skips signing entirely — the request is forwarded without a
  `payment-signature` header. The gateway will 402 unless it's in
  `dev_bypass_payment` mode. Parity with the Phase 1 MCP server. Never use in
  production against a live gateway.

### Escrow accounting (F4)

Escrow mode deposits a max-amount USDC envelope on-chain before the call; the
gateway claims the actual cost back from the deposit after the stream
completes. There are two ways the claim can be triggered:

1. **Gateway auto-claim after `max_timeout_seconds`** — works today; the
   deposit is reconciled on a timer, so refunds for over-deposits arrive
   asynchronously.
2. **Client-side `wrapStreamForF4` (the default in escrow mode)** — the
   plugin's wrapped `streamFn` observes the inner stream's `result()`
   Promise and fire-and-forget POSTs the actual token usage to the
   gateway's `/v1/escrow/settle` endpoint as soon as the stream finishes.
   Reduces reconciliation latency from `max_timeout_seconds` (minutes) to
   milliseconds. Settle failures are logged to stderr but never block the
   stream consumer; auto-claim remains the fallback.

The helper is also exported (`wrapStreamForF4`) for advanced wiring
outside the bundled `wrapStreamFn` path. Both the client side and the
gateway-side endpoint (`POST /v1/escrow/settle`) are live in production
as of 2026-05-26.

**Security note on `direct` mode:** Direct-transfer payments are
non-refundable if the stream fails mid-response. If recovery from
partial-stream failures matters for your workload, prefer escrow mode.

## Dev bypass

If your gateway is running in `dev_bypass_payment` mode (probe returns 200, no
402 envelope), the plugin **throws by default** to prevent silent bypass in
production. To permit passthrough explicitly:

```bash
SOLVELA_ALLOW_DEV_BYPASS=1 openclaw ...
```

Never set `SOLVELA_ALLOW_DEV_BYPASS=1` in production — it disables payment
enforcement.

## How it works

The plugin uses OpenClaw's `wrapStreamFn` hook to inject a `payment-signature`
header into every outbound inference request **before** the stream fires:

1. Before each call, the plugin probes the gateway with the request body to
   obtain a 402 PaymentRequired response containing the cost and accepted
   payment schemes.
2. The plugin calls `createPaymentHeader` from `@solvela/signer-core` to
   produce a base64-encoded signed USDC transaction.
3. The `payment-signature` header is injected into the outbound request.
4. The original stream function executes with the signed header.

The gateway's `middleware/x402.rs` reads the `payment-signature` header and
verifies the on-chain transaction — no gateway changes required.

> **Why not Authorization: Bearer?**  
> The gateway middleware reads `payment-signature` (not `Authorization`).
> Using Bearer was rejected (plan r1.3 amendment 1) because it conflates HTTP
> auth with x402 payment authorization.

## Model picker

After installing, OpenClaw's model picker shows:

- **Routing profiles** (smart router picks the model):
  - `Solvela Auto` — cheapest capable model for your prompt
  - `Solvela Eco` — force cheapest tier
  - `Solvela Premium` — force best-quality tier
  - `Solvela Free` — open-source models only
- **Real models** (direct gateway access): all 26+ models from the Solvela
  model registry (GPT-5.2, Claude Sonnet 4.6, Gemini 3.1 Pro, etc.)

## Security

- `SOLANA_WALLET_KEY` is read from the environment **per-call**, never stored on
  the plugin instance.
- Secret key bytes are zeroed after signing (handled by `@solvela/sdk`).
- Error messages never include raw key bytes or `err.cause` contents.
- Stub payment headers (`STUB_BASE64_TX`, `STUB_ESCROW_DEPOSIT_TX`) are rejected
  before injection — if `@solana/web3.js` is not resolvable at runtime, the
  plugin throws rather than silently sending an invalid payment.

## Development

```bash
npm install
npm run generate:models   # Regenerate src/models.generated.ts from config/models.toml
npm run build             # TypeScript compile
npm test                  # Run all tests
```

## Publishing

**Do not publish** until Phase 4 approval. The `package.json` version is
`0.1.0` and publishing is gated on private user testing (Phase 3) plus a
go/no-go from the user before Phase 4 distribution.

## Documentation

[docs.solvela.ai/sdks](https://docs.solvela.ai/sdks) — Solvela SDK index. (Dedicated OpenClaw provider page is pending publish.)

## License

MIT
