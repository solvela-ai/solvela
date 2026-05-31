# @solvela/signer-core

Shared x402 protocol primitives for the Solvela ecosystem — payment signing, 402 parsing, scheme filtering, stub-header guards, and error sanitization. This is the single source of truth other Solvela TypeScript packages use to stay wire-compatible with the production gateway.

## Status

**Internal package — not published to npm**. Used by `sdks/mcp/`, `sdks/openclaw-provider/`, and `sdks/ai-sdk-provider/` via a `file:../signer-core` workspace dependency (the `^0.1.0` semver range in some manifests is forward-looking; today it resolves locally). If you need to sign x402 payments from your own application, depend on [`@solvela/sdk`](../typescript) instead — it bundles a `KeypairSigner` built on these primitives.

## What's inside

| Export | Kind | Purpose |
|---|---|---|
| `createPaymentHeader` | function | Build a base64-encoded `payment-signature` header. Accepts optional `PaymentExpectations` so a phishing 402 can't redirect funds to a different recipient or escrow program (highly recommended). |
| `decodePaymentHeader` | function | Round-trip helper for tests/debug. |
| `parse402` | function | Parse a `402 Payment Required` body — accepts both the envelope shape (`{ error: { message: <json> } }`) and the direct shape. Throws on malformed input. |
| `filterAccepts` | function | Scheme-based filter over a `PaymentRequired.accepts[]` list with mode support (`auto` / `escrow` / `direct` / `off`). |
| `isStubHeader`, `isStubTransaction` | function | Detect stub payment headers and stub markers in extracted transactions — guards against sending unsigned placeholder payloads. |
| `sanitizeGatewayError` | function | Slice + redact gateway error bodies before logging or surfacing them. |
| `redactBase58`, `redactHex` | function | Byte-pattern redactors for log output. |
| `SigningError` | error | Thrown by `createPaymentHeader` on signing failure. |
| `PaymentRequired`, `PaymentAccept`, `CostBreakdown`, `PaymentExpectations` | type | Wire-format types matching the gateway's 402 response shape. |

## Why it exists

Three TypeScript packages need to sign x402 payments against the same gateway: the MCP server, the OpenClaw provider, and the Vercel AI SDK provider. Duplicating the wire-format logic across all three caused a real incident — the 2026-05-11 `ModelInfo` zero-fill — so the signing primitives now live in one place with their own tests and CI. Changes here are picked up by all three consumers in a single PR; cross-package drift fails CI rather than shipping silently.

## Development

```bash
cd sdks/signer-core
npm install
npm run build         # tsc
npm test              # node:test runners across parse-402, scheme-filter, stub-guard, redact, sign
npm run typecheck     # tsc --noEmit
```

## License

Apache-2.0
