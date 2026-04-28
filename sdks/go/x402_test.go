package solvela

import (
	"crypto/ed25519"
	"encoding/base64"
	"encoding/json"
	"errors"
	"strings"
	"testing"

	"github.com/mr-tron/base58"
)

// TestCreatePaymentHeader_ExactStubModeNoKey verifies the header builder
// returns a stub transaction when no private key is configured.
func TestCreatePaymentHeader_ExactStubModeNoKey(t *testing.T) {
	info := &PaymentRequired{
		X402Version: 2,
		Accepts: []PaymentAccept{{
			Scheme:            "exact",
			Network:           SolanaNetwork,
			Amount:            "1000",
			Asset:             USDCMint,
			PayTo:             "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
			MaxTimeoutSeconds: 300,
		}},
		CostBreakdown: CostBreakdown{Total: "0.001"},
	}

	wallet := NewWallet("") // stub mode
	header, err := createPaymentHeader(info, "/v1/chat/completions", wallet, nil)
	if err != nil {
		t.Fatalf("createPaymentHeader: %v", err)
	}

	decoded, err := base64.StdEncoding.DecodeString(header)
	if err != nil {
		t.Fatalf("base64 decode: %v", err)
	}
	var payload paymentPayload
	if err := json.Unmarshal(decoded, &payload); err != nil {
		t.Fatalf("json decode: %v", err)
	}
	if payload.Payload.Transaction != "STUB_BASE64_TX" {
		t.Errorf("transaction = %q, want STUB_BASE64_TX (stub mode)",
			payload.Payload.Transaction)
	}
}

// TestCreatePaymentHeader_EscrowStubModeNoKey verifies that without a key,
// the escrow scheme returns a stub payload (no longer a hard error).
func TestCreatePaymentHeader_EscrowStubModeNoKey(t *testing.T) {
	info := &PaymentRequired{
		X402Version: 2,
		Accepts: []PaymentAccept{{
			Scheme:            "escrow",
			Network:           SolanaNetwork,
			Amount:            "1000",
			Asset:             USDCMint,
			PayTo:             "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
			MaxTimeoutSeconds: 300,
			EscrowProgramID:   "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
		}},
		CostBreakdown: CostBreakdown{Total: "0.001"},
	}

	wallet := NewWallet("") // stub mode
	header, err := createPaymentHeader(info, "/v1/chat/completions", wallet, []byte(`{"model":"test"}`))
	if err != nil {
		t.Fatalf("createPaymentHeader: %v", err)
	}

	decoded, err := base64.StdEncoding.DecodeString(header)
	if err != nil {
		t.Fatalf("base64 decode: %v", err)
	}
	var payload escrowPaymentPayload
	if err := json.Unmarshal(decoded, &payload); err != nil {
		t.Fatalf("json decode: %v", err)
	}
	if payload.Payload.DepositTx != "STUB_ESCROW_DEPOSIT_TX" {
		t.Errorf("deposit_tx = %q, want STUB_ESCROW_DEPOSIT_TX",
			payload.Payload.DepositTx)
	}
	if payload.Payload.AgentPubkey != "STUB_AGENT_PUBKEY" {
		t.Errorf("agent_pubkey = %q, want STUB_AGENT_PUBKEY",
			payload.Payload.AgentPubkey)
	}
	// service_id is a real base64-encoded 32-byte random value, not a stub.
	sidBytes, err := base64.StdEncoding.DecodeString(payload.Payload.ServiceID)
	if err != nil {
		t.Errorf("service_id base64 decode: %v", err)
	}
	if len(sidBytes) != 32 {
		t.Errorf("service_id length = %d, want 32", len(sidBytes))
	}
}

// TestCreatePaymentHeader_PrefersEscrowWhenBothPresent verifies that when a
// 402 response advertises both exact and escrow, the escrow scheme wins.
func TestCreatePaymentHeader_PrefersEscrowWhenBothPresent(t *testing.T) {
	info := &PaymentRequired{
		X402Version: 2,
		Accepts: []PaymentAccept{
			{
				Scheme:            "exact",
				Network:           SolanaNetwork,
				Amount:            "1000",
				Asset:             USDCMint,
				PayTo:             "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
				MaxTimeoutSeconds: 300,
			},
			{
				Scheme:            "escrow",
				Network:           SolanaNetwork,
				Amount:            "1000",
				Asset:             USDCMint,
				PayTo:             "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
				MaxTimeoutSeconds: 300,
				EscrowProgramID:   "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
			},
		},
		CostBreakdown: CostBreakdown{Total: "0.001"},
	}

	wallet := NewWallet("") // stub mode
	header, err := createPaymentHeader(info, "/v1/chat/completions", wallet, nil)
	if err != nil {
		t.Fatalf("createPaymentHeader: %v", err)
	}
	decoded, _ := base64.StdEncoding.DecodeString(header)
	// Expect an escrow payload shape — look for "deposit_tx" key.
	if !strings.Contains(string(decoded), `"deposit_tx"`) {
		t.Error("expected escrow payload (contains deposit_tx); got exact-shaped header")
	}
}

// TestCreatePaymentHeader_ExactKeyButNoRPC verifies that when a wallet has a
// key but SOLANA_RPC_URL is unset, exact-scheme signing returns a PaymentError
// whose underlying cause is a SigningError mentioning SOLANA_RPC_URL.
func TestCreatePaymentHeader_ExactKeyButNoRPC(t *testing.T) {
	// Deterministic keypair (seed [42;32]) — same pattern as builder tests.
	var seed [32]byte
	for i := range seed {
		seed[i] = 42
	}
	priv := ed25519.NewKeyFromSeed(seed[:])
	keypairB58 := base58.Encode(priv)

	t.Setenv("SOLANA_WALLET_KEY", keypairB58)
	t.Setenv("SOLANA_RPC_URL", "") // explicitly unset
	wallet := NewWallet("")        // picks up SOLANA_WALLET_KEY
	if !wallet.HasKey() {
		t.Fatal("wallet should have key")
	}

	info := &PaymentRequired{
		X402Version: 2,
		Accepts: []PaymentAccept{{
			Scheme:            "exact",
			Network:           SolanaNetwork,
			Amount:            "1000",
			Asset:             USDCMint,
			PayTo:             "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
			MaxTimeoutSeconds: 300,
		}},
		CostBreakdown: CostBreakdown{Total: "0.001"},
	}

	_, err := createPaymentHeader(info, "/v1/chat/completions", wallet, nil)
	if err == nil {
		t.Fatal("expected error when key set but SOLANA_RPC_URL unset")
	}
	pe, ok := err.(*PaymentError)
	if !ok {
		t.Fatalf("expected *PaymentError, got %T", err)
	}
	var se *SigningError
	if !errors.As(pe, &se) {
		t.Fatalf("expected PaymentError to wrap *SigningError, got cause %T", pe.Unwrap())
	}
	if !strings.Contains(se.Error(), "SOLANA_RPC_URL") {
		t.Errorf("expected SigningError to mention SOLANA_RPC_URL, got %q", se.Error())
	}
}

// TestCreatePaymentHeader_EscrowKeyButNoRPC verifies the analogous path for
// the escrow scheme.
func TestCreatePaymentHeader_EscrowKeyButNoRPC(t *testing.T) {
	var seed [32]byte
	for i := range seed {
		seed[i] = 42
	}
	priv := ed25519.NewKeyFromSeed(seed[:])
	keypairB58 := base58.Encode(priv)

	t.Setenv("SOLANA_WALLET_KEY", keypairB58)
	t.Setenv("SOLANA_RPC_URL", "") // explicitly unset
	wallet := NewWallet("")
	if !wallet.HasKey() {
		t.Fatal("wallet should have key")
	}

	info := &PaymentRequired{
		X402Version: 2,
		Accepts: []PaymentAccept{{
			Scheme:            "escrow",
			Network:           SolanaNetwork,
			Amount:            "1000",
			Asset:             USDCMint,
			PayTo:             "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
			MaxTimeoutSeconds: 300,
			EscrowProgramID:   "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
		}},
		CostBreakdown: CostBreakdown{Total: "0.001"},
	}

	_, err := createPaymentHeader(info, "/v1/chat/completions", wallet, []byte(`{}`))
	if err == nil {
		t.Fatal("expected error when escrow key set but SOLANA_RPC_URL unset")
	}
	pe, ok := err.(*PaymentError)
	if !ok {
		t.Fatalf("expected *PaymentError, got %T", err)
	}
	var se *SigningError
	if !errors.As(pe, &se) {
		t.Fatalf("expected PaymentError to wrap *SigningError, got cause %T", pe.Unwrap())
	}
	if !strings.Contains(se.Error(), "SOLANA_RPC_URL") {
		t.Errorf("expected SigningError to mention SOLANA_RPC_URL, got %q", se.Error())
	}
}

// TestGenerateServiceID_Distinct verifies that two calls produce different IDs.
func TestGenerateServiceID_Distinct(t *testing.T) {
	a, err := GenerateServiceID([]byte("body"))
	if err != nil {
		t.Fatalf("GenerateServiceID: %v", err)
	}
	b, err := GenerateServiceID([]byte("body"))
	if err != nil {
		t.Fatalf("GenerateServiceID: %v", err)
	}
	if a == b {
		t.Error("expected distinct service IDs from separate calls")
	}
}

// TestGenerateServiceID_Length is a sanity check.
func TestGenerateServiceID_Length(t *testing.T) {
	id, err := GenerateServiceID(nil)
	if err != nil {
		t.Fatalf("GenerateServiceID: %v", err)
	}
	if len(id) != 32 {
		t.Errorf("len = %d, want 32", len(id))
	}
}

// TestPaymentAcceptEscrowProgramID keeps the JSON-unmarshal test from the old
// file intact — it's pure type-level and still relevant.
func TestPaymentAcceptEscrowProgramID(t *testing.T) {
	jsonStr := `{
		"scheme": "escrow",
		"network": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
		"amount": "5000",
		"asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
		"pay_to": "recipient",
		"max_timeout_seconds": 300,
		"escrow_program_id": "TestEscrowProgram"
	}`
	var accept PaymentAccept
	if err := json.Unmarshal([]byte(jsonStr), &accept); err != nil {
		t.Fatalf("json unmarshal: %v", err)
	}
	if accept.EscrowProgramID != "TestEscrowProgram" {
		t.Errorf("escrow_program_id = %q, want TestEscrowProgram", accept.EscrowProgramID)
	}
}
