package solvela

import "fmt"

// ClientError represents a general client error.
type ClientError struct {
	Message string
}

func (e *ClientError) Error() string { return e.Message }

// WalletError represents a wallet-related error.
type WalletError struct {
	Message string
}

func (e *WalletError) Error() string { return fmt.Sprintf("wallet error: %s", e.Message) }

// SignerError represents a signing-related error.
type SignerError struct {
	Message string
}

func (e *SignerError) Error() string { return fmt.Sprintf("signer error: %s", e.Message) }

// InsufficientBalanceError indicates the wallet does not have enough funds.
type InsufficientBalanceError struct {
	Have, Need uint64
}

func (e *InsufficientBalanceError) Error() string {
	return fmt.Sprintf("insufficient balance: have %d, need %d", e.Have, e.Need)
}

// GatewayError represents an HTTP error from the gateway.
type GatewayError struct {
	Status  int
	Message string
}

func (e *GatewayError) Error() string {
	return fmt.Sprintf("gateway error %d: %s", e.Status, e.Message)
}

// PaymentRequiredError wraps a 402 response.
type PaymentRequiredError struct {
	PaymentRequired PaymentRequired
}

func (e *PaymentRequiredError) Error() string {
	return fmt.Sprintf("payment required: %s USDC", e.PaymentRequired.CostBreakdown.Total)
}

// PaymentRejectedError indicates the payment was not accepted.
type PaymentRejectedError struct {
	Reason string
}

func (e *PaymentRejectedError) Error() string {
	return fmt.Sprintf("payment rejected: %s", e.Reason)
}

// RecipientMismatchError indicates the payment recipient does not match expectations.
type RecipientMismatchError struct {
	Expected, Actual string
}

func (e *RecipientMismatchError) Error() string {
	return fmt.Sprintf("recipient mismatch: expected %s, got %s", e.Expected, e.Actual)
}

// AmountExceedsMaxError indicates the payment amount exceeds the configured maximum.
type AmountExceedsMaxError struct {
	Amount, MaxAmount uint64
}

func (e *AmountExceedsMaxError) Error() string {
	return fmt.Sprintf("amount %d exceeds max %d", e.Amount, e.MaxAmount)
}

// TimeoutError indicates a request timed out.
type TimeoutError struct {
	TimeoutSecs float64
}

func (e *TimeoutError) Error() string {
	return fmt.Sprintf("request timed out after %.1fs", e.TimeoutSecs)
}

// QualityDegradedError is returned by [SolvelaClient.Chat] when a response
// repeatedly fails the quality check (after MaxQualityRetries) and the
// caller cannot get a clean response. The Response field is the last
// (still-degraded) response, available for callers that prefer to use it
// anyway. Degraded responses are never cached.
type QualityDegradedError struct {
	Reason   DegradedReason
	Response *ChatResponse
}

func (e *QualityDegradedError) Error() string {
	return fmt.Sprintf("response quality degraded after retries: %s", e.Reason)
}
