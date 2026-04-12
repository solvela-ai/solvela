# TypeScript SDK

The TypeScript SDK provides an `LLMClient` class with transparent x402 payment handling, streaming support, and session budgets.

## Installation

```bash
npm install @solvela/sdk
```

## Quick Start

```typescript
import { LLMClient } from '@solvela/sdk';

const client = new LLMClient({ apiUrl: 'http://localhost:8402' });

const reply = await client.chat('openai/gpt-4o', 'Explain the x402 protocol');
console.log(reply);
```

## Configuration

```typescript
const client = new LLMClient({
  apiUrl: 'http://localhost:8402',        // or RCR_API_URL env var
  sessionBudget: 1.0,                     // max USDC per session
  timeout: 60000,                         // request timeout in ms
});
```

Environment variables:

| Variable | Description |
|----------|-------------|
| `RCR_API_URL` | Gateway URL (default: `https://api.solvela.com`) |
| `SOLANA_WALLET_KEY` | Base58 wallet private key |
| `SOLANA_RPC_URL` | Solana RPC endpoint |

## Chat Completion

```typescript
const response = await client.chatCompletion({
  model: 'anthropic/claude-sonnet-4.6',
  messages: [
    { role: 'system', content: 'You are a Rust expert.' },
    { role: 'user', content: 'Explain lifetimes.' },
  ],
  maxTokens: 1000,
  temperature: 0.5,
});

console.log(response.choices[0].message.content);
console.log(`Tokens: ${response.usage.total_tokens}`);
```

## Streaming

```typescript
const stream = client.chatStream({
  model: 'openai/gpt-4o',
  messages: [{ role: 'user', content: 'Write a poem about decentralized AI' }],
});

for await (const chunk of stream) {
  process.stdout.write(chunk.choices[0]?.delta?.content || '');
}
console.log();
```

## Smart Routing

```typescript
// Use profile aliases as the model name
const reply = await client.chat('auto', 'Hello!');           // balanced
const reply2 = await client.chat('eco', 'Quick question');   // cheapest
const reply3 = await client.chat('premium', 'Deep analysis'); // best quality
```

## Error Handling

```typescript
import { LLMClient, PaymentError, BudgetExceededError } from '@solvela/sdk';

const client = new LLMClient({
  apiUrl: 'http://localhost:8402',
  sessionBudget: 0.50,
});

try {
  const reply = await client.chat('openai/gpt-4o', 'Hello');
  console.log(reply);
} catch (error) {
  if (error instanceof BudgetExceededError) {
    console.error(`Budget exceeded. Spent: $${error.spent.toFixed(4)}`);
  } else if (error instanceof PaymentError) {
    console.error(`Payment failed: ${error.message}`);
  } else {
    console.error(`Unexpected error: ${error}`);
  }
}
```

## List Models

```typescript
const models = await client.listModels();

for (const model of models) {
  console.log(`${model.id}: $${model.pricing.input_cost_per_million}/M input`);
}
```

## OpenAI Compatibility

The SDK provides an OpenAI-compatible wrapper that works as a drop-in replacement:

```typescript
import { OpenAICompat } from '@solvela/sdk';

const openai = new OpenAICompat({ apiUrl: 'http://localhost:8402' });

// Same interface as the OpenAI SDK
const completion = await openai.chat.completions.create({
  model: 'openai/gpt-4o',
  messages: [{ role: 'user', content: 'Hello!' }],
});

console.log(completion.choices[0].message.content);
```

This is a wrapper class (not a subclass) with zero dependency on the `openai` npm package.

## Session Tracking

The SDK tracks spend per session:

```typescript
const client = new LLMClient({
  apiUrl: 'http://localhost:8402',
  sessionBudget: 2.0,
});

await client.chat('openai/gpt-4o-mini', 'Quick question');
console.log(`Spent so far: $${client.sessionSpent.toFixed(4)}`);
```
