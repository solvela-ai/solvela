# @rustyclawrouter/sdk

TypeScript SDK for RustyClawRouter -- AI agent payments with USDC on Solana via the x402 protocol.

## Installation

```bash
npm install @rustyclawrouter/sdk
```

```bash
yarn add @rustyclawrouter/sdk
```

```bash
pnpm add @rustyclawrouter/sdk
```

With Solana wallet support (transaction signing):

```bash
npm install @rustyclawrouter/sdk @solana/web3.js
```

## Quick Start

```typescript
import { LLMClient } from '@rustyclawrouter/sdk';

const client = new LLMClient({ apiUrl: 'http://localhost:8402' });

// Simple one-shot chat -- returns the assistant's text reply
const reply = await client.chat('openai/gpt-4o', 'What is the x402 protocol?');
console.log(reply);
```

## Full Chat Completion

Use `chatCompletion` for the full OpenAI-compatible response object:

```typescript
const response = await client.chatCompletion({
  model: 'anthropic/claude-sonnet-4',
  messages: [
    { role: 'system', content: 'You are a helpful assistant.' },
    { role: 'user', content: 'Explain Solana in one paragraph.' },
  ],
  maxTokens: 256,
  temperature: 0.7,
});

console.log(response.choices[0].message.content);
console.log(response.usage); // { prompt_tokens, completion_tokens, total_tokens }
```

## OpenAI Drop-in Replacement

Switch existing OpenAI SDK code to pay with USDC by changing one import:

```typescript
// Before:
// import OpenAI from 'openai';

// After:
import { OpenAI } from '@rustyclawrouter/sdk';

const client = new OpenAI({ apiUrl: 'http://localhost:8402' });

const response = await client.chat.completions.create({
  model: 'openai/gpt-4o',
  messages: [{ role: 'user', content: 'Hello!' }],
});

console.log(response.choices[0].message.content);
```

Access the underlying `LLMClient` for budget tracking and health checks:

```typescript
const llmClient = client.getClient();
console.log(llmClient.getSessionSpent());
```

## Smart Routing

Let the gateway pick the best model for the complexity of your prompt:

```typescript
// Profiles: 'eco' (cheapest), 'auto' (balanced), 'premium' (best), 'free' (open-source)
const response = await client.smartChat('Explain quantum computing', 'eco');

console.log(response.model);   // The model the router selected
console.log(response.choices[0].message.content);
```

## Session Budget Tracking

```typescript
const client = new LLMClient({
  apiUrl: 'http://localhost:8402',
  sessionBudget: 0.50, // Max $0.50 USDC per session
});

try {
  const reply = await client.chat('openai/gpt-4o', 'Hello!');
  console.log(reply);
} catch (err) {
  if (err instanceof BudgetExceededError) {
    console.log(`Spent: $${client.getSessionSpent().toFixed(6)}`);
    console.log(`Remaining: $${client.getRemainingBudget()?.toFixed(6)}`);
  }
}
```

## Error Handling

```typescript
import { LLMClient, PaymentError, BudgetExceededError } from '@rustyclawrouter/sdk';

const client = new LLMClient({ apiUrl: 'http://localhost:8402' });

try {
  const reply = await client.chat('openai/gpt-4o', 'Hello');
} catch (err) {
  if (err instanceof PaymentError) {
    // x402 payment handshake failed (e.g., malformed 402 response)
    console.error('Payment failed:', err.message);
  } else if (err instanceof BudgetExceededError) {
    // Request cost would exceed the configured session budget
    console.error('Budget exceeded:', err.message);
  } else {
    // Network error, gateway error, etc.
    console.error('Request failed:', err);
  }
}
```

## Utility Methods

```typescript
// List available models with pricing
const models = await client.listModels();

// Gateway health check
const health = await client.health();

// Session spend tracking
console.log(client.getSessionSpent());       // Total USDC spent
console.log(client.getRemainingBudget());     // Remaining budget (or undefined)
console.log(client.getApiUrl());              // Configured gateway URL
```

## Wallet

The `Wallet` class manages Solana key access. It reads from the `SOLANA_WALLET_KEY` environment variable by default, or accepts a key directly:

```typescript
import { Wallet } from '@rustyclawrouter/sdk';

const wallet = new Wallet(); // Uses SOLANA_WALLET_KEY env var
console.log(wallet.hasKey);     // true if a key is available
console.log(wallet.address);    // Solana public address (requires @solana/web3.js)
console.log(wallet.redactedKey); // "5K1g...w5gS"
```

## Configuration

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `apiUrl` | `string` | `$RCR_API_URL` or `https://api.rustyclawrouter.com` | Gateway URL |
| `privateKey` | `string` | `$SOLANA_WALLET_KEY` | Base58 Solana private key for signing |
| `sessionBudget` | `number` | `undefined` | Max USDC spend per session |
| `timeout` | `number` | `60000` | HTTP timeout in milliseconds |

## API Reference

### `LLMClient`

| Method | Returns | Description |
|--------|---------|-------------|
| `chat(model, prompt)` | `Promise<string>` | One-shot chat, returns assistant text |
| `chatCompletion(request)` | `Promise<ChatResponse>` | Full OpenAI-compatible completion |
| `smartChat(prompt, profile?)` | `Promise<ChatResponse>` | Smart-routed chat (default profile: `auto`) |
| `listModels()` | `Promise<unknown>` | List available models with pricing |
| `health()` | `Promise<unknown>` | Gateway health check |
| `getSessionSpent()` | `number` | Total USDC spent this session |
| `getRemainingBudget()` | `number \| undefined` | Remaining session budget |
| `getApiUrl()` | `string` | Configured gateway URL |

### `OpenAI`

| Method | Returns | Description |
|--------|---------|-------------|
| `chat.completions.create(params)` | `Promise<ChatResponse>` | OpenAI-compatible completion |
| `getClient()` | `LLMClient` | Access underlying client |

### Types

- `ChatMessage` -- `{ role: Role; content: string; name?: string }`
- `ChatResponse` -- `{ id, object, created, model, choices, usage? }`
- `ChatChoice` -- `{ index, message, finish_reason }`
- `Usage` -- `{ prompt_tokens, completion_tokens, total_tokens }`
- `PaymentRequired` -- 402 response with `x402_version`, `accepts`, `cost_breakdown`
- `ClientOptions` -- `{ privateKey?, apiUrl?, sessionBudget?, timeout? }`

## Testing

```bash
npm test
```

Tests use Node.js built-in test runner. No build step required for tests:

```bash
node --import tsx --test tests/client.test.ts
```

## License

MIT
