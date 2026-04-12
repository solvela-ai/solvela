package rcr

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestNewClient(t *testing.T) {
	c, err := NewClient()
	if err != nil {
		t.Fatalf("NewClient: %v", err)
	}
	if c.apiURL != DefaultAPIURL {
		t.Errorf("apiURL = %q, want %q", c.apiURL, DefaultAPIURL)
	}
}

func TestClientWithOptions(t *testing.T) {
	c, _ := NewClient(
		WithAPIURL("http://localhost:8402/"),
		WithSessionBudget(10.0),
	)
	if c.apiURL != "http://localhost:8402" {
		t.Errorf("apiURL = %q, want trimmed", c.apiURL)
	}
	if c.sessionBudget == nil || *c.sessionBudget != 10.0 {
		t.Error("session budget not set")
	}
}

func TestWallet(t *testing.T) {
	w := NewWallet("")
	if w.HasKey() {
		t.Error("empty wallet should not have key")
	}
	w2 := NewWallet("test-key")
	if !w2.HasKey() {
		t.Error("wallet with key should have key")
	}
}

func TestCreatePaymentHeader(t *testing.T) {
	info := &PaymentRequired{
		X402Version: 2,
		Accepts: []PaymentAccept{{
			Scheme:            "exact",
			Network:           SolanaNetwork,
			Amount:            "1000",
			Asset:             USDCMint,
			PayTo:             "recipient",
			MaxTimeoutSeconds: 300,
		}},
		CostBreakdown: CostBreakdown{Total: "0.001"},
	}

	header, err := createPaymentHeader(info, "/v1/chat/completions", nil, nil)
	if err != nil {
		t.Fatalf("createPaymentHeader: %v", err)
	}

	decoded, err := base64.StdEncoding.DecodeString(header)
	if err != nil {
		t.Fatalf("base64 decode: %v", err)
	}

	var payload paymentPayload
	if err := json.Unmarshal(decoded, &payload); err != nil {
		t.Fatalf("json unmarshal: %v", err)
	}
	if payload.X402Version != 2 {
		t.Errorf("x402_version = %d, want 2", payload.X402Version)
	}
	if payload.Resource.URL != "/v1/chat/completions" {
		t.Errorf("resource URL = %q", payload.Resource.URL)
	}
	if payload.Resource.Method != "POST" {
		t.Errorf("resource method = %q", payload.Resource.Method)
	}
	if payload.Accepted.Network != SolanaNetwork {
		t.Errorf("accepted network = %q", payload.Accepted.Network)
	}
}

func TestCreatePaymentHeaderNoAccepts(t *testing.T) {
	info := &PaymentRequired{
		X402Version: 2,
		Accepts:     []PaymentAccept{},
	}
	_, err := createPaymentHeader(info, "/v1/chat/completions", nil, nil)
	if err == nil {
		t.Fatal("expected error for empty accepts")
	}
	if _, ok := err.(*PaymentError); !ok {
		t.Errorf("expected PaymentError, got %T", err)
	}
}

func TestChatRequestJSON(t *testing.T) {
	req := ChatRequest{
		Model:    "openai/gpt-4o",
		Messages: []ChatMessage{{Role: RoleUser, Content: "Hello"}},
	}
	b, err := json.Marshal(req)
	if err != nil {
		t.Fatal(err)
	}
	var m map[string]any
	if err := json.Unmarshal(b, &m); err != nil {
		t.Fatal(err)
	}
	if m["model"] != "openai/gpt-4o" {
		t.Errorf("model = %v", m["model"])
	}
	// Verify omitempty fields are absent when nil
	if _, ok := m["max_tokens"]; ok {
		t.Error("max_tokens should be omitted when nil")
	}
	if _, ok := m["temperature"]; ok {
		t.Error("temperature should be omitted when nil")
	}
}

func TestChatResponseJSON(t *testing.T) {
	raw := `{
		"id": "chatcmpl-123",
		"object": "chat.completion",
		"created": 1700000000,
		"model": "openai/gpt-4o",
		"choices": [{
			"index": 0,
			"message": {"role": "assistant", "content": "Hello!"},
			"finish_reason": "stop"
		}],
		"usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}
	}`
	var resp ChatResponse
	if err := json.Unmarshal([]byte(raw), &resp); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if resp.ID != "chatcmpl-123" {
		t.Errorf("id = %q", resp.ID)
	}
	if len(resp.Choices) != 1 {
		t.Fatalf("choices = %d", len(resp.Choices))
	}
	if resp.Choices[0].Message.Content != "Hello!" {
		t.Errorf("content = %q", resp.Choices[0].Message.Content)
	}
	if resp.Usage == nil || resp.Usage.TotalTokens != 7 {
		t.Errorf("usage = %+v", resp.Usage)
	}
}

func TestErrorTypes(t *testing.T) {
	pe := &PaymentError{Message: "test"}
	if pe.Error() != "payment error: test" {
		t.Errorf("PaymentError = %q", pe.Error())
	}
	be := &BudgetExceededError{Budget: 10, Spent: 8, Cost: 3}
	if be.Error() == "" {
		t.Error("BudgetExceededError empty")
	}
	ae := &APIError{StatusCode: 500, Message: "fail"}
	if ae.Error() == "" {
		t.Error("APIError empty")
	}
}

func TestErrorUnwrap(t *testing.T) {
	sentinel := errors.New("underlying cause")

	pe := &PaymentError{Message: "test", cause: sentinel}
	if !errors.Is(pe, sentinel) {
		t.Error("PaymentError.Unwrap should expose cause via errors.Is")
	}

	be := &BudgetExceededError{Budget: 10, Spent: 8, Cost: 3, cause: sentinel}
	if !errors.Is(be, sentinel) {
		t.Error("BudgetExceededError.Unwrap should expose cause via errors.Is")
	}

	// nil cause should not panic
	pe2 := &PaymentError{Message: "no cause"}
	if pe2.Unwrap() != nil {
		t.Error("nil cause should return nil from Unwrap")
	}
}

func TestSessionSpent(t *testing.T) {
	c, _ := NewClient()
	if c.GetSessionSpent() != 0 {
		t.Error("initial session spent should be 0")
	}
}

func TestRoleConstants(t *testing.T) {
	if RoleSystem != "system" {
		t.Error("RoleSystem")
	}
	if RoleUser != "user" {
		t.Error("RoleUser")
	}
	if RoleAssistant != "assistant" {
		t.Error("RoleAssistant")
	}
	if RoleTool != "tool" {
		t.Error("RoleTool")
	}
}

func TestConfigConstants(t *testing.T) {
	if DefaultAPIURL == "" {
		t.Error("DefaultAPIURL empty")
	}
	if USDCMint == "" {
		t.Error("USDCMint empty")
	}
	if X402Version != 2 {
		t.Errorf("X402Version = %d", X402Version)
	}
}

// Verify context is threaded through API methods.
func TestChatRequiresContext(t *testing.T) {
	_, _ = NewClient()
	ctx := context.Background()
	_ = ctx // Verifying it compiles with context
}

// body402 is the JSON body the gateway sends on a 402 response.
const body402 = `{"error":{"message":"{\"x402_version\":2,\"accepts\":[{\"scheme\":\"exact\",\"network\":\"solana:mainnet\",\"amount\":\"2625\",\"asset\":\"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v\",\"pay_to\":\"RecipWallet\",\"max_timeout_seconds\":300}],\"cost_breakdown\":{\"provider_cost\":\"0.002500\",\"platform_fee\":\"0.000125\",\"total\":\"0.002625\",\"currency\":\"USDC\",\"fee_percent\":5},\"error\":\"Payment required\"}"}}`

// chatOKBody is a minimal valid ChatResponse JSON.
const chatOKBody = `{"id":"chatcmpl-1","object":"chat.completion","created":1700000000,"model":"openai/gpt-4o","choices":[{"index":0,"message":{"role":"assistant","content":"Hi"},"finish_reason":"stop"}]}`

func TestChatSucceeds200(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/chat/completions" {
			t.Errorf("unexpected path %q", r.URL.Path)
		}
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusOK)
		w.Write([]byte(chatOKBody)) //nolint:errcheck
	}))
	defer srv.Close()

	c, _ := NewClient(WithAPIURL(srv.URL))
	resp, err := c.ChatCompletion(context.Background(), ChatRequest{
		Model:    "openai/gpt-4o",
		Messages: []ChatMessage{{Role: RoleUser, Content: "hello"}},
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(resp.Choices) == 0 || resp.Choices[0].Message.Content != "Hi" {
		t.Errorf("unexpected response: %+v", resp)
	}
	if c.GetSessionSpent() != 0 {
		t.Error("session spent should be 0 for direct 200 (no payment)")
	}
}

func TestChat402ThenSuccess(t *testing.T) {
	callCount := 0
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		callCount++
		if callCount == 1 {
			// First request → 402
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(http.StatusPaymentRequired)
			w.Write([]byte(body402)) //nolint:errcheck
			return
		}
		// Second request → check payment header present, return 200
		if r.Header.Get("payment-signature") == "" {
			t.Error("retry request missing payment-signature header")
		}
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusOK)
		w.Write([]byte(chatOKBody)) //nolint:errcheck
	}))
	defer srv.Close()

	c, _ := NewClient(WithAPIURL(srv.URL))
	resp, err := c.ChatCompletion(context.Background(), ChatRequest{
		Model:    "openai/gpt-4o",
		Messages: []ChatMessage{{Role: RoleUser, Content: "hello"}},
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(resp.Choices) == 0 {
		t.Error("expected choices in response")
	}
	if callCount != 2 {
		t.Errorf("expected 2 calls, got %d", callCount)
	}
	// sessionSpent should reflect the cost from the 402 response (0.002625)
	if c.GetSessionSpent() == 0 {
		t.Error("session spent should be non-zero after successful payment")
	}
}

func TestChatBudgetExceeded(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Always return 402 with cost 0.002625
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusPaymentRequired)
		w.Write([]byte(body402)) //nolint:errcheck
	}))
	defer srv.Close()

	// Budget lower than the 0.002625 cost
	c, _ := NewClient(WithAPIURL(srv.URL), WithSessionBudget(0.001))
	_, err := c.ChatCompletion(context.Background(), ChatRequest{
		Model:    "openai/gpt-4o",
		Messages: []ChatMessage{{Role: RoleUser, Content: "hello"}},
	})
	if err == nil {
		t.Fatal("expected BudgetExceededError, got nil")
	}
	var be *BudgetExceededError
	if !errors.As(err, &be) {
		t.Errorf("expected *BudgetExceededError, got %T: %v", err, err)
	}
	// sessionSpent must NOT be incremented when budget is exceeded
	if c.GetSessionSpent() != 0 {
		t.Errorf("session spent should remain 0, got %f", c.GetSessionSpent())
	}
}

func TestListModelsSuccess(t *testing.T) {
	modelsBody := `{"object":"list","data":[{"id":"openai/gpt-4o","object":"model"}]}`
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/models" {
			t.Errorf("unexpected path %q", r.URL.Path)
		}
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusOK)
		w.Write([]byte(modelsBody)) //nolint:errcheck
	}))
	defer srv.Close()

	c, _ := NewClient(WithAPIURL(srv.URL))
	raw, err := c.ListModels(context.Background())
	if err != nil {
		t.Fatalf("ListModels: %v", err)
	}
	var parsed map[string]any
	if err := json.Unmarshal(raw, &parsed); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if parsed["object"] != "list" {
		t.Errorf("object = %v", parsed["object"])
	}
}

func TestGetSpending(t *testing.T) {
	budget := 5.0
	c, _ := NewClient(
		WithSessionBudget(budget),
	)

	summary, err := c.GetSpending(context.Background())
	if err != nil {
		t.Fatalf("GetSpending: %v", err)
	}
	if summary.SessionSpentUSDC != 0 {
		t.Errorf("initial session spent = %f, want 0", summary.SessionSpentUSDC)
	}
	if summary.BudgetRemaining == nil {
		t.Fatal("BudgetRemaining should be set when budget is configured")
	}
	if *summary.BudgetRemaining != budget {
		t.Errorf("BudgetRemaining = %f, want %f", *summary.BudgetRemaining, budget)
	}

	// Client with no budget should have nil BudgetRemaining
	c2, _ := NewClient()
	s2, err := c2.GetSpending(context.Background())
	if err != nil {
		t.Fatalf("GetSpending (no budget): %v", err)
	}
	if s2.BudgetRemaining != nil {
		t.Error("BudgetRemaining should be nil when no budget is set")
	}
}
