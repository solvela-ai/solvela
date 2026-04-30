# solvela-go

Pay-as-you-go LLM calls in Go via Solana USDC and x402 — no API keys.

Canonical SDK: https://github.com/solvela-ai/solvela-go.

## Install

```bash
go get github.com/solvela-ai/solvela-go@latest
```

A tagged release is tracked in [STATUS.md](../../STATUS.md); `@latest` resolves to current `main` until then.

## Quickstart

Create a Solana wallet, fund it on devnet (https://faucet.solana.com), then
export `SOLVELA_WALLET_KEYFILE=~/.config/solana/id.json` (or
`SOLANA_PRIVATE_KEY=<base58-secret>`) and run:

```go
import "github.com/solvela-ai/solvela-go/solvela"

client, err := solvela.NewClient(solvela.WithBaseURL("https://api.solvela.ai"))
if err != nil { log.Fatal(err) }

resp, err := client.Chat.Completions.Create(ctx, &solvela.ChatRequest{
    Model:    "auto", // smart router picks the cheapest capable model
    Messages: []solvela.Message{{Role: "user", Content: "Explain x402 in one sentence."}},
})
if err != nil { log.Fatal(err) }
fmt.Println(resp.Choices[0].Message.Content)
fmt.Printf("Paid: $%.6f via %s\n", resp.Payment.AmountUSDC, resp.Payment.TxSignature)
```

## Streaming

```go
stream, err := client.Chat.Completions.CreateStream(ctx, &solvela.ChatRequest{
    Model:    "anthropic-claude-sonnet-4-6",
    Messages: []solvela.Message{{Role: "user", Content: "Write a haiku about USDC."}},
})
if err != nil { log.Fatal(err) }
defer stream.Close()
for c := range stream.Chan() { fmt.Print(c.Choices[0].Delta.Content) }
```

## Estimate cost before paying

```go
// List pricing for every model:
models, _ := client.Models.List(ctx)
for _, m := range models {
    fmt.Println(m.ID, m.InputCostPerMillion, m.OutputCostPerMillion)
}

// Or fetch the 402 challenge without paying:
quote, _ := client.Chat.Completions.Estimate(ctx, &solvela.ChatRequest{Model: "auto", Messages: msgs})
fmt.Printf("Estimated: $%.6f\n", quote.CostBreakdown.TotalUSDC)
```

Free-tier example: `Model: "openai-gpt-oss-120b"` is $0 (still needs a 0-amount payment header for replay protection).

## Error handling

Errors come back as a structured envelope. `se.Type` is one of
`invalid_request_error`, `upstream_error`, `payment_required`, `rate_limit_error`.

```go
var se *solvela.Error
if errors.As(err, &se) { log.Printf("[%s] %s (code=%s)", se.Type, se.Message, se.Code) }
```

## Links

- Standalone repo: https://github.com/solvela-ai/solvela-go
- Docs: https://docs.solvela.ai
- Dashboard: https://solvela.vercel.app
- Gateway source: https://github.com/sky4/solvela
