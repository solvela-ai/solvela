// Manual smoke test — exercises the real wire contract before release.
//
// Run before tagging a release to verify the SDK still agrees with a live
// Solvela gateway. Catches wire-format drift that unit/integration tests
// cannot (header names, JSON field renames, accepts-array ordering, new
// required fields).
//
// Usage:
//
//	SOLVELA_GATEWAY_URL=https://staging.solvela.ai \
//	go run ./scripts/smoke
//
// Defaults to http://localhost:8402 if SOLVELA_GATEWAY_URL is unset.
//
// Exit codes:
//
//	0 — all assertions passed
//	1 — an assertion failed or the gateway is unreachable
package main

import (
	"context"
	"errors"
	"fmt"
	"os"
	"regexp"
	"strconv"

	solvela "github.com/solvela-ai/solvela/sdks/go"
)

// solanaPubkeyRE matches Solana base58 pubkeys (32–44 chars over the Bitcoin
// base58 alphabet; no 0/O/I/l). An empty or malformed PayTo would route
// signed funds to address-zero — the most expensive class of silent drift.
var solanaPubkeyRE = regexp.MustCompile(`^[1-9A-HJ-NP-Za-km-z]{32,44}$`)

// amountRE matches positive decimal-integer strings (no leading sign, no
// decimals, no scientific notation).
var amountRE = regexp.MustCompile(`^\d+$`)

func main() {
	os.Exit(run())
}

func run() int {
	gatewayURL := os.Getenv("SOLVELA_GATEWAY_URL")
	if gatewayURL == "" {
		gatewayURL = "http://localhost:8402"
	}
	fmt.Printf("Smoke test against: %s\n\n", gatewayURL)

	// No wallet, no signer: smoke probes the gateway's unsigned-request paths
	// (models registry + 402 negotiation), not payment settlement.
	client, err := solvela.NewClient(nil, nil, solvela.WithGatewayURL(gatewayURL))
	if err != nil {
		fmt.Printf("FAIL: client construction rejected gateway URL: %v\n", err)
		return 1
	}
	defer client.Close()

	ctx := context.Background()

	// 1. Models() reaches the gateway and parses the response.
	models, err := client.Models(ctx)
	if err != nil {
		fmt.Printf("FAIL: Models() returned error: %v\n", err)
		return 1
	}
	fmt.Printf("  Models()           -> %d model(s) returned\n", len(models))
	if len(models) == 0 {
		fmt.Println("FAIL: expected at least one model from the gateway")
		return 1
	}
	sample := models[0]
	fmt.Printf("  sample model       -> id=%q ctx=%d in=%g/M out=%g/M\n",
		sample.ID, sample.ContextWindow, sample.InputUsdcPerMillion, sample.OutputUsdcPerMillion)

	// Guard against silent ModelInfo wire drift: every prior shape change
	// surfaced as all-zero pricing / all-false capabilities. Assert at least
	// one model in the registry exposes streaming and paid input pricing.
	anyStreaming := false
	anyPriced := false
	for _, m := range models {
		if m.SupportsStreaming {
			anyStreaming = true
		}
		if m.InputUsdcPerMillion > 0 {
			anyPriced = true
		}
	}
	if !anyStreaming {
		fmt.Println("FAIL: no model reports SupportsStreaming=true — capabilities parsing may be drifted")
		return 1
	}
	if !anyPriced {
		fmt.Println("FAIL: no model reports InputUsdcPerMillion > 0 — pricing parsing may be drifted")
		return 1
	}
	if sample.DisplayName == "" || sample.Provider == "" {
		fmt.Println("FAIL: sample model missing DisplayName or Provider")
		return 1
	}

	// 2. Unsigned chat returns 402 with a parseable PaymentRequired body.
	req := &solvela.ChatRequest{
		Model: sample.ID,
		Messages: []solvela.ChatMessage{
			{Role: solvela.RoleUser, Content: "ping"},
		},
	}
	_, err = client.Chat(ctx, req)
	if err == nil {
		fmt.Println("FAIL: Chat() returned a response with no signer configured (expected 402)")
		return 1
	}
	var prErr *solvela.PaymentRequiredError
	if !errors.As(err, &prErr) {
		fmt.Printf("FAIL: Chat() raised %T (expected *PaymentRequiredError): %v\n", err, err)
		return 1
	}
	pr := prErr.PaymentRequired
	schemes := make([]string, 0, len(pr.Accepts))
	for _, a := range pr.Accepts {
		schemes = append(schemes, string(a.Scheme))
	}
	fmt.Printf("  Chat() unsigned    -> 402 OK (total=%s %s, schemes=%v)\n",
		pr.CostBreakdown.Total, pr.CostBreakdown.Currency, schemes)
	if len(pr.Accepts) == 0 {
		fmt.Println("FAIL: 402 response had empty accepts array")
		return 1
	}

	// Critical drift checks — a silent regression in any of these would
	// route real funds wrong. All six are derivable from the unsigned 402
	// we just received, so no extra gateway round-trip is required.
	accept := pr.Accepts[0]
	if accept.PayTo == "" || !solanaPubkeyRE.MatchString(accept.PayTo) {
		fmt.Printf("FAIL: accepts[0].PayTo invalid: %q\n", truncate(accept.PayTo, 64))
		return 1
	}
	if !amountRE.MatchString(accept.Amount) {
		fmt.Printf("FAIL: accepts[0].Amount must be a positive decimal-integer string: %q\n",
			truncate(accept.Amount, 32))
		return 1
	}
	amt, err := strconv.ParseUint(accept.Amount, 10, 64)
	if err != nil || amt == 0 {
		fmt.Printf("FAIL: accepts[0].Amount must parse as a positive uint: %q\n",
			truncate(accept.Amount, 32))
		return 1
	}
	if accept.Network != solvela.SolanaNetwork {
		fmt.Printf("FAIL: accepts[0].Network=%q (expected %q)\n",
			accept.Network, solvela.SolanaNetwork)
		return 1
	}
	if accept.Asset != solvela.USDCMint {
		fmt.Printf("FAIL: accepts[0].Asset=%q (expected %q)\n",
			accept.Asset, solvela.USDCMint)
		return 1
	}
	if pr.CostBreakdown.Currency != "USDC" {
		fmt.Printf("FAIL: cost_breakdown.currency=%q (expected %q)\n",
			pr.CostBreakdown.Currency, "USDC")
		return 1
	}
	if pr.X402Version != solvela.X402Version {
		fmt.Printf("FAIL: x402_version=%d (SDK expects %d)\n",
			pr.X402Version, solvela.X402Version)
		return 1
	}
	fmt.Printf("  critical checks    -> PayTo OK Amount OK Network OK Asset OK Currency OK x402_version=%d OK\n",
		solvela.X402Version)

	fmt.Println("\nSmoke test PASSED")
	return 0
}

// truncate returns s clipped to at most n runes, used only for printing
// invalid wire fields without flooding stderr on garbage input.
func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n]
}
