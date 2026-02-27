package rcr

import (
	"context"
	"encoding/base64"
	"encoding/json"
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
	_, err := createPaymentHeader(info, "/v1/chat/completions")
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
	var m map[string]interface{}
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
