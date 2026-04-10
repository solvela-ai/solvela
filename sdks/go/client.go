package rcr

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strconv"
	"strings"
	"sync"
	"time"
)

// Option configures a Client. Use the With* helpers to create options.
type Option func(*Client)

// WithAPIURL sets the gateway base URL (trailing slash is trimmed).
func WithAPIURL(url string) Option {
	return func(c *Client) { c.apiURL = strings.TrimRight(url, "/") }
}

// WithPrivateKey sets the Solana wallet private key for payment signing.
func WithPrivateKey(key string) Option {
	return func(c *Client) { c.wallet = NewWallet(key) }
}

// WithSessionBudget sets a maximum USDC spend for this client session.
func WithSessionBudget(budget float64) Option {
	return func(c *Client) { c.sessionBudget = &budget }
}

// WithTimeout overrides the default HTTP request timeout.
func WithTimeout(d time.Duration) Option {
	return func(c *Client) { c.httpClient.Timeout = d }
}

// WithHTTPClient replaces the default http.Client entirely.
func WithHTTPClient(client *http.Client) Option {
	return func(c *Client) { c.httpClient = client }
}

// Client is the main entry point for the RustyClawRouter Go SDK.
type Client struct {
	apiURL        string
	wallet        *Wallet
	sessionBudget *float64
	sessionSpent  float64
	mu            sync.Mutex
	httpClient    *http.Client
}

// NewClient creates a Client with the given options.
func NewClient(opts ...Option) (*Client, error) {
	c := &Client{
		apiURL: DefaultAPIURL,
		wallet: NewWallet(""),
		httpClient: &http.Client{
			Timeout: time.Duration(DefaultTimeout) * time.Second,
		},
	}
	for _, opt := range opts {
		opt(c)
	}
	return c, nil
}

// Chat is a convenience wrapper that sends a single user prompt and returns
// the assistant's reply text.
func (c *Client) Chat(ctx context.Context, model, prompt string) (string, error) {
	resp, err := c.ChatCompletion(ctx, ChatRequest{
		Model:    model,
		Messages: []ChatMessage{{Role: RoleUser, Content: prompt}},
	})
	if err != nil {
		return "", err
	}
	if len(resp.Choices) == 0 {
		return "", &APIError{Message: "no choices in response"}
	}
	return resp.Choices[0].Message.Content, nil
}

// ChatCompletion sends an OpenAI-compatible chat completion request.
// If the gateway responds with HTTP 402, the client automatically constructs
// a payment header and retries the request.
func (c *Client) ChatCompletion(ctx context.Context, req ChatRequest) (*ChatResponse, error) {
	url := fmt.Sprintf("%s/v1/chat/completions", c.apiURL)

	body, err := json.Marshal(req)
	if err != nil {
		return nil, err
	}

	httpReq, err := http.NewRequestWithContext(ctx, "POST", url, bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	httpReq.Header.Set("Content-Type", "application/json")

	resp, err := c.httpClient.Do(httpReq)
	if err != nil {
		return nil, err
	}

	// Handle x402 payment flow
	if resp.StatusCode == http.StatusPaymentRequired {
		// Read and close the 402 body before making the retry request to avoid
		// double-close and to free the connection for reuse.
		paymentInfo, parseErr := c.parse402(resp)
		resp.Body.Close()
		if parseErr != nil {
			return nil, &PaymentError{Message: "failed to parse 402: " + parseErr.Error()}
		}

		cost, parseFloatErr := strconv.ParseFloat(paymentInfo.CostBreakdown.Total, 64)
		if parseFloatErr != nil {
			return nil, fmt.Errorf("invalid cost in 402 response: %w", parseFloatErr)
		}

		c.mu.Lock()
		spent := c.sessionSpent
		budget := c.sessionBudget
		c.mu.Unlock()

		if budget != nil && spent+cost > *budget {
			return nil, &BudgetExceededError{
				Budget: *budget,
				Spent:  spent,
				Cost:   cost,
			}
		}

		header, headerErr := createPaymentHeader(paymentInfo, url, c.wallet, body)
		if headerErr != nil {
			return nil, headerErr
		}

		httpReq2, err := http.NewRequestWithContext(ctx, "POST", url, bytes.NewReader(body))
		if err != nil {
			return nil, err
		}
		httpReq2.Header.Set("Content-Type", "application/json")
		httpReq2.Header.Set("payment-signature", header)

		resp2, err := c.httpClient.Do(httpReq2)
		if err != nil {
			return nil, err
		}
		defer resp2.Body.Close()

		// Only credit spend after the retry succeeds (2xx).
		if resp2.StatusCode >= 200 && resp2.StatusCode < 300 {
			c.mu.Lock()
			c.sessionSpent += cost
			c.mu.Unlock()
		}

		resp = resp2
	} else {
		defer resp.Body.Close()
	}

	if resp.StatusCode != http.StatusOK {
		bodyBytes, _ := io.ReadAll(resp.Body)
		return nil, &APIError{StatusCode: resp.StatusCode, Message: string(bodyBytes)}
	}

	var chatResp ChatResponse
	if err := json.NewDecoder(resp.Body).Decode(&chatResp); err != nil {
		return nil, err
	}
	return &chatResp, nil
}

// SmartChat sends a prompt using a smart-routing profile name as the model.
func (c *Client) SmartChat(ctx context.Context, prompt string, profile string) (*ChatResponse, error) {
	return c.ChatCompletion(ctx, ChatRequest{
		Model:    profile,
		Messages: []ChatMessage{{Role: RoleUser, Content: prompt}},
	})
}

// ListModels returns the raw JSON list of available models from the gateway.
func (c *Client) ListModels(ctx context.Context) (json.RawMessage, error) {
	url := fmt.Sprintf("%s/v1/models", c.apiURL)
	httpReq, err := http.NewRequestWithContext(ctx, "GET", url, nil)
	if err != nil {
		return nil, err
	}
	resp, err := c.httpClient.Do(httpReq)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		bodyBytes, _ := io.ReadAll(resp.Body)
		return nil, &APIError{StatusCode: resp.StatusCode, Message: string(bodyBytes)}
	}

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, err
	}
	return json.RawMessage(body), nil
}

// Health checks the gateway health endpoint.
func (c *Client) Health(ctx context.Context) (map[string]any, error) {
	url := fmt.Sprintf("%s/health", c.apiURL)
	httpReq, err := http.NewRequestWithContext(ctx, "GET", url, nil)
	if err != nil {
		return nil, err
	}
	resp, err := c.httpClient.Do(httpReq)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	var result map[string]any
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return nil, err
	}
	return result, nil
}

// GetSessionSpent returns the total USDC spent in this session.
func (c *Client) GetSessionSpent() float64 {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.sessionSpent
}

// GetBalance returns the USDC balance of the configured wallet.
// Returns 0, nil if the wallet has no address or the balance endpoint is not
// yet implemented (404).
func (c *Client) GetBalance(ctx context.Context) (float64, error) {
	if c.wallet == nil || c.wallet.Address() == "" {
		return 0, nil
	}
	url := fmt.Sprintf("%s/v1/wallet/balance?address=%s", c.apiURL, c.wallet.Address())
	httpReq, err := http.NewRequestWithContext(ctx, "GET", url, nil)
	if err != nil {
		return 0, err
	}
	resp, err := c.httpClient.Do(httpReq)
	if err != nil {
		return 0, err
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusNotFound {
		return 0, nil
	}
	if resp.StatusCode != http.StatusOK {
		bodyBytes, _ := io.ReadAll(resp.Body)
		return 0, &APIError{StatusCode: resp.StatusCode, Message: string(bodyBytes)}
	}

	var result struct {
		Balance float64 `json:"balance"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return 0, err
	}
	return result.Balance, nil
}

// GetSpending returns in-memory session spending statistics.
func (c *Client) GetSpending(ctx context.Context) (*SpendSummary, error) {
	c.mu.Lock()
	spent := c.sessionSpent
	budget := c.sessionBudget
	c.mu.Unlock()

	summary := &SpendSummary{
		WalletAddress:    c.wallet.Address(),
		SessionSpentUSDC: spent,
	}

	if budget != nil {
		remaining := *budget - spent
		summary.BudgetRemaining = &remaining
	}

	return summary, nil
}

// GetCostEstimate returns a cost breakdown for the given model and token counts.
// Not yet implemented — requires a 402 round-trip to fetch pricing.
func (c *Client) GetCostEstimate(model string, inputTokens, outputTokens int) (*CostBreakdown, error) {
	return nil, fmt.Errorf("not implemented")
}

// parse402 extracts the PaymentRequired info from an HTTP 402 response body.
// The gateway wraps the payment info inside an OpenAI-style error envelope:
//
//	{"error": {"message": "<json-encoded PaymentRequired>"}}
func (c *Client) parse402(resp *http.Response) (*PaymentRequired, error) {
	bodyBytes, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, err
	}

	var errorResp struct {
		Error struct {
			Message string `json:"message"`
		} `json:"error"`
	}
	if err := json.Unmarshal(bodyBytes, &errorResp); err != nil {
		return nil, err
	}

	var paymentInfo PaymentRequired
	if err := json.Unmarshal([]byte(errorResp.Error.Message), &paymentInfo); err != nil {
		return nil, err
	}
	return &paymentInfo, nil
}
