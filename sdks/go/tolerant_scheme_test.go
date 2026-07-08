package solvela

// Slice B (channel-on-A2A plan §7): tolerant channel-scheme parsing.
//
// A 402 accepts[] entry carrying a scheme this SDK does not implement (e.g. a
// future "channel" advert) must not poison the whole body parse
// (skip-or-represent), while scheme SELECTION stays fail-closed — an unknown
// scheme is never silently picked, and a body offering ONLY unknown schemes
// fails selection loudly.

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

// schemeAgnosticSigner implements Signer but NOT SchemeCapable, so selection
// treats it as able to sign any known scheme — the gateway-preferred `exact`
// entry is chosen, matching the pre-channel status quo these tests pin.
type schemeAgnosticSigner struct{}

func (schemeAgnosticSigner) SignPayment(_ context.Context, _ uint64, _ string, resource Resource, accepted PaymentAccept) (*PaymentPayload, error) {
	return &PaymentPayload{
		X402Version: X402Version,
		Resource:    resource,
		Accepted:    accepted,
		Payload:     SolanaPayload{Transaction: "fakeTx=="},
	}, nil
}

func tolerant402Body(t *testing.T, schemes ...string) []byte {
	t.Helper()
	entries := make([]map[string]any, 0, len(schemes))
	for _, s := range schemes {
		e := map[string]any{
			"scheme":              s,
			"network":             SolanaNetwork,
			"amount":              "1000",
			"asset":               USDCMint,
			"pay_to":              "recipient123",
			"max_timeout_seconds": 300,
		}
		if s == "channel" {
			// A future channel advert need not carry the exact/escrow field
			// set — tolerance must not depend on the entry's shape.
			delete(e, "max_timeout_seconds")
			e["channel_id"] = "Chan111111111111111111111111111111111111111"
		}
		entries = append(entries, e)
	}
	body, err := json.Marshal(map[string]any{
		"x402_version": X402Version,
		"resource":     map[string]any{"url": "/v1/chat/completions", "method": "POST"},
		"accepts":      entries,
		"cost_breakdown": map[string]any{
			"provider_cost": "950",
			"platform_fee":  "50",
			"total":         "1000",
			"currency":      "USDC",
			"fee_percent":   5,
		},
		"error": "Payment required",
	})
	if err != nil {
		t.Fatalf("marshal 402 body: %v", err)
	}
	return body
}

// TestAcceptsWithChannelEntryParsesAndSelectsExact is the plan-mandated Slice
// B contract: a 402 offering BOTH a channel entry and an exact entry parses,
// and selection picks the same exact entry it would have picked without the
// channel entry present.
func TestAcceptsWithChannelEntryParsesAndSelectsExact(t *testing.T) {
	var pr PaymentRequired
	if err := json.Unmarshal(tolerant402Body(t, "channel", "exact"), &pr); err != nil {
		t.Fatalf("a channel accepts[] entry must not poison the 402 parse: %v", err)
	}
	if len(pr.Accepts) != 1 || pr.Accepts[0].Scheme != SchemeExact {
		t.Fatalf("typed accepts must hold exactly the exact entry, got %+v", pr.Accepts)
	}
	if len(pr.UnrecognizedSchemes) != 1 || pr.UnrecognizedSchemes[0] != "channel" {
		t.Fatalf("skipped schemes must be represented, got %v", pr.UnrecognizedSchemes)
	}

	wallet, _, err := CreateWallet()
	if err != nil {
		t.Fatalf("wallet: %v", err)
	}
	client, err := NewClient(wallet, schemeAgnosticSigner{}, WithGatewayURL("https://gw.test.local"))
	if err != nil {
		t.Fatalf("client: %v", err)
	}
	chosen := client.findCompatibleScheme(&pr)
	if chosen == nil {
		t.Fatal("selection must still find the exact entry")
	}
	if chosen.Scheme != SchemeExact {
		t.Fatalf("scheme: got %q, want exact (skip must never become select)", chosen.Scheme)
	}
}

// TestAcceptsWithOnlyUnknownSchemesFailsSelectionLoudly drives the real chat
// path: the body parses (no crash), but with no recognized entry left the
// client surfaces PaymentRequiredError — it never silently signs against an
// unknown scheme.
func TestAcceptsWithOnlyUnknownSchemesFailsSelectionLoudly(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(402)
		_, _ = w.Write(tolerant402Body(t, "channel"))
	}))
	defer server.Close()

	wallet, _, err := CreateWallet()
	if err != nil {
		t.Fatalf("wallet: %v", err)
	}
	client, err := NewClient(wallet, schemeAgnosticSigner{}, WithGatewayURL(server.URL))
	if err != nil {
		t.Fatalf("client: %v", err)
	}
	_, err = client.Chat(context.Background(), &ChatRequest{
		Model:    "gpt-4",
		Messages: []ChatMessage{{Role: RoleUser, Content: "Hi"}},
	})
	if err == nil {
		t.Fatal("a channel-only 402 must fail loudly, never sign silently")
	}
	if _, ok := err.(*PaymentRequiredError); !ok {
		t.Fatalf("expected PaymentRequiredError (fail closed), got %T: %v", err, err)
	}
}
