<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# src

## Purpose
TypeScript SDK source. Flat module layout — one concern per file.

## Key Files
| File | Description |
|------|-------------|
| `index.ts` | Public entry — re-exports `Client`, `Wallet`, types, and `openai-compat` |
| `client.ts` | `Client` — gateway HTTP wrapper, handles 402→sign→retry |
| `wallet.ts` | Keypair loading + Solana signing helpers |
| `x402.ts` | x402 header encoding, payment-required parsing |
| `types.ts` | Wire types — chat request/response, payment required, cost breakdown |
| `openai-compat.ts` | Drop-in shim for `openai` npm package consumers — construct a Client that exposes `chat.completions.create` |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Public surface lives in `index.ts` — every consumer-visible export goes there.
- `openai-compat.ts` is the main adoption lever; keep its method signatures aligned with the `openai` package so users can swap clients with minimal changes.
- Errors as discriminated unions, not thrown strings.

### Testing Requirements
```bash
npm --prefix sdks/typescript test
```

### Common Patterns
- Fetch API; no axios.
- `Uint8Array` for all binary data (keys, signatures, transaction bytes).
- Async/await throughout — no manual promise chains.

## Dependencies

### Internal
_(none — leaf)_

### External
- See `../package.json`.

<!-- MANUAL: -->
