package rcr

import (
	"encoding/base64"
	"encoding/json"
	"testing"
)

func TestCreatePaymentHeaderEscrowScheme(t *testing.T) {
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

	header, err := createPaymentHeader(info, "/v1/chat/completions")
	if err != nil {
		t.Fatalf("createPaymentHeader: %v", err)
	}

	decoded, err := base64.StdEncoding.DecodeString(header)
	if err != nil {
		t.Fatalf("base64 decode: %v", err)
	}

	var payload escrowPaymentPayload
	if err := json.Unmarshal(decoded, &payload); err != nil {
		t.Fatalf("json unmarshal: %v", err)
	}

	// Verify x402_version
	if payload.X402Version != 2 {
		t.Errorf("x402_version = %d, want 2", payload.X402Version)
	}

	// Verify resource
	if payload.Resource.URL != "/v1/chat/completions" {
		t.Errorf("resource URL = %q, want /v1/chat/completions", payload.Resource.URL)
	}
	if payload.Resource.Method != "POST" {
		t.Errorf("resource method = %q, want POST", payload.Resource.Method)
	}

	// Verify accepted
	if payload.Accepted.Scheme != "escrow" {
		t.Errorf("accepted scheme = %q, want escrow", payload.Accepted.Scheme)
	}
	if payload.Accepted.EscrowProgramID != "EscProgram123" {
		t.Errorf("accepted escrow_program_id = %q, want EscProgram123", payload.Accepted.EscrowProgramID)
	}

	// Verify payload (escrow stub values)
	if payload.Payload.DepositTx != StubEscrowDepositTx {
		t.Errorf("deposit_tx = %q, want %q", payload.Payload.DepositTx, StubEscrowDepositTx)
	}
	if payload.Payload.ServiceID != StubServiceID {
		t.Errorf("service_id = %q, want %q", payload.Payload.ServiceID, StubServiceID)
	}
	if payload.Payload.AgentPubkey != StubAgentPubkey {
		t.Errorf("agent_pubkey = %q, want %q", payload.Payload.AgentPubkey, StubAgentPubkey)
	}
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
	// Test that escrow scheme is preferred when available
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

	header, err := createPaymentHeader(info, "/v1/chat/completions")
	if err != nil {
		t.Fatalf("createPaymentHeader: %v", err)
	}

	decoded, err := base64.StdEncoding.DecodeString(header)
	if err != nil {
		t.Fatalf("base64 decode: %v", err)
	}

	// Should be escrow payload, not exact
	var payload escrowPaymentPayload
	if err := json.Unmarshal(decoded, &payload); err != nil {
		t.Fatalf("json unmarshal as escrow: %v", err)
	}

	if payload.Accepted.Scheme != "escrow" {
		t.Errorf("selected scheme = %q, want escrow (should prefer escrow)", payload.Accepted.Scheme)
	}
	if payload.Accepted.EscrowProgramID != "EscProgram456" {
		t.Errorf("selected escrow_program_id = %q, want EscProgram456", payload.Accepted.EscrowProgramID)
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
