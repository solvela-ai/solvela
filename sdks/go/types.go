package rcr

// Role represents a chat message role.
type Role string

const (
	RoleSystem    Role = "system"
	RoleUser      Role = "user"
	RoleAssistant Role = "assistant"
	RoleTool      Role = "tool"
)

// ChatMessage is a single message in a chat conversation.
type ChatMessage struct {
	Role    Role   `json:"role"`
	Content string `json:"content"`
	Name    string `json:"name,omitempty"`
}

// ChatRequest is the OpenAI-compatible request body for chat completions.
type ChatRequest struct {
	Model       string        `json:"model"`
	Messages    []ChatMessage `json:"messages"`
	MaxTokens   *int          `json:"max_tokens,omitempty"`
	Temperature *float64      `json:"temperature,omitempty"`
	TopP        *float64      `json:"top_p,omitempty"`
	Stream      bool          `json:"stream"`
}

// Usage contains token usage information from a completion response.
type Usage struct {
	PromptTokens     int `json:"prompt_tokens"`
	CompletionTokens int `json:"completion_tokens"`
	TotalTokens      int `json:"total_tokens"`
}

// ChatChoice is a single completion choice.
type ChatChoice struct {
	Index        int         `json:"index"`
	Message      ChatMessage `json:"message"`
	FinishReason *string     `json:"finish_reason"`
}

// ChatResponse is the OpenAI-compatible response from chat completions.
type ChatResponse struct {
	ID      string       `json:"id"`
	Object  string       `json:"object"`
	Created int64        `json:"created"`
	Model   string       `json:"model"`
	Choices []ChatChoice `json:"choices"`
	Usage   *Usage       `json:"usage,omitempty"`
}

// CostBreakdown shows the price breakdown for a request.
type CostBreakdown struct {
	ProviderCost string `json:"provider_cost"`
	PlatformFee  string `json:"platform_fee"`
	Total        string `json:"total"`
	Currency     string `json:"currency"`
	FeePercent   int    `json:"fee_percent"`
}

// PaymentAccept describes one accepted payment method from a 402 response.
type PaymentAccept struct {
	Scheme            string `json:"scheme"`
	Network           string `json:"network"`
	Amount            string `json:"amount"`
	Asset             string `json:"asset"`
	PayTo             string `json:"pay_to"`
	MaxTimeoutSeconds int    `json:"max_timeout_seconds"`
}

// PaymentRequired is the parsed body of an HTTP 402 response.
type PaymentRequired struct {
	X402Version   int             `json:"x402_version"`
	Accepts       []PaymentAccept `json:"accepts"`
	CostBreakdown CostBreakdown   `json:"cost_breakdown"`
	Error         string          `json:"error"`
}

// SpendInfo tracks cumulative spending information.
type SpendInfo struct {
	TotalRequests int64   `json:"total_requests"`
	TotalCostUSDC float64 `json:"total_cost_usdc"`
	DailyCostUSDC float64 `json:"daily_cost_usdc"`
}
