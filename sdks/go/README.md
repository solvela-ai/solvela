# solvela-go

Go SDK for [Solvela](https://solvela.ai) — a Solana-native AI agent payment gateway.

AI agents pay for LLM API calls with USDC-SPL on Solana via the x402 protocol. No API keys, no accounts, just wallets.

## Install

```bash
go get github.com/solvela-ai/solvela/sdks/go
```

## Quick Start

```go
package main

import (
	"context"
	"fmt"
	"log"

	solvela "github.com/solvela-ai/solvela/sdks/go"
)

func main() {
	wallet, _, err := solvela.CreateWallet()
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println("Wallet:", wallet.Address())

	client, err := solvela.NewClient(wallet, nil,
		solvela.WithGatewayURL("https://api.solvela.ai"),
		solvela.WithCache(true),
		solvela.WithSessions(true),
	)
	if err != nil {
		log.Fatal(err)
	}

	resp, err := client.Chat(context.Background(), &solvela.ChatRequest{
		Model: "gpt-4o-mini",
		Messages: []solvela.ChatMessage{
			{Role: solvela.RoleUser, Content: "Hello!"},
		},
	})
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println(resp.Choices[0].Message.Content)
}
```

## Status

The core SDK (transport, caching, sessions, quality checking, streaming, balance monitoring) is fully implemented and tested.

**`KeypairSigner` is the bundled signer.** It builds real Solana transactions, branching on the x402 payment scheme:

- **`escrow`** — fully implemented. `SignPayment` builds a byte-exact escrow `deposit` transaction (golden-vector-pinned against the canonical Rust builder `crates/escrow-tx`), generating a per-request CSPRNG `service_id`, computing the expiry slot from `getSlot`, and fetching a recent blockhash. This is the trustless overpayment-protection path for a single metered request.
- **`exact`** — not yet implemented in the Go SDK. `SignPayment` returns a clear error (it never silently substitutes another scheme). The byte layout has no pinned golden vector, so it is intentionally left unimplemented rather than guessed at on the money path.

When the gateway advertises both `exact` and `escrow` (the default), `KeypairSigner` reports — via the optional `SchemeCapable` interface — that it can only sign `escrow`, so scheme selection prefers `escrow` rather than auto-selecting the unsignable first-listed `exact`. There is no silent substitution: if no compatible, signable scheme is offered, the client surfaces a `PaymentRequiredError`.

To make `exact`-scheme payments today you have two options:

1. **Use the Rust SDK** — the [Rust SDK](https://github.com/solvela-ai/solvela/tree/main/sdks/rust) is the canonical reference implementation and includes a working exact-scheme signer.
2. **Implement a custom `Signer`** — the `Signer` interface is pluggable. Provide your own implementation using `crypto/ed25519` (already in the Go standard library) and a Solana JSON-RPC client of your choice. Optionally implement `SchemeCapable` so scheme selection knows which schemes your signer supports.

```go
type MySigner struct{ wallet *solvela.Wallet }

func (s *MySigner) SignPayment(ctx context.Context, amount uint64, recipient string, resource solvela.Resource, accepted solvela.PaymentAccept) (*solvela.PaymentPayload, error) {
    // build and sign a USDC-SPL transfer transaction, return PaymentPayload
}

client, err := solvela.NewClient(wallet, &MySigner{wallet: wallet}, ...)
```

## Features

- Automatic x402 payment flow (402 detection, signing, retry)
- Response caching with LRU eviction and dedup window
- Session tracking with three-strike model escalation
- Quality checking with automatic retry on degraded responses
- SSE streaming support
- Balance monitoring with low-balance callbacks
- Pluggable `Signer` interface for custom payment signing

## Configuration

The default per-request payment cap is **10 USDC** (`10_000_000` atomic units). `WithMaxPaymentAmount` is only needed when a caller explicitly requires a higher cap.

```go
client, err := solvela.NewClient(wallet, signer,
	solvela.WithGatewayURL("https://api.solvela.ai"),
	solvela.WithTimeout(60 * time.Second),
	solvela.WithCache(true),
	solvela.WithSessions(true),
	solvela.WithQualityCheck(true),
	solvela.WithMaxQualityRetries(2),
	solvela.WithExpectedRecipient("expected-wallet-address"),
	solvela.WithMaxPaymentAmount(10_000_000), // atomic USDC units (10 USDC; the default cap)
	solvela.WithFreeFallbackModel("gpt-4o-mini"),
)
if err != nil {
	log.Fatal(err)
}
```

## Breaking Changes (security hardening)

- `NewClient` now returns `(*SolvelaClient, error)`. Callers must check the
  error; it is non-nil when the configured gateway URL is invalid (for
  example, a plaintext `http://` URL pointing at a non-loopback host).
- The default `GatewayURL` is now `https://api.solvela.ai`. Plaintext
  `http://` URLs are only accepted for `localhost`, `127.0.0.1`, and `::1`.
- `MaxPaymentAmount` now defaults to **10 USDC atomic units**
  (`10_000_000`). Callers that legitimately need higher per-request payments
  must override this via `WithMaxPaymentAmount(...)`. A `nil`
  `MaxPaymentAmount` is rejected at request time as a misconfiguration.
- `Wallet.PrivateKey()` has been removed. Use `Wallet.Sign(msg)` for
  signing operations, or `Wallet.ToKeypairBytes()` /
  `Wallet.ToKeypairB58()` for persistence (clearly marked secret).
- `ChatStream` now performs the x402 402-handshake before opening the
  stream, matching `Chat`'s behavior. A `Signer` is therefore required for
  paid streaming; when none is configured the call returns
  `*PaymentRequiredError`.

## Testing

```bash
go test ./... -v -count=1

# Live tests (requires running gateway)
go test ./... -v -tags=live
```

## Documentation

[docs.solvela.ai/sdks/go](https://docs.solvela.ai/sdks/go) — full reference, context patterns, custom Signer guide.

## License

Apache-2.0
