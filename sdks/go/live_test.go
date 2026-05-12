//go:build live

package solvela_test

import (
	"context"
	"os"
	"testing"
	"time"

	solvela "github.com/solvela-ai/solvela/sdks/go"
)

func TestLiveHealth(t *testing.T) {
	gatewayURL := os.Getenv("SOLVELA_GATEWAY_URL")
	if gatewayURL == "" {
		gatewayURL = "http://localhost:8402"
	}

	transport := solvela.NewTransport(gatewayURL, 10*time.Second)
	models, err := transport.FetchModels(context.Background())
	if err != nil {
		t.Fatalf("fetch models: %v", err)
	}
	if len(models) == 0 {
		t.Error("expected at least one model")
	}
	t.Logf("found %d models", len(models))
}

func TestLiveChat402(t *testing.T) {
	gatewayURL := os.Getenv("SOLVELA_GATEWAY_URL")
	if gatewayURL == "" {
		gatewayURL = "http://localhost:8402"
	}

	wallet, _, err := solvela.CreateWallet()
	if err != nil {
		t.Fatalf("create wallet: %v", err)
	}

	client, err := solvela.NewClient(wallet, nil,
		solvela.WithGatewayURL(gatewayURL),
		solvela.WithTimeout(30*time.Second),
	)
	if err != nil {
		t.Fatalf("new client: %v", err)
	}

	_, err = client.Chat(context.Background(), &solvela.ChatRequest{
		Model: "gpt-4o-mini",
		Messages: []solvela.ChatMessage{
			{Role: solvela.RoleUser, Content: "Say hello in one word."},
		},
	})

	// Expect 402 since we have no payment signer
	if err == nil {
		t.Fatal("expected error (402 payment required)")
	}
	_, ok := err.(*solvela.PaymentRequiredError)
	if !ok {
		t.Logf("got error type %T: %v", err, err)
	}
}
