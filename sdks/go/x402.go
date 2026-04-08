package rcr

import (
	"encoding/base64"
	"encoding/json"
)

// paymentPayload is the structure encoded into the PAYMENT-SIGNATURE header for exact scheme.
type paymentPayload struct {
	X402Version int             `json:"x402_version"`
	Resource    paymentResource `json:"resource"`
	Accepted    PaymentAccept   `json:"accepted"`
	Payload     solanaPayload   `json:"payload"`
}

// escrowPaymentPayload is the structure encoded into the PAYMENT-SIGNATURE header for escrow scheme.
type escrowPaymentPayload struct {
	X402Version int             `json:"x402_version"`
	Resource    paymentResource `json:"resource"`
	Accepted    PaymentAccept   `json:"accepted"`
	Payload     escrowPayload   `json:"payload"`
}

type paymentResource struct {
	URL    string `json:"url"`
	Method string `json:"method"`
}

type solanaPayload struct {
	Transaction string `json:"transaction"`
}

type escrowPayload struct {
	DepositTx   string `json:"deposit_tx"`
	ServiceID   string `json:"service_id"`
	AgentPubkey string `json:"agent_pubkey"`
}

// createPaymentHeader builds the base64-encoded payment header from a 402 response.
// In production this would sign a real Solana transaction; the stub puts a
// placeholder transaction so the rest of the flow can be exercised.
// Prefers escrow scheme if available with non-empty EscrowProgramID.
func createPaymentHeader(info *PaymentRequired, resourceURL string) (string, error) {
	if len(info.Accepts) == 0 {
		return "", &PaymentError{Message: "no payment accepts in 402 response"}
	}

	// Find escrow scheme with non-empty EscrowProgramID
	var selectedAccept *PaymentAccept
	for i := range info.Accepts {
		if info.Accepts[i].Scheme == "escrow" && info.Accepts[i].EscrowProgramID != "" {
			selectedAccept = &info.Accepts[i]
			break
		}
	}

	// Fall back to first accept if no escrow found
	if selectedAccept == nil {
		selectedAccept = &info.Accepts[0]
	}

	// Escrow signing is not implemented in the Go SDK — return a clear error
	// rather than silently producing a stub payload that will be rejected.
	if selectedAccept.Scheme == "escrow" && selectedAccept.EscrowProgramID != "" {
		return "", &PaymentError{
			Message: "escrow deposit signing is not yet implemented in the Go SDK; " +
				"use the Python, TypeScript, or Rust CLI for escrow payments",
		}
	}

	// Exact scheme (default)
	payload := paymentPayload{
		X402Version: X402Version,
		Resource:    paymentResource{URL: resourceURL, Method: "POST"},
		Accepted:    *selectedAccept,
		Payload:     solanaPayload{Transaction: "STUB_BASE64_TX"},
	}

	jsonBytes, err := json.Marshal(payload)
	if err != nil {
		return "", err
	}

	return base64.StdEncoding.EncodeToString(jsonBytes), nil
}
