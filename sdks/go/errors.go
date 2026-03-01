package rcr

import "fmt"

// PaymentError represents errors during payment processing.
type PaymentError struct {
	Message string
	cause   error
}

func (e *PaymentError) Error() string {
	return fmt.Sprintf("payment error: %s", e.Message)
}

// Unwrap returns the underlying cause, if any.
func (e *PaymentError) Unwrap() error {
	return e.cause
}

// BudgetExceededError is returned when a request would exceed the session budget.
type BudgetExceededError struct {
	Budget float64
	Spent  float64
	Cost   float64
	cause  error
}

func (e *BudgetExceededError) Error() string {
	return fmt.Sprintf("budget exceeded: limit=$%.4f spent=$%.4f cost=$%.4f", e.Budget, e.Spent, e.Cost)
}

// Unwrap returns the underlying cause, if any.
func (e *BudgetExceededError) Unwrap() error {
	return e.cause
}

// APIError represents non-200 responses from the gateway.
type APIError struct {
	StatusCode int
	Message    string
}

func (e *APIError) Error() string {
	return fmt.Sprintf("API error %d: %s", e.StatusCode, e.Message)
}
