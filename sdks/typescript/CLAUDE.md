# CLAUDE.md

This file provides guidance to Claude Code when working with this repository.

## Build / Test / Lint Commands

```bash
npm run build          # tsc — compile to dist/
npm run lint           # tsc --noEmit — type-check without emitting
npm test               # vitest run — all tests
npm run test:unit      # vitest run tests/unit
npm run test:integration  # vitest run tests/integration
```

Single test file:

```bash
npx vitest run tests/unit/cache.test.ts
npx vitest run --reporter=verbose -t "handles 402"   # by name
```

Vitest uses `globals: true` — no imports needed for `describe`/`it`/`expect`.

## Architecture

TypeScript client SDK for Solvela, the Solana-native AI agent payment gateway. Agents pay for LLM API calls with USDC-SPL via the x402 protocol.

```
src/
  constants.ts      # Protocol constants (x402 version, USDC mint, network CAIP-2)
  types.ts          # Wire-format types: ChatRequest, ChatResponse, ChatChunk, PaymentRequired, etc.
  errors.ts         # Error hierarchy: GatewayError, PaymentRequiredError, TimeoutError, etc.
  config.ts         # ClientConfig interface, DEFAULT_CONFIG, fluent ClientBuilder
  wallet.ts         # Wallet — keypair from env, base58, mnemonic
  signer.ts         # Signer trait + KeypairSigner (signs x402 payment payloads)
  transport.ts      # HTTP transport — sendChat (JSON), sendChatStream (SSE), fetchModels
  cache.ts          # ResponseCache — TTL + LRU eviction, deterministic cache keys
  session.ts        # SessionStore — session derivation from message hashes, TTL tracking
  quality.ts        # Heuristic quality checks for degraded LLM responses
  balance.ts        # BalanceMonitor — polling with low-balance callback
  client.ts         # SolvelaClient — orchestrates: balance guard → session → cache → transport → quality → retry
  openai_compat.ts  # OpenAICompat — drop-in openai.chat.completions.create() wrapper
  index.ts          # Barrel exports
```

### Request flow (client.chat)

1. Balance guard — if balance is 0 and `freeFallbackModel` is set, swap model
2. Session lookup — may override model from prior session
3. Cache check — return cached response if hit
4. Transport send — HTTP POST to `/v1/chat/completions`
5. x402 payment handshake — on 402, sign payment and retry with `Payment-Signature` header
6. Quality check — heuristic retry on degraded responses
7. Cache store + session update

### Payment flow

On HTTP 402: gateway returns `PaymentRequired` with accepted payment schemes. Client finds a compatible scheme (exact or escrow on Solana), validates recipient and amount limits, signs via `Signer`, and retries with the signature.

## Code Conventions

- ESM only (`"type": "module"` in package.json, `.js` extensions in imports)
- Strict TypeScript (`strict: true`, target ES2022, module ESNext)
- File naming: `snake_case.ts`, one concern per file
- Classes with static factory methods (`fromJSON`, `fromEnv`) — not plain objects
- Immutable builder pattern (`ClientBuilder.with()` returns new instance)
- Wire-format serialization via `toJSON()` / `fromJSON()` on type classes
- Error handling: typed error classes extending `Error`, never bare throws
- No `.unwrap()` equivalents — all errors are explicitly typed and caught
- Amounts are always in atomic USDC units (6 decimals, integer)
- Tests organized: `tests/unit/`, `tests/integration/`, `tests/live/` (CI runs unit + integration only)

## Workflow

- **Commits**: Conventional Commits with scope — `feat(client):`, `fix(cache):`, `cleanup(balance):`, `refactor:`, `chore:`. For breaking wire/cache changes, reference the SHA that introduced the break in the version-bump commit body.
- **Cross-SDK parity**: this SDK is one of a polyglot family (TS / Rust / Go / Python). Many features ship across all four — check whether a change should be mirrored, and call it out in the commit message when it is.
- **CI**: GitHub Actions matrix on Node 20/22 runs `tsc --noEmit` + unit + integration, plus an `npm audit --production --audit-level=high` job. Live tests are manual.

## Dependencies

Runtime: `@solana/web3.js`, `@solana/spl-token`, `bip39`, `bs58`
Dev: `typescript`, `vitest`, `@types/node`

## Environment

No `.env` file required for tests (all unit tests use mocks). Live tests need `SOLANA_PRIVATE_KEY` and a running gateway.
