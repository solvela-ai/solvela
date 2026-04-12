# Go SDK

The Go SDK provides a `Client` with transparent x402 payment handling using the functional options pattern.

## Installation

```bash
go get github.com/solvela/sdk-go
```

## Quick Start

```go
package main

import (
    "context"
    "fmt"
    "log"

    rcr "github.com/solvela/sdk-go"
)

func main() {
    client, err := rcr.NewClient(
        rcr.WithAPIURL("http://localhost:8402"),
        rcr.WithPrivateKey("your-base58-private-key"),
    )
    if err != nil {
        log.Fatal(err)
    }

    reply, err := client.Chat(context.Background(), "openai/gpt-4o", "Hello!")
    if err != nil {
        log.Fatal(err)
    }
    fmt.Println(reply)
}
```

## Configuration

The client uses the functional options pattern:

```go
client, err := rcr.NewClient(
    rcr.WithAPIURL("http://localhost:8402"),
    rcr.WithPrivateKey("base58-private-key"),
    rcr.WithSessionBudget(1.0),              // max USDC per session
    rcr.WithTimeout(60 * time.Second),       // request timeout
    rcr.WithHTTPClient(customHTTPClient),     // custom http.Client
)
```

Environment variables:

| Variable | Description |
|----------|-------------|
| `RCR_API_URL` | Gateway URL |
| `SOLANA_WALLET_KEY` | Base58 wallet private key |
| `SOLANA_RPC_URL` | Solana RPC endpoint |

## Chat Completion

```go
ctx := context.Background()

response, err := client.ChatCompletion(ctx, rcr.ChatRequest{
    Model: "anthropic/claude-sonnet-4.6",
    Messages: []rcr.ChatMessage{
        {Role: "system", Content: "You are a Go expert."},
        {Role: "user", Content: "Explain interfaces."},
    },
    MaxTokens:   1000,
    Temperature: 0.5,
})
if err != nil {
    log.Fatal(err)
}

fmt.Println(response.Choices[0].Message.Content)
fmt.Printf("Tokens: %d\n", response.Usage.TotalTokens)
```

## Streaming

```go
stream, err := client.ChatStream(ctx, rcr.ChatRequest{
    Model: "openai/gpt-4o",
    Messages: []rcr.ChatMessage{
        {Role: "user", Content: "Write a haiku about blockchains"},
    },
})
if err != nil {
    log.Fatal(err)
}

for chunk := range stream {
    if chunk.Error != nil {
        log.Fatal(chunk.Error)
    }
    if len(chunk.Choices) > 0 && chunk.Choices[0].Delta.Content != "" {
        fmt.Print(chunk.Choices[0].Delta.Content)
    }
}
fmt.Println()
```

## Smart Routing

```go
// Use profile aliases as the model name
reply, _ := client.Chat(ctx, "auto", "Hello!")        // balanced
reply, _ = client.Chat(ctx, "eco", "Quick question")   // cheapest
reply, _ = client.Chat(ctx, "premium", "Deep analysis") // best
```

## Error Handling

```go
reply, err := client.Chat(ctx, "openai/gpt-4o", "Hello")
if err != nil {
    var budgetErr *rcr.BudgetExceededError
    var paymentErr *rcr.PaymentError
    var providerErr *rcr.ProviderError

    switch {
    case errors.As(err, &budgetErr):
        fmt.Printf("Budget exceeded. Spent: %.4f USDC\n", budgetErr.Spent)
    case errors.As(err, &paymentErr):
        fmt.Printf("Payment failed: %s\n", paymentErr.Message)
    case errors.As(err, &providerErr):
        fmt.Printf("Provider error %d: %s\n", providerErr.StatusCode, providerErr.Message)
    default:
        fmt.Printf("Error: %v\n", err)
    }
}
```

## List Models

```go
models, err := client.ListModels(ctx)
if err != nil {
    log.Fatal(err)
}

for _, model := range models {
    fmt.Printf("%s: $%.2f/M input\n",
        model.ID,
        model.Pricing.InputCostPerMillion,
    )
}
```

## Session Budget

```go
client, _ := rcr.NewClient(
    rcr.WithAPIURL("http://localhost:8402"),
    rcr.WithPrivateKey("your-key"),
    rcr.WithSessionBudget(0.50),
)

reply, err := client.Chat(ctx, "openai/gpt-4o", "Hello")
if err != nil {
    // Returns BudgetExceededError if session limit is hit
    log.Fatal(err)
}

fmt.Printf("Spent so far: %.4f USDC\n", client.SessionSpent())
```

```admonish tip
The Go SDK does not provide an OpenAI-compatible wrapper because there is no dominant Go OpenAI SDK pattern to mimic. Use the native `Client` API directly.
```
