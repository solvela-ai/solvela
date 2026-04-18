# @solvela/ai-sdk-provider

> Vercel AI SDK provider for Solvela — AI agent payments with USDC on Solana via the x402 protocol.

**Status: under construction.** Full documentation at [docs.solvela.ai/sdks/ai-sdk](https://docs.solvela.ai/sdks/ai-sdk) (Phase 9).

## Install

```sh
npm install @solvela/ai-sdk-provider ai @ai-sdk/provider-utils @ai-sdk/openai-compatible
```

Node >= 18.17 required. ESM only.

## Quick start

```typescript
import { generateText } from 'ai';
import { createSolvelaProvider } from '@solvela/ai-sdk-provider';
import { createLocalWalletAdapter } from '@solvela/ai-sdk-provider/adapters/local'; // DEV/TEST ONLY
import { Keypair } from '@solana/web3.js';
import bs58 from 'bs58';

const keypair = Keypair.fromSecretKey(bs58.decode(process.env.SOLANA_WALLET_KEY!));

const solvela = createSolvelaProvider({
  baseURL: 'https://api.solvela.ai/v1',
  wallet: createLocalWalletAdapter(keypair),
});

const { text } = await generateText({
  model: solvela('claude-sonnet-4-5'),
  prompt: 'Explain the x402 protocol.',
});
```

See full documentation for production adapter implementations, budget configuration, and error handling.
