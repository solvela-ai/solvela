package rcr

import (
	"encoding/base64"
	"encoding/json"
	"testing"
)

func TestCreatePaymentHeaderEscrowScheme(t *testing.T) {
	// Escrow signing is not implemented in the Go SDK — expect an error.
	info := &PaymentRequired{
		X402Version: 2,
		Accepts: []PaymentAccept{{
			Scheme:            "escrow",
			Network:           SolanaNetwork,
			Amount:            "1000",
			Asset:             USDCMint,
			PayTo:             "recipient",
			MaxTimeoutSeconds: 300,
			EscrowProgramID:   "EscProgram123",
		}},
		CostBreakdown: CostBreakdown{Total: "0.001"},
	}

	_, err := createPaymentHeader(info, "/v1/chat/completions")
	if err == nil {
		t.Fatal("createPaymentHeader: expected error for escrow scheme, got nil")
	}

	payErr, ok := err.(*PaymentError)
	if !ok {
		t.Fatalf("expected *PaymentError, got %T: %v", err, err)
	}

	const wantSubstr = "escrow deposit signing is not yet implemented in the Go SDK"
	if payErr.Message == "" || len(payErr.Message) < len(wantSubstr) {
		t.Errorf("error message too short: %q", payErr.Message)
	}
	if !containsSubstr(payErr.Message, wantSubstr) {
		t.Errorf("error message = %q, want it to contain %q", payErr.Message, wantSubstr)
	}
}

// containsSubstr is a simple substring check to avoid importing strings in test.
func containsSubstr(s, sub string) bool {
	return len(s) >= len(sub) && (s == sub || len(sub) == 0 ||
		func() bool {
			for i := 0; i <= len(s)-len(sub); i++ {
				if s[i:i+len(sub)] == sub {
					return true
				}
			}
			return false
		}())
}

func TestPaymentAcceptEscrowProgramID(t *testing.T) {
	// Test JSON unmarshaling of escrow_program_id field
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

	if accept.Scheme != "escrow" {
		t.Errorf("scheme = %q, want escrow", accept.Scheme)
	}
	if accept.EscrowProgramID != "TestEscrowProgram" {
		t.Errorf("escrow_program_id = %q, want TestEscrowProgram", accept.EscrowProgramID)
	}
	if accept.Amount != "5000" {
		t.Errorf("amount = %q, want 5000", accept.Amount)
	}
}

func TestCreatePaymentHeaderPrefersEscrow(t *testing.T) {
	// Escrow is preferred when available, but the Go SDK returns an error for
	// escrow scheme since deposit signing is not implemented.
	info := &PaymentRequired{
		X402Version: 2,
		Accepts: []PaymentAccept{
			{
				Scheme:            "exact",
				Network:           SolanaNetwork,
				Amount:            "1000",
				Asset:             USDCMint,
				PayTo:             "recipient1",
				MaxTimeoutSeconds: 300,
			},
			{
				Scheme:            "escrow",
				Network:           SolanaNetwork,
				Amount:            "1000",
				Asset:             USDCMint,
				PayTo:             "recipient2",
				MaxTimeoutSeconds: 300,
				EscrowProgramID:   "EscProgram456",
			},
		},
		CostBreakdown: CostBreakdown{Total: "0.001"},
	}

	_, err := createPaymentHeader(info, "/v1/chat/completions")
	if err == nil {
		t.Fatal("createPaymentHeader: expected error when escrow scheme selected, got nil")
	}

	payErr, ok := err.(*PaymentError)
	if !ok {
		t.Fatalf("expected *PaymentError, got %T: %v", err, err)
	}
	if !containsSubstr(payErr.Message, "escrow deposit signing is not yet implemented") {
		t.Errorf("unexpected error message: %q", payErr.Message)
	}
}

func TestCreatePaymentHeaderExactFallback(t *testing.T) {
	// Test that exact scheme is used when escrow not available
	info := &PaymentRequired{
		X402Version: 2,
		Accepts: []PaymentAccept{
			{
				Scheme:            "exact",
				Network:           SolanaNetwork,
				Amount:            "1000",
				Asset:             USDCMint,
				PayTo:             "recipient",
				MaxTimeoutSeconds: 300,
			},
		},
		CostBreakdown: CostBreakdown{Total: "0.001"},
	}

	header, err := createPaymentHeader(info, "/v1/chat/completions")
	if err != nil {
		t.Fatalf("createPaymentHeader: %v", err)
	}

	decoded, err := base64.StdEncoding.DecodeString(header)
	if err != nil {
		t.Fatalf("base64 decode: %v", err)
	}

	var payload paymentPayload
	if err := json.Unmarshal(decoded, &payload); err != nil {
		t.Fatalf("json unmarshal as exact: %v", err)
	}

	if payload.Accepted.Scheme != "exact" {
		t.Errorf("selected scheme = %q, want exact", payload.Accepted.Scheme)
	}
	if payload.Payload.Transaction != "STUB_BASE64_TX" {
		t.Errorf("transaction = %q, want STUB_BASE64_TX", payload.Payload.Transaction)
	}
}

func TestEscrowPayloadMarshal(t *testing.T) {
	// Test that escrowPaymentPayload marshals correctly
	payload := escrowPaymentPayload{
		X402Version: 2,
		Resource: paymentResource{
			URL:    "/v1/chat/completions",
			Method: "POST",
		},
		Accepted: PaymentAccept{
			Scheme:            "escrow",
			Network:           SolanaNetwork,
			Amount:            "1000",
			Asset:             USDCMint,
			PayTo:             "recipient",
			MaxTimeoutSeconds: 300,
			EscrowProgramID:   "EscProgram789",
		},
		Payload: escrowPayload{
			DepositTx:   StubEscrowDepositTx,
			ServiceID:   StubServiceID,
			AgentPubkey: StubAgentPubkey,
		},
	}

	jsonBytes, err := json.Marshal(payload)
	if err != nil {
		t.Fatalf("json marshal: %v", err)
	}

	// Verify round-trip
	var decoded escrowPaymentPayload
	if err := json.Unmarshal(jsonBytes, &decoded); err != nil {
		t.Fatalf("json unmarshal: %v", err)
	}

	if decoded.Accepted.EscrowProgramID != "EscProgram789" {
		t.Errorf("round-trip escrow_program_id = %q, want EscProgram789", decoded.Accepted.EscrowProgramID)
	}
	if decoded.Payload.DepositTx != StubEscrowDepositTx {
		t.Errorf("round-trip deposit_tx = %q, want %q", decoded.Payload.DepositTx, StubEscrowDepositTx)
	}
}
