package rcr

import "fmt"

// PaymentError represents errors during payment processing.
type PaymentError struct {
	Message string
}

func (e *PaymentError) Error() string {
	return fmt.Sprintf("payment error: %s", e.Message)
}

// BudgetExceededError is returned when a request would exceed the session budget.
type BudgetExceededError struct {
	Budget float64
	Spent  float64
	Cost   float64
}

func (e *BudgetExceededError) Error() string {
	return fmt.Sprintf("budget exceeded: limit=$%.4f spent=$%.4f cost=$%.4f", e.Budget, e.Spent, e.Cost)
}

// APIError represents non-200 responses from the gateway.
type APIError struct {
	StatusCode int
	Message    string
}

func (e *APIError) Error() string {
	return fmt.Sprintf("API error %d: %s", e.StatusCode, e.Message)
}
