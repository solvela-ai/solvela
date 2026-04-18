# @solvela/ai-sdk-provider

Vercel AI SDK provider for Solvela — AI agent payments with USDC on Solana via the x402 protocol. No API keys, no accounts, just wallets.

## Install

```bash
npm install @solvela/ai-sdk-provider ai @ai-sdk/provider-utils @ai-sdk/openai-compatible
```

Node >= 18.17 required. ESM only.

## Quick start

```typescript
import { generateText } from 'ai';
import { createSolvelaProvider } from '@solvela/ai-sdk-provider';
import { createLocalWalletAdapter } from '@solvela/ai-sdk-provider/adapters/local';
import { Keypair } from '@solana/web3.js';
import bs58 from 'bs58';

const keypair = Keypair.fromSecretKey(bs58.decode(process.env.SOLANA_WALLET_KEY!));
const solvela = createSolvelaProvider({
  baseURL: 'https://api.solvela.ai/v1',
  wallet: createLocalWalletAdapter(keypair),
});

const { text } = await generateText({
  model: solvela('anthropic-claude-sonnet-4-5'),
  prompt: 'Explain the x402 protocol.',
});
console.log(text);
```

Note: `createLocalWalletAdapter` is for development and testing only. See the [canonical docs](https://docs.solvela.ai/sdks/ai-sdk) for production adapter patterns.

## Features

- **OpenAI-compatible** — drop-in replacement for `@ai-sdk/openai`
- **Streaming** — real-time token streaming with x402 payment handling
- **Tool calls** — automatic tool use with Solana payment verification
- **Structured output** — opt-in schema validation via `responseFormat`
- **Session budgets** — USDC spending caps per agent instance
- **Type-safe models** — auto-generated model IDs with runtime type checking

## Documentation

Full reference, adapter authoring guide, error handling, and observability integration: [https://docs.solvela.ai/sdks/ai-sdk](https://docs.solvela.ai/sdks/ai-sdk)

### Error reference

- `SolvelaPaymentError` — payment verification failed
- `SolvelaBudgetExceededError` — session spending limit reached
- `SolvelaSigningError` — wallet adapter could not sign
- `SolvelaUpstreamError` — LLM provider error
- `SolvelaInvalidConfigError` — configuration validation failed

See the docs for retry guidance and `isRetryable` checks.

## License

MIT
