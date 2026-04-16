# Solvela Go SDK

Go SDK for Solvela -- AI agent payments with USDC on Solana via the x402 protocol.

Module path: `github.com/solvela/sdk-go`

## Installation

```bash
go get github.com/solvela/sdk-go
```

Requires Go 1.21 or later.

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
	)
	if err != nil {
		log.Fatal(err)
	}

	// Simple one-shot chat -- returns the assistant's text reply
	reply, err := client.Chat(context.Background(), "openai/gpt-4o", "What is the x402 protocol?")
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println(reply)
}
```

## Full Chat Completion

Use `ChatCompletion` for the full OpenAI-compatible response object:

```go
resp, err := client.ChatCompletion(context.Background(), rcr.ChatRequest{
	Model: "anthropic/claude-sonnet-4",
	Messages: []rcr.ChatMessage{
		{Role: rcr.RoleSystem, Content: "You are a helpful assistant."},
		{Role: rcr.RoleUser, Content: "Explain Solana in one paragraph."},
	},
})
if err != nil {
	log.Fatal(err)
}

fmt.Println(resp.Choices[0].Message.Content)
fmt.Printf("Tokens: %d in / %d out\n",
	resp.Usage.PromptTokens, resp.Usage.CompletionTokens)
```

## Smart Routing

Let the gateway pick the best model for the complexity of your prompt:

```go
// Profiles: "eco" (cheapest), "auto" (balanced), "premium" (best), "free" (open-source)
resp, err := client.SmartChat(context.Background(), "Explain quantum computing", "eco")
if err != nil {
	log.Fatal(err)
}

fmt.Printf("Model: %s\n", resp.Model)
fmt.Println(resp.Choices[0].Message.Content)
```

## Session Budget Tracking

```go
client, _ := rcr.NewClient(
	rcr.WithAPIURL("http://localhost:8402"),
	rcr.WithSessionBudget(0.50), // Max $0.50 USDC per session
)

reply, err := client.Chat(context.Background(), "openai/gpt-4o", "Hello!")
if err != nil {
	var be *rcr.BudgetExceededError
	if errors.As(err, &be) {
		fmt.Printf("Budget: $%.4f, Spent: $%.4f, Cost: $%.4f\n",
			be.Budget, be.Spent, be.Cost)
	}
}

// Check session spending
fmt.Printf("Session spent: $%.6f USDC\n", client.GetSessionSpent())

// Detailed spending summary
summary, _ := client.GetSpending(context.Background())
fmt.Printf("Spent: $%.6f, Remaining: %v\n",
	summary.SessionSpentUSDC, summary.BudgetRemaining)
```

## Error Handling

All errors are typed. Use `errors.As` or `errors.Is` to inspect them:

```go
import "errors"

reply, err := client.Chat(ctx, "openai/gpt-4o", "Hello")
if err != nil {
	var pe *rcr.PaymentError
	var be *rcr.BudgetExceededError
	var ae *rcr.APIError

	switch {
	case errors.As(err, &pe):
		// x402 payment handshake failed
		fmt.Println("Payment error:", pe.Message)
	case errors.As(err, &be):
		// Request cost would exceed session budget
		fmt.Printf("Over budget: limit=$%.4f spent=$%.4f\n", be.Budget, be.Spent)
	case errors.As(err, &ae):
		// Non-200 HTTP response from gateway
		fmt.Printf("API error %d: %s\n", ae.StatusCode, ae.Message)
	default:
		// Network error, JSON decode error, etc.
		fmt.Println("Error:", err)
	}
}
```

All error types support `Unwrap()` for use with `errors.Is` chains.

## Utility Methods

```go
// List available models (returns raw JSON)
models, err := client.ListModels(context.Background())

// Gateway health check
health, err := client.Health(context.Background())
fmt.Println(health["status"])

// Wallet balance
balance, err := client.GetBalance(context.Background())

// Session spend tracking
fmt.Println(client.GetSessionSpent())

// Cost estimate (not yet implemented)
cost, err := client.GetCostEstimate("openai/gpt-4o", 1000, 500)
```

## Configuration

Options are applied via functional option pattern:

| Option | Default | Description |
|--------|---------|-------------|
| `WithAPIURL(url)` | `https://api.solvela.ai` | Gateway URL (trailing slash trimmed) |
| `WithPrivateKey(key)` | `$SOLANA_WALLET_KEY` | Base58 Solana private key for signing |
| `WithSessionBudget(budget)` | `nil` (unlimited) | Max USDC spend per session |
| `WithTimeout(duration)` | `60s` | HTTP request timeout |
| `WithHTTPClient(client)` | default `http.Client` | Replace the underlying HTTP client |

## API Reference

### `Client`

| Method | Signature | Description |
|--------|-----------|-------------|
| `NewClient` | `NewClient(opts ...Option) (*Client, error)` | Create a new client |
| `Chat` | `Chat(ctx, model, prompt) (string, error)` | One-shot chat, returns text |
| `ChatCompletion` | `ChatCompletion(ctx, ChatRequest) (*ChatResponse, error)` | Full OpenAI-compatible completion |
| `SmartChat` | `SmartChat(ctx, prompt, profile) (*ChatResponse, error)` | Smart-routed chat |
| `ListModels` | `ListModels(ctx) (json.RawMessage, error)` | List available models |
| `Health` | `Health(ctx) (map[string]any, error)` | Gateway health check |
| `GetBalance` | `GetBalance(ctx) (float64, error)` | Wallet USDC balance |
| `GetSessionSpent` | `GetSessionSpent() float64` | Total USDC spent this session |
| `GetSpending` | `GetSpending(ctx) (*SpendSummary, error)` | Session spending summary |
| `GetCostEstimate` | `GetCostEstimate(model, in, out) (*CostBreakdown, error)` | Cost estimate (not yet implemented) |

### Types

- `ChatMessage` -- `Role`, `Content`, `Name`
- `ChatRequest` -- `Model`, `Messages`, `MaxTokens`, `Temperature`, `TopP`, `Stream`
- `ChatResponse` -- `ID`, `Object`, `Created`, `Model`, `Choices`, `Usage`
- `ChatChoice` -- `Index`, `Message`, `FinishReason`
- `Usage` -- `PromptTokens`, `CompletionTokens`, `TotalTokens`
- `SpendSummary` -- `WalletAddress`, `TotalRequests`, `SessionSpentUSDC`, `BudgetRemaining`
- `PaymentRequired` -- 402 response with `X402Version`, `Accepts`, `CostBreakdown`

### Error Types

- `*PaymentError` -- x402 payment processing failure
- `*BudgetExceededError` -- request would exceed session budget (fields: `Budget`, `Spent`, `Cost`)
- `*APIError` -- non-200 gateway response (fields: `StatusCode`, `Message`)

### Constants

| Constant | Value | Description |
|----------|-------|-------------|
| `DefaultAPIURL` | `https://api.solvela.ai` | Production gateway URL |
| `DefaultDevnetURL` | `http://localhost:8402` | Local development URL |
| `DefaultTimeout` | `60` | Default timeout in seconds |
| `USDCMint` | `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v` | USDC SPL token mint address |
| `SolanaNetwork` | `solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp` | Solana mainnet CAIP-2 identifier |
| `X402Version` | `2` | x402 protocol version |

## Testing

```bash
go test ./... -v
```

Tests use `httptest.NewServer` to mock the gateway. No live gateway required.

## License

MIT
