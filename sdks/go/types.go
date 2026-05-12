package solvela

import (
	"encoding/json"
	"fmt"
)

// Role represents a chat message role.
type Role string

const (
	RoleSystem    Role = "system"
	RoleUser      Role = "user"
	RoleAssistant Role = "assistant"
	RoleTool      Role = "tool"
	RoleDeveloper Role = "developer"
)

// ChatMessage represents a single message in a chat conversation.
type ChatMessage struct {
	Role       Role       `json:"role"`
	Content    string     `json:"content"`
	Name       *string    `json:"name,omitempty"`
	ToolCalls  []ToolCall `json:"tool_calls,omitempty"`
	ToolCallID *string    `json:"tool_call_id,omitempty"`
}

// ChatRequest represents a chat completion request.
type ChatRequest struct {
	Model       string           `json:"model"`
	Messages    []ChatMessage    `json:"messages"`
	MaxTokens   *int             `json:"max_tokens,omitempty"`
	Temperature *float64         `json:"temperature,omitempty"`
	TopP        *float64         `json:"top_p,omitempty"`
	Stream      bool             `json:"stream"`
	Tools       []ToolDefinition `json:"tools,omitempty"`
	ToolChoice  interface{}      `json:"tool_choice,omitempty"`
}

// ChatResponse represents a chat completion response.
type ChatResponse struct {
	ID      string       `json:"id"`
	Object  string       `json:"object"`
	Created int64        `json:"created"`
	Model   string       `json:"model"`
	Choices []ChatChoice `json:"choices"`
	Usage   *Usage       `json:"usage,omitempty"`
}

// ChatChoice represents a single completion choice.
type ChatChoice struct {
	Index        int         `json:"index"`
	Message      ChatMessage `json:"message"`
	FinishReason *string     `json:"finish_reason,omitempty"`
}

// Usage represents token usage statistics.
type Usage struct {
	PromptTokens     int `json:"prompt_tokens"`
	CompletionTokens int `json:"completion_tokens"`
	TotalTokens      int `json:"total_tokens"`
}

// ChatChunk represents a streaming chat completion chunk.
type ChatChunk struct {
	ID      string            `json:"id"`
	Object  string            `json:"object"`
	Created int64             `json:"created"`
	Model   string            `json:"model"`
	Choices []ChatChunkChoice `json:"choices"`
}

// ChatChunkChoice represents a single streaming choice.
type ChatChunkChoice struct {
	Index        int       `json:"index"`
	Delta        ChatDelta `json:"delta"`
	FinishReason *string   `json:"finish_reason,omitempty"`
}

// ChatDelta represents incremental content in a streaming chunk.
type ChatDelta struct {
	Role      *Role           `json:"role,omitempty"`
	Content   *string         `json:"content,omitempty"`
	ToolCalls []ToolCallDelta `json:"tool_calls,omitempty"`
}

// ToolCall represents a tool call in a message.
type ToolCall struct {
	ID       string       `json:"id"`
	Type     string       `json:"type"`
	Function FunctionCall `json:"function"`
}

// FunctionCall represents the function details in a tool call.
type FunctionCall struct {
	Name      string `json:"name"`
	Arguments string `json:"arguments"`
}

// ToolCallDelta represents an incremental tool call in streaming.
type ToolCallDelta struct {
	Index    int                `json:"index"`
	ID       *string            `json:"id,omitempty"`
	Type     *string            `json:"type,omitempty"`
	Function *FunctionCallDelta `json:"function,omitempty"`
}

// FunctionCallDelta represents incremental function call data.
type FunctionCallDelta struct {
	Name      *string `json:"name,omitempty"`
	Arguments *string `json:"arguments,omitempty"`
}

// ToolDefinition represents a tool that can be used by the model.
type ToolDefinition struct {
	Type     string                  `json:"type"`
	Function FunctionDefinitionInner `json:"function"`
}

// FunctionDefinitionInner represents the function details in a tool definition.
type FunctionDefinitionInner struct {
	Name        string      `json:"name"`
	Description *string     `json:"description,omitempty"`
	Parameters  interface{} `json:"parameters,omitempty"`
}

// Resource represents the API resource being accessed.
type Resource struct {
	URL    string `json:"url"`
	Method string `json:"method"`
}

// Scheme is the x402 payment scheme. Mirrors Python's
// `Scheme = Literal["exact", "escrow"]` + `_KNOWN_SCHEMES` guard: a gateway
// response carrying an unknown scheme is rejected at parse time rather than
// silently mis-branching at scheme-matching time.
type Scheme string

const (
	SchemeExact  Scheme = "exact"
	SchemeEscrow Scheme = "escrow"
)

// parseScheme validates a wire scheme string and returns the typed value.
// An unknown scheme is a wire-format failure, surfaced as a typed ClientError
// so callers can distinguish "gateway refused" from "gateway speaks a dialect
// we don't understand."
func parseScheme(raw string) (Scheme, error) {
	switch Scheme(raw) {
	case SchemeExact, SchemeEscrow:
		return Scheme(raw), nil
	}
	return "", &ClientError{Message: fmt.Sprintf("unknown payment scheme: %q", raw)}
}

// PaymentAccept describes an accepted payment scheme.
type PaymentAccept struct {
	Scheme            Scheme  `json:"scheme"`
	Network           string  `json:"network"`
	Amount            string  `json:"amount"`
	Asset             string  `json:"asset"`
	PayTo             string  `json:"pay_to"`
	MaxTimeoutSeconds int     `json:"max_timeout_seconds"`
	EscrowProgramID   *string `json:"escrow_program_id,omitempty"`
}

// UnmarshalJSON decodes a PaymentAccept and rejects unknown schemes at the
// wire boundary. See parseScheme for rationale.
func (p *PaymentAccept) UnmarshalJSON(data []byte) error {
	type alias PaymentAccept
	aux := &struct {
		Scheme string `json:"scheme"`
		*alias
	}{alias: (*alias)(p)}
	if err := json.Unmarshal(data, aux); err != nil {
		return err
	}
	s, err := parseScheme(aux.Scheme)
	if err != nil {
		return err
	}
	p.Scheme = s
	return nil
}

// PaymentRequired represents a 402 Payment Required response.
type PaymentRequired struct {
	X402Version   int             `json:"x402_version"`
	Resource      Resource        `json:"resource"`
	Accepts       []PaymentAccept `json:"accepts"`
	CostBreakdown CostBreakdown   `json:"cost_breakdown"`
	Error         string          `json:"error"`
}

// CostBreakdown provides the cost details for a request.
type CostBreakdown struct {
	ProviderCost string `json:"provider_cost"`
	PlatformFee  string `json:"platform_fee"`
	Total        string `json:"total"`
	Currency     string `json:"currency"`
	FeePercent   int    `json:"fee_percent"`
}

// SolanaPayload is the payload for exact Solana payments.
type SolanaPayload struct {
	Transaction string `json:"transaction"`
}

// EscrowPayload is the payload for escrow-based payments.
type EscrowPayload struct {
	DepositTx   string `json:"deposit_tx"`
	ServiceID   string `json:"service_id"`
	AgentPubkey string `json:"agent_pubkey"`
}

// PaymentPayload represents a payment attached to a request.
type PaymentPayload struct {
	X402Version int           `json:"x402_version"`
	Resource    Resource      `json:"resource"`
	Accepted    PaymentAccept `json:"accepted"`
	Payload     interface{}   `json:"payload"`
}

// ModelInfo describes a model available on the gateway's GET /v1/models
// registry.
//
// The wire format nests capabilities under `capabilities: {...}` and pricing
// under `pricing: {...}`; the struct flattens those so internal code stays
// terse. Pricing values are USDC per million tokens as floats (e.g. 0.28 for
// DeepSeek's input rate), NOT atomic units — convert at the boundary if you
// need atomic units.
//
// Mirrors Python's ModelInfo dataclass (the cross-SDK canonical) and TS's
// post-rewrite layout. Pre-fix flat fields silently parsed every value as
// zero/false; see commit history for the regression.
type ModelInfo struct {
	ID                   string
	Provider             string
	DisplayName          string
	ContextWindow        int
	SupportsStreaming    bool
	SupportsTools        bool
	SupportsVision       bool
	Reasoning            bool
	InputUsdcPerMillion  float64
	OutputUsdcPerMillion float64
	Currency             string
	FeePercent           int
}

type modelInfoWire struct {
	ID            string `json:"id"`
	Object        string `json:"object,omitempty"`
	Provider      string `json:"provider"`
	DisplayName   string `json:"display_name"`
	ContextWindow int    `json:"context_window"`
	Capabilities  struct {
		Streaming bool `json:"streaming"`
		Tools     bool `json:"tools"`
		Vision    bool `json:"vision"`
		Reasoning bool `json:"reasoning"`
	} `json:"capabilities"`
	Pricing struct {
		InputPerMillion  float64 `json:"input_per_million"`
		OutputPerMillion float64 `json:"output_per_million"`
		Currency         *string `json:"currency,omitempty"`
		FeePercent       *int    `json:"fee_percent,omitempty"`
	} `json:"pricing"`
}

// MarshalJSON emits the nested gateway wire shape.
func (m ModelInfo) MarshalJSON() ([]byte, error) {
	w := modelInfoWire{
		ID:            m.ID,
		Object:        "model",
		Provider:      m.Provider,
		DisplayName:   m.DisplayName,
		ContextWindow: m.ContextWindow,
	}
	w.Capabilities.Streaming = m.SupportsStreaming
	w.Capabilities.Tools = m.SupportsTools
	w.Capabilities.Vision = m.SupportsVision
	w.Capabilities.Reasoning = m.Reasoning
	w.Pricing.InputPerMillion = m.InputUsdcPerMillion
	w.Pricing.OutputPerMillion = m.OutputUsdcPerMillion
	currency := m.Currency
	if currency == "" {
		currency = "USDC"
	}
	w.Pricing.Currency = &currency
	feePercent := m.FeePercent
	w.Pricing.FeePercent = &feePercent
	return json.Marshal(w)
}

// UnmarshalJSON decodes the nested gateway wire shape, defaulting currency to
// "USDC" and fee_percent to 5 when the gateway omits them (matches Python).
func (m *ModelInfo) UnmarshalJSON(data []byte) error {
	var w modelInfoWire
	if err := json.Unmarshal(data, &w); err != nil {
		return err
	}
	m.ID = w.ID
	m.Provider = w.Provider
	m.DisplayName = w.DisplayName
	m.ContextWindow = w.ContextWindow
	m.SupportsStreaming = w.Capabilities.Streaming
	m.SupportsTools = w.Capabilities.Tools
	m.SupportsVision = w.Capabilities.Vision
	m.Reasoning = w.Capabilities.Reasoning
	m.InputUsdcPerMillion = w.Pricing.InputPerMillion
	m.OutputUsdcPerMillion = w.Pricing.OutputPerMillion
	if w.Pricing.Currency != nil {
		m.Currency = *w.Pricing.Currency
	} else {
		m.Currency = "USDC"
	}
	if w.Pricing.FeePercent != nil {
		m.FeePercent = *w.Pricing.FeePercent
	} else {
		m.FeePercent = 5
	}
	return nil
}
