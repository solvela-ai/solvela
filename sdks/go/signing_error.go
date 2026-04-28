package solvela

import "fmt"

// SigningError represents a failure while building or signing a Solana
// transaction. Distinct from PaymentError so callers can distinguish
// configuration issues (PaymentError — no key, no RPC URL set) from active
// cryptographic or RPC failures (SigningError — bad key, RPC unreachable,
// amount invalid).
type SigningError struct {
	Message string
	cause   error
}

// Error implements the error interface.
func (e *SigningError) Error() string {
	return fmt.Sprintf("signing error: %s", e.Message)
}

// Unwrap returns the underlying cause, if any.
func (e *SigningError) Unwrap() error {
	return e.cause
}
