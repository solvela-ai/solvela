package solvela

import (
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"os"
)

// paymentPayload is the structure encoded into the PAYMENT-SIGNATURE header
// for the exact scheme.
type paymentPayload struct {
	X402Version int             `json:"x402_version"`
	Resource    paymentResource `json:"resource"`
	Accepted    PaymentAccept   `json:"accepted"`
	Payload     solanaPayload   `json:"payload"`
}

// escrowPaymentPayload is the structure encoded into the PAYMENT-SIGNATURE
// header for the escrow scheme.
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

// GenerateServiceID produces a 32-byte correlation ID for an escrow deposit
// from the request body and 8 random bytes. Matches the Python/TS helpers in
// sdks/python/solvela/x402.py and sdks/typescript/src/x402.ts.
func GenerateServiceID(requestBody []byte) ([32]byte, error) {
	var out [32]byte
	var nonce [8]byte
	if _, err := rand.Read(nonce[:]); err != nil {
		return out, err
	}
	h := sha256.New()
	h.Write(requestBody)
	h.Write(nonce[:])
	sum := h.Sum(nil)
	copy(out[:], sum)
	return out, nil
}

// createPaymentHeader builds the base64-encoded PAYMENT-SIGNATURE header
// from a 402 response. It prefers the escrow scheme when available AND the
// wallet has a key; otherwise it falls back to exact.
//
// When wallet has no key (HasKey() == false), a stub transaction value is
// used (development mode — payments will be rejected by a real gateway).
//
// The function takes a *Wallet (not a raw private key string) so the raw
// key never leaves the wallet's type boundary.
//
// On any real-signing failure returns a *PaymentError wrapping the
// underlying *SigningError for inspection via errors.As.
func createPaymentHeader(
	info *PaymentRequired,
	resourceURL string,
	wallet *Wallet,
	requestBody []byte,
) (string, error) {
	if len(info.Accepts) == 0 {
		return "", &PaymentError{Message: "no payment accepts in 402 response"}
	}

	// Prefer escrow scheme if available with a non-empty program id.
	var selected *PaymentAccept
	for i := range info.Accepts {
		if info.Accepts[i].Scheme == "escrow" && info.Accepts[i].EscrowProgramID != "" {
			selected = &info.Accepts[i]
			break
		}
	}
	if selected == nil {
		selected = &info.Accepts[0]
	}

	if selected.Scheme == "escrow" && selected.EscrowProgramID != "" {
		return buildEscrowHeader(selected, resourceURL, wallet, requestBody)
	}
	return buildExactHeader(selected, resourceURL, wallet)
}

// buildExactHeader builds the header for the direct-SPL-transfer path.
func buildExactHeader(
	accept *PaymentAccept,
	resourceURL string,
	wallet *Wallet,
) (string, error) {
	txB64 := "STUB_BASE64_TX"
	if wallet != nil && wallet.HasKey() {
		amount, err := parseAtomicAmount(accept.Amount)
		if err != nil {
			return "", &PaymentError{
				Message: "invalid amount in 402 response: " + err.Error(),
				cause:   err,
			}
		}
		signed, signErr := wallet.signExactPayment(accept.PayTo, amount)
		if signErr != nil {
			return "", &PaymentError{
				Message: "signing exact transfer failed: " + signErr.Error(),
				cause:   signErr,
			}
		}
		txB64 = signed
	}

	payload := paymentPayload{
		X402Version: X402Version,
		Resource:    paymentResource{URL: resourceURL, Method: "POST"},
		Accepted:    *accept,
		Payload:     solanaPayload{Transaction: txB64},
	}
	return marshalPayload(payload)
}

// buildEscrowHeader builds the header for the escrow deposit path.
func buildEscrowHeader(
	accept *PaymentAccept,
	resourceURL string,
	wallet *Wallet,
	requestBody []byte,
) (string, error) {
	serviceID, err := GenerateServiceID(requestBody)
	if err != nil {
		return "", &PaymentError{
			Message: "failed to generate service id: " + err.Error(),
			cause:   err,
		}
	}
	serviceIDB64 := base64.StdEncoding.EncodeToString(serviceID[:])

	depositTx := "STUB_ESCROW_DEPOSIT_TX"
	agentPubkey := "STUB_AGENT_PUBKEY"

	if wallet != nil && wallet.HasKey() {
		amount, amtErr := parseAtomicAmount(accept.Amount)
		if amtErr != nil {
			return "", &PaymentError{
				Message: "invalid escrow amount: " + amtErr.Error(),
				cause:   amtErr,
			}
		}

		rpcURL := os.Getenv("SOLANA_RPC_URL")
		if rpcURL == "" {
			return "", &PaymentError{
				Message: "SOLANA_RPC_URL env var required for escrow signing",
				cause:   &SigningError{Message: "SOLANA_RPC_URL not set"},
			}
		}
		expiry, slotErr := GetExpirySlotFromTimeout(rpcURL, accept.MaxTimeoutSeconds)
		if slotErr != nil {
			return "", &PaymentError{
				Message: "failed to fetch current slot: " + slotErr.Error(),
				cause:   slotErr,
			}
		}

		result, buildErr := wallet.signEscrowDeposit(EscrowDepositParams{
			ProviderWalletB58:  accept.PayTo,
			EscrowProgramIDB58: accept.EscrowProgramID,
			Amount:             amount,
			ServiceID:          serviceID,
			ExpirySlot:         expiry,
		})
		if buildErr != nil {
			return "", &PaymentError{
				Message: "escrow deposit signing failed: " + buildErr.Error(),
				cause:   buildErr,
			}
		}
		depositTx = result.DepositTxB64
		agentPubkey = result.AgentPubkey
	}

	payload := escrowPaymentPayload{
		X402Version: X402Version,
		Resource:    paymentResource{URL: resourceURL, Method: "POST"},
		Accepted:    *accept,
		Payload: escrowPayload{
			DepositTx:   depositTx,
			ServiceID:   serviceIDB64,
			AgentPubkey: agentPubkey,
		},
	}
	return marshalPayload(payload)
}

// marshalPayload JSON-encodes a payload and wraps it in base64.
func marshalPayload(payload any) (string, error) {
	jsonBytes, err := json.Marshal(payload)
	if err != nil {
		return "", err
	}
	return base64.StdEncoding.EncodeToString(jsonBytes), nil
}

// parseAtomicAmount converts a decimal string amount to a uint64 of
// atomic USDC units (6 decimals). Accepts plain integer strings only — the
// 402 response wire format is already in atomic units.
func parseAtomicAmount(s string) (uint64, error) {
	var v uint64
	for _, ch := range s {
		if ch < '0' || ch > '9' {
			return 0, &SigningError{Message: "amount must contain only digits: " + s}
		}
		v = v*10 + uint64(ch-'0')
	}
	return v, nil
}
