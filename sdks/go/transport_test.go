package solvela

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func TestTransportSendChat200(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/chat/completions" {
			t.Errorf("unexpected path: %s", r.URL.Path)
		}
		if r.Header.Get("Content-Type") != "application/json" {
			t.Error("missing Content-Type header")
		}

		resp := ChatResponse{
			ID:    "chatcmpl-test",
			Model: "gpt-4",
			Choices: []ChatChoice{
				{Index: 0, Message: ChatMessage{Role: RoleAssistant, Content: "Hello!"}},
			},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	transport := NewTransport(server.URL, 10*time.Second)
	req := &ChatRequest{Model: "gpt-4", Messages: []ChatMessage{{Role: RoleUser, Content: "Hi"}}}

	result, err := transport.SendChat(context.Background(), req, "", nil)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Response == nil {
		t.Fatal("expected response, got nil")
	}
	if result.Response.ID != "chatcmpl-test" {
		t.Errorf("id: got %q, want %q", result.Response.ID, "chatcmpl-test")
	}
	if result.PaymentRequired != nil {
		t.Error("expected no payment required")
	}
}

func TestTransportSendChat402(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		pr := PaymentRequired{
			X402Version: 2,
			Error:       "payment required",
			CostBreakdown: CostBreakdown{
				Total:    "1000",
				Currency: "USDC",
			},
			Accepts: []PaymentAccept{
				{Scheme: "exact", Network: SolanaNetwork, Amount: "1000", PayTo: "recipient123"},
			},
		}
		w.WriteHeader(402)
		json.NewEncoder(w).Encode(pr)
	}))
	defer server.Close()

	transport := NewTransport(server.URL, 10*time.Second)
	req := &ChatRequest{Model: "gpt-4", Messages: []ChatMessage{{Role: RoleUser, Content: "Hi"}}}

	result, err := transport.SendChat(context.Background(), req, "", nil)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.PaymentRequired == nil {
		t.Fatal("expected payment required")
	}
	if result.PaymentRequired.CostBreakdown.Total != "1000" {
		t.Errorf("total: got %q, want %q", result.PaymentRequired.CostBreakdown.Total, "1000")
	}
}

func TestTransportSendChat500(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(500)
		w.Write([]byte(`{"error":"internal server error"}`))
	}))
	defer server.Close()

	transport := NewTransport(server.URL, 10*time.Second)
	req := &ChatRequest{Model: "gpt-4", Messages: []ChatMessage{{Role: RoleUser, Content: "Hi"}}}

	_, err := transport.SendChat(context.Background(), req, "", nil)
	if err == nil {
		t.Fatal("expected error")
	}
	gatewayErr, ok := err.(*GatewayError)
	if !ok {
		t.Fatalf("expected GatewayError, got %T", err)
	}
	if gatewayErr.Status != 500 {
		t.Errorf("status: got %d, want 500", gatewayErr.Status)
	}
}

func TestTransportSendChatPaymentHeader(t *testing.T) {
	var receivedSig string
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		receivedSig = r.Header.Get("Payment-Signature")
		resp := ChatResponse{ID: "paid"}
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	transport := NewTransport(server.URL, 10*time.Second)
	req := &ChatRequest{Model: "gpt-4", Messages: []ChatMessage{{Role: RoleUser, Content: "Hi"}}}

	_, err := transport.SendChat(context.Background(), req, "test-sig-123", nil)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if receivedSig != "test-sig-123" {
		t.Errorf("sig: got %q, want %q", receivedSig, "test-sig-123")
	}
}

func TestTransportSendChatExtraHeaders(t *testing.T) {
	var receivedCustom string
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		receivedCustom = r.Header.Get("X-Custom")
		json.NewEncoder(w).Encode(ChatResponse{ID: "test"})
	}))
	defer server.Close()

	transport := NewTransport(server.URL, 10*time.Second)
	req := &ChatRequest{Model: "gpt-4", Messages: []ChatMessage{{Role: RoleUser, Content: "Hi"}}}
	headers := map[string]string{"X-Custom": "custom-value"}

	_, err := transport.SendChat(context.Background(), req, "", headers)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if receivedCustom != "custom-value" {
		t.Errorf("custom header: got %q, want %q", receivedCustom, "custom-value")
	}
}

func TestTransportSendChatStream(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		flusher := w.(http.Flusher)

		content := "Hello"
		chunk := ChatChunk{
			ID:    "chunk-1",
			Model: "gpt-4",
			Choices: []ChatChunkChoice{
				{Index: 0, Delta: ChatDelta{Content: &content}},
			},
		}
		data, _ := json.Marshal(chunk)
		fmt.Fprintf(w, "data: %s\n\n", data)
		flusher.Flush()

		fmt.Fprint(w, "data: [DONE]\n\n")
		flusher.Flush()
	}))
	defer server.Close()

	transport := NewTransport(server.URL, 10*time.Second)
	req := &ChatRequest{Model: "gpt-4", Messages: []ChatMessage{{Role: RoleUser, Content: "Hi"}}}

	ch, err := transport.SendChatStream(context.Background(), req, "", nil)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	var chunks []ChatChunk
	for item := range ch {
		if item.Err != nil {
			t.Fatalf("chunk error: %v", item.Err)
		}
		chunks = append(chunks, *item.Chunk)
	}

	if len(chunks) != 1 {
		t.Fatalf("got %d chunks, want 1", len(chunks))
	}
	if chunks[0].ID != "chunk-1" {
		t.Errorf("chunk id: got %q, want %q", chunks[0].ID, "chunk-1")
	}
}

func TestTransportSendChatStream402(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(402)
		json.NewEncoder(w).Encode(PaymentRequired{X402Version: 2})
	}))
	defer server.Close()

	transport := NewTransport(server.URL, 10*time.Second)
	req := &ChatRequest{Model: "gpt-4", Messages: []ChatMessage{{Role: RoleUser, Content: "Hi"}}}

	_, err := transport.SendChatStream(context.Background(), req, "", nil)
	if err == nil {
		t.Fatal("expected error")
	}
	_, ok := err.(*PaymentRequiredError)
	if !ok {
		t.Fatalf("expected PaymentRequiredError, got %T: %v", err, err)
	}
}

func TestTransportFetchModels(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/models" {
			t.Errorf("unexpected path: %s", r.URL.Path)
		}
		result := struct {
			Data []ModelInfo `json:"data"`
		}{
			Data: []ModelInfo{
				{ID: "gpt-4", Provider: "openai", DisplayName: "GPT-4"},
				{ID: "claude-3", Provider: "anthropic", DisplayName: "Claude 3"},
			},
		}
		json.NewEncoder(w).Encode(result)
	}))
	defer server.Close()

	transport := NewTransport(server.URL, 10*time.Second)
	models, err := transport.FetchModels(context.Background())
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(models) != 2 {
		t.Fatalf("got %d models, want 2", len(models))
	}
	if models[0].ID != "gpt-4" {
		t.Errorf("model[0].ID: got %q, want %q", models[0].ID, "gpt-4")
	}
}

func TestTransportFetchModels500(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(500)
		_, _ = w.Write([]byte(`{"error":"internal server error"}`))
	}))
	defer server.Close()

	transport := NewTransport(server.URL, 10*time.Second)
	_, err := transport.FetchModels(context.Background())
	if err == nil {
		t.Fatal("expected error")
	}
	gatewayErr, ok := err.(*GatewayError)
	if !ok {
		t.Fatalf("expected GatewayError, got %T", err)
	}
	if gatewayErr.Status != 500 {
		t.Errorf("status: got %d, want 500", gatewayErr.Status)
	}
	if !strings.Contains(gatewayErr.Message, "internal server error") {
		t.Errorf("message: got %q, want substring %q", gatewayErr.Message, "internal server error")
	}
}

// TestTransportFetchModels500BodyCapped sends an error body larger than
// maxErrorBodyBytes and verifies the LimitReader ceiling holds, so a
// misbehaving gateway cannot force an unbounded GatewayError.Message
// allocation. Locks the cap as a regression-prevention invariant.
func TestTransportFetchModels500BodyCapped(t *testing.T) {
	oversized := bytes.Repeat([]byte("A"), maxErrorBodyBytes+1024)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(500)
		_, _ = w.Write(oversized)
	}))
	defer server.Close()

	transport := NewTransport(server.URL, 30*time.Second)
	_, err := transport.FetchModels(context.Background())
	if err == nil {
		t.Fatal("expected error")
	}
	gatewayErr, ok := err.(*GatewayError)
	if !ok {
		t.Fatalf("expected GatewayError, got %T", err)
	}
	if len(gatewayErr.Message) > maxErrorBodyBytes {
		t.Errorf("message length: got %d, want <= %d (LimitReader cap)", len(gatewayErr.Message), maxErrorBodyBytes)
	}
}

// TestSendChatStreamSurfacesScannerErr drives a server that hijacks the
// connection, writes a partial SSE line (no terminating newline) along with
// a chunked-transfer header, and then closes the underlying TCP connection.
// The bufio scanner inside SendChatStream must surface the resulting read
// error via the channel as a ChatChunkOrError{Err: ...} rather than letting
// the stream look like a clean close.
func TestSendChatStreamSurfacesScannerErr(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Hijack the connection so we can write malformed/truncated bytes
		// directly. After writing a partial chunk the underlying TCP
		// connection is closed, which triggers an unexpected EOF inside the
		// scanner once it has consumed the partial chunk.
		hj, ok := w.(http.Hijacker)
		if !ok {
			t.Fatalf("ResponseWriter does not support hijack")
		}
		conn, bufrw, err := hj.Hijack()
		if err != nil {
			t.Fatalf("hijack: %v", err)
		}
		// Send the response head with chunked encoding; we will then write a
		// chunk that *claims* more bytes than we actually deliver before
		// closing. This guarantees the client-side bufio.Scanner observes an
		// unexpected EOF rather than a clean close.
		fmt.Fprint(bufrw, "HTTP/1.1 200 OK\r\n")
		fmt.Fprint(bufrw, "Content-Type: text/event-stream\r\n")
		fmt.Fprint(bufrw, "Transfer-Encoding: chunked\r\n")
		fmt.Fprint(bufrw, "\r\n")
		// First chunk: a complete, valid JSON SSE event terminated with
		// "\n\n". The scanner consumes one line, the JSON parses, the
		// chunk lands on the channel, and the loop continues. The second
		// chunk header promises 100 bytes but we only write 13 before
		// slamming the connection shut. The Go HTTP client surfaces this
		// truncation as io.ErrUnexpectedEOF, which bufio.Scanner reports
		// via scanner.Err() (since it is non-EOF). This is the exact
		// surface the security-fix agent added handling for.
		role := "assistant"
		content := "first"
		first := fmt.Sprintf(`{"id":"c1","model":"gpt-4","choices":[{"index":0,"delta":{"role":%q,"content":%q}}]}`, role, content)
		ssePayload := fmt.Sprintf("data: %s\n\n", first)
		fmt.Fprintf(bufrw, "%x\r\n", len(ssePayload))
		fmt.Fprint(bufrw, ssePayload)
		fmt.Fprint(bufrw, "\r\n")
		// Promise a second 100-byte chunk but only deliver 7 bytes, then
		// close. The Go chunked-transfer reader surfaces this truncation
		// as io.ErrUnexpectedEOF on the next read after the buffered bytes
		// are consumed. The bytes themselves do NOT begin with "data: " so
		// the scanner skips them; the next Scan call observes the
		// truncation as a non-EOF error and surfaces it via scanner.Err().
		fmt.Fprint(bufrw, "64\r\n")  // 0x64 = 100 in hex (promise 100 bytes)
		fmt.Fprint(bufrw, "garbage") // 7 bytes, no "data: " prefix, no terminator
		bufrw.Flush()
		conn.Close()
	}))
	defer server.Close()

	transport := NewTransport(server.URL, 10*time.Second)
	req := &ChatRequest{Model: "gpt-4", Messages: []ChatMessage{{Role: RoleUser, Content: "Hi"}}}

	ch, err := transport.SendChatStream(context.Background(), req, "", nil)
	if err != nil {
		t.Fatalf("SendChatStream: %v", err)
	}

	var sawErr bool
	timeout := time.After(2 * time.Second)
	for {
		select {
		case item, ok := <-ch:
			if !ok {
				goto done
			}
			if item.Err != nil {
				sawErr = true
				if !strings.Contains(item.Err.Error(), "stream read error") {
					t.Errorf("expected wrapped scanner error, got %v", item.Err)
				}
			}
		case <-timeout:
			t.Fatal("stream never closed")
		}
	}
done:
	if !sawErr {
		t.Error("expected ChatChunkOrError with non-nil Err for truncated stream; got clean close")
	}
}

// TestSendChatStreamRejectsMalformed402Body verifies that a 402 with a
// non-JSON body returns *GatewayError mentioning "parse 402 body" rather
// than silently producing a zero-value PaymentRequiredError that the SDK
// could not act on. This exercises the body-parse wrapper added by the
// security fix agent.
func TestSendChatStreamRejectsMalformed402Body(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain")
		w.WriteHeader(402)
		_, _ = w.Write([]byte("not-json-at-all<<<"))
	}))
	defer server.Close()

	transport := NewTransport(server.URL, 10*time.Second)
	req := &ChatRequest{Model: "gpt-4", Messages: []ChatMessage{{Role: RoleUser, Content: "Hi"}}}

	_, err := transport.SendChatStream(context.Background(), req, "", nil)
	if err == nil {
		t.Fatal("expected error for malformed 402 body, got nil")
	}
	// Must surface as GatewayError with the parse-error wrapper, not as a
	// PaymentRequiredError carrying a zero-value PaymentRequired struct.
	var gwErr *GatewayError
	if !errors.As(err, &gwErr) {
		t.Fatalf("expected *GatewayError, got %T: %v", err, err)
	}
	if gwErr.Status != 402 {
		t.Errorf("status: got %d, want 402", gwErr.Status)
	}
	if !strings.Contains(gwErr.Message, "parse 402 body") {
		t.Errorf("expected message to contain 'parse 402 body', got %q", gwErr.Message)
	}
	if _, isPRE := err.(*PaymentRequiredError); isPRE {
		t.Error("malformed 402 body must NOT be returned as PaymentRequiredError")
	}
}

func TestTransportBaseURLTrailingSlash(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(ChatResponse{ID: "test"})
	}))
	defer server.Close()

	transport := NewTransport(server.URL+"/", 10*time.Second)
	if transport.baseURL != server.URL {
		t.Errorf("trailing slash not trimmed: got %q", transport.baseURL)
	}
}
