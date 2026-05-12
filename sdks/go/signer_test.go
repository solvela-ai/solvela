package solvela

import (
	"context"
	"testing"
)

func TestNewKeypairSignerDefaultRPCURL(t *testing.T) {
	wallet, _, err := CreateWallet()
	if err != nil {
		t.Fatalf("create wallet: %v", err)
	}
	signer := NewKeypairSigner(wallet, "")
	if signer.rpcURL != "https://api.mainnet-beta.solana.com" {
		t.Errorf("rpcURL: got %q, want default mainnet", signer.rpcURL)
	}
}

func TestNewKeypairSignerCustomRPCURL(t *testing.T) {
	wallet, _, err := CreateWallet()
	if err != nil {
		t.Fatalf("create wallet: %v", err)
	}
	signer := NewKeypairSigner(wallet, "https://api.devnet.solana.com")
	if signer.rpcURL != "https://api.devnet.solana.com" {
		t.Errorf("rpcURL: got %q, want devnet", signer.rpcURL)
	}
}

func TestKeypairSignerReturnsStubError(t *testing.T) {
	wallet, _, err := CreateWallet()
	if err != nil {
		t.Fatalf("create wallet: %v", err)
	}
	signer := NewKeypairSigner(wallet, "")

	_, err = signer.SignPayment(
		context.Background(),
		1000,
		"recipient",
		Resource{URL: "/v1/chat/completions", Method: "POST"},
		PaymentAccept{Scheme: "exact", Network: SolanaNetwork, Amount: "1000"},
	)
	if err == nil {
		t.Fatal("expected error from stub signer")
	}
	signerErr, ok := err.(*SignerError)
	if !ok {
		t.Fatalf("expected SignerError, got %T", err)
	}
	if signerErr.Message == "" {
		t.Error("expected non-empty error message")
	}
}
