package solvela

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

// maxResponseBytes caps the response body size to 10 MB to prevent memory
// exhaustion from a malicious or misbehaving gateway. Used on success paths
// (chat completions, model lists) where a legitimate body can be large.
const maxResponseBytes = 10 << 20 // 10 MB

// maxErrorBodyBytes caps the response body size for pure error paths where
// the body is destined for a GatewayError.Message string. 4 KB is plenty for
// any human-readable diagnostic (rate-limit text, auth failure, HTML title)
// and prevents a misbehaving gateway from forcing a multi-MB error string
// into memory and logs. Applied where we know the body cannot be a
// legitimate large success payload.
const maxErrorBodyBytes = 4 << 10 // 4 KB

// Transport handles HTTP communication with the Solvela gateway.
type Transport struct {
	baseURL string
	timeout time.Duration
	client  *http.Client
}

// NewTransport creates a new Transport with the given base URL and timeout.
func NewTransport(baseURL string, timeout time.Duration) *Transport {
	return &Transport{
		baseURL: strings.TrimRight(baseURL, "/"),
		timeout: timeout,
		client: &http.Client{
			Timeout: timeout,
			// Refuse redirects entirely: following a redirect would forward the
			// Payment-Signature header to an unintended destination.
			CheckRedirect: func(_ *http.Request, _ []*http.Request) error {
				return http.ErrUseLastResponse
			},
		},
	}
}

// SendChatResult is either a ChatResponse or PaymentRequired.
type SendChatResult struct {
	Response        *ChatResponse
	PaymentRequired *PaymentRequired
}

// SendChat sends a non-streaming chat request.
func (t *Transport) SendChat(ctx context.Context, request *ChatRequest, paymentSignature string, extraHeaders map[string]string) (*SendChatResult, error) {
	request.Stream = false
	body, err := json.Marshal(request)
	if err != nil {
		return nil, fmt.Errorf("marshal request: %w", err)
	}

	url := t.baseURL + "/v1/chat/completions"
	req, err := http.NewRequestWithContext(ctx, "POST", url, bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/json")
	if paymentSignature != "" {
		req.Header.Set("Payment-Signature", paymentSignature)
	}
	for k, v := range extraHeaders {
		req.Header.Set(k, v)
	}

	resp, err := t.client.Do(req)
	if err != nil {
		if ctx.Err() != nil {
			return nil, &TimeoutError{TimeoutSecs: t.timeout.Seconds()}
		}
		return nil, err
	}
	defer resp.Body.Close()

	data, err := io.ReadAll(io.LimitReader(resp.Body, maxResponseBytes))
	if err != nil {
		return nil, err
	}

	switch resp.StatusCode {
	case 200:
		var chatResp ChatResponse
		if err := json.Unmarshal(data, &chatResp); err != nil {
			return nil, fmt.Errorf("parse error: %w", err)
		}
		return &SendChatResult{Response: &chatResp}, nil
	case 402:
		var pr PaymentRequired
		if err := json.Unmarshal(data, &pr); err != nil {
			return nil, fmt.Errorf("parse 402 error: %w", err)
		}
		return &SendChatResult{PaymentRequired: &pr}, nil
	default:
		// Best-effort JSON parse: prefer .error from a structured body, fall
		// back to the raw response text. A non-JSON body is expected here
		// (e.g., HTML error pages from a misconfigured proxy), so the
		// unmarshal error is intentionally discarded.
		var errData map[string]interface{}
		_ = json.Unmarshal(data, &errData)
		msg := string(data)
		if e, ok := errData["error"]; ok {
			msg = fmt.Sprintf("%v", e)
		}
		return nil, &GatewayError{Status: resp.StatusCode, Message: msg}
	}
}

// ChatChunkOrError holds either a streaming chunk or an error.
type ChatChunkOrError struct {
	Chunk *ChatChunk
	Err   error
}

// SendChatStream sends a streaming chat request and returns a channel of chunks.
func (t *Transport) SendChatStream(ctx context.Context, request *ChatRequest, paymentSignature string, extraHeaders map[string]string) (<-chan ChatChunkOrError, error) {
	request.Stream = true
	body, err := json.Marshal(request)
	if err != nil {
		return nil, fmt.Errorf("marshal request: %w", err)
	}

	url := t.baseURL + "/v1/chat/completions"
	req, err := http.NewRequestWithContext(ctx, "POST", url, bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/json")
	if paymentSignature != "" {
		req.Header.Set("Payment-Signature", paymentSignature)
	}
	for k, v := range extraHeaders {
		req.Header.Set(k, v)
	}

	resp, err := t.client.Do(req)
	if err != nil {
		return nil, err
	}

	if resp.StatusCode == 402 {
		defer resp.Body.Close()
		data, readErr := io.ReadAll(io.LimitReader(resp.Body, maxResponseBytes))
		if readErr != nil {
			return nil, &GatewayError{Status: 402, Message: fmt.Sprintf("read 402 body: %v", readErr)}
		}
		var pr PaymentRequired
		if err := json.Unmarshal(data, &pr); err != nil {
			return nil, &GatewayError{Status: 402, Message: fmt.Sprintf("parse 402 body: %v", err)}
		}
		return nil, &PaymentRequiredError{PaymentRequired: pr}
	}
	if resp.StatusCode != 200 {
		defer resp.Body.Close()
		data, readErr := io.ReadAll(io.LimitReader(resp.Body, maxErrorBodyBytes))
		if readErr != nil {
			return nil, &GatewayError{Status: resp.StatusCode, Message: fmt.Sprintf("read error body: %v", readErr)}
		}
		return nil, &GatewayError{Status: resp.StatusCode, Message: string(data)}
	}

	ch := make(chan ChatChunkOrError)
	go func() {
		defer close(ch)
		defer resp.Body.Close()
		scanner := bufio.NewScanner(resp.Body)
		// Default scanner token buffer is 64 KB, which can truncate large SSE
		// chunks containing long completions. Allow up to 1 MB per line.
		scanner.Buffer(make([]byte, 0, 64*1024), 1<<20)
		// send returns false if the context was cancelled while we waited to
		// hand off; the caller must then return so the deferred body close
		// runs and the goroutine exits.
		send := func(item ChatChunkOrError) bool {
			select {
			case ch <- item:
				return true
			case <-ctx.Done():
				return false
			}
		}
		for scanner.Scan() {
			if ctx.Err() != nil {
				return
			}
			line := scanner.Text()
			if strings.HasPrefix(line, "data: ") {
				dataStr := strings.TrimPrefix(line, "data: ")
				dataStr = strings.TrimSpace(dataStr)
				if dataStr == "[DONE]" {
					return
				}
				var chunk ChatChunk
				if err := json.Unmarshal([]byte(dataStr), &chunk); err != nil {
					send(ChatChunkOrError{Err: err})
					return
				}
				if !send(ChatChunkOrError{Chunk: &chunk}) {
					return
				}
			}
		}
		// Surface scanner errors (e.g., connection reset, oversize line) so
		// truncated streams are not mistaken for clean closes. Skip the
		// error-send if the context is already cancelled (consumer left).
		if err := scanner.Err(); err != nil && ctx.Err() == nil {
			send(ChatChunkOrError{Err: fmt.Errorf("stream read error: %w", err)})
		}
	}()
	return ch, nil
}

// FetchModels retrieves available models from the gateway.
func (t *Transport) FetchModels(ctx context.Context) ([]ModelInfo, error) {
	url := t.baseURL + "/v1/models"
	req, err := http.NewRequestWithContext(ctx, "GET", url, nil)
	if err != nil {
		return nil, err
	}
	resp, err := t.client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		// Surface the response body so callers can diagnose an upstream
		// failure (rate-limit message, auth error, etc.) instead of seeing
		// a bare status code. Mirror SendChatStream's non-200 branch: if the
		// body read itself fails (connection reset, timeout mid-drain), report
		// that explicitly rather than producing an empty Message.
		data, readErr := io.ReadAll(io.LimitReader(resp.Body, maxErrorBodyBytes))
		if readErr != nil {
			return nil, &GatewayError{Status: resp.StatusCode, Message: fmt.Sprintf("read error body: %v", readErr)}
		}
		return nil, &GatewayError{Status: resp.StatusCode, Message: string(data)}
	}
	var result struct {
		Data []ModelInfo `json:"data"`
	}
	// Cap the response body to maxResponseBytes to prevent a malicious or
	// misbehaving gateway from forcing the client to allocate unbounded memory.
	if err := json.NewDecoder(io.LimitReader(resp.Body, maxResponseBytes)).Decode(&result); err != nil {
		return nil, fmt.Errorf("parse models: %w", err)
	}
	return result.Data, nil
}
