package rcr

import (
	"encoding/base64"
	"encoding/json"
)

// paymentPayload is the structure encoded into the PAYMENT-SIGNATURE header.
type paymentPayload struct {
	X402Version int             `json:"x402_version"`
	Resource    paymentResource `json:"resource"`
	Accepted    PaymentAccept   `json:"accepted"`
	Payload     solanaPayload   `json:"payload"`
}

type paymentResource struct {
	URL    string `json:"url"`
	Method string `json:"method"`
}

type solanaPayload struct {
	Transaction string `json:"transaction"`
}

// createPaymentHeader builds the base64-encoded payment header from a 402 response.
// In production this would sign a real Solana transaction; the stub puts a
// placeholder transaction so the rest of the flow can be exercised.
func createPaymentHeader(info *PaymentRequired, resourceURL string) (string, error) {
	if len(info.Accepts) == 0 {
		return "", &PaymentError{Message: "no payment accepts in 402 response"}
	}

	payload := paymentPayload{
		X402Version: X402Version,
		Resource:    paymentResource{URL: resourceURL, Method: "POST"},
		Accepted:    info.Accepts[0],
		Payload:     solanaPayload{Transaction: "STUB_BASE64_TX"},
	}

	jsonBytes, err := json.Marshal(payload)
	if err != nil {
		return "", err
	}

	return base64.StdEncoding.EncodeToString(jsonBytes), nil
}
