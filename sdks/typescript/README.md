# @solvela/sdk

Pay-as-you-go LLM calls in TypeScript via Solana USDC and x402 — no API keys.

Canonical SDK: https://github.com/solvela-ai/solvela-ts.

## Install

Not yet on npm (tracked in [STATUS.md](../../STATUS.md)). Install from GitHub:

```bash
npm install github:solvela-ai/solvela-ts
# or: pnpm add github:solvela-ai/solvela-ts
```

## Quickstart

Create a Solana wallet, fund it on devnet (https://faucet.solana.com), then
export `SOLVELA_WALLET_KEYFILE=~/.config/solana/id.json` (or
`SOLANA_PRIVATE_KEY=<base58-secret>`) and run:

```ts
import { Solvela } from "@solvela/sdk";

const client = new Solvela({ baseURL: "https://api.solvela.ai" }); // reads wallet from env

const resp = await client.chat.completions.create({
  model: "auto", // smart router picks the cheapest capable model
  messages: [{ role: "user", content: "Explain x402 in one sentence." }],
});
console.log(resp.choices[0].message.content);
console.log(`Paid: $${resp.payment.amountUsdc.toFixed(6)} via ${resp.payment.txSignature}`);
```

## Streaming

```ts
const stream = await client.chat.completions.create({
  model: "anthropic-claude-sonnet-4-6",
  messages: [{ role: "user", content: "Write a haiku about USDC." }],
  stream: true,
});
for await (const c of stream) process.stdout.write(c.choices[0].delta.content ?? "");
```

## Estimate cost before paying

```ts
// List pricing for every model:
for (const m of await client.models.list()) {
  console.log(m.id, m.inputCostPerMillion, m.outputCostPerMillion);
}

// Or fetch the 402 challenge without paying:
const quote = await client.chat.completions.estimate({ model: "auto", messages });
console.log(`Estimated: $${quote.costBreakdown.totalUsdc.toFixed(6)}`);
```

Free-tier example: `model: "openai-gpt-oss-120b"` is $0 (still needs a
0-amount payment header for replay protection).

## Error handling

Errors come back as a structured envelope. `e.type` is one of
`invalid_request_error`, `upstream_error`, `payment_required`, `rate_limit_error`.

```ts
import { SolvelaError } from "@solvela/sdk";
try { await client.chat.completions.create({ model: "auto", messages }); }
catch (e) {
  if (e instanceof SolvelaError) console.error(`[${e.type}] ${e.message} (code=${e.code})`);
}
```

## Links

- Standalone repo: https://github.com/solvela-ai/solvela-ts
- Docs: https://docs.solvela.ai
- Dashboard: https://solvela.vercel.app
- Gateway source: https://github.com/sky4/solvela
