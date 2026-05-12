package solvela

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/url"
	"strconv"
	"strings"
	"sync"
)

// validateGatewayURL rejects plaintext http:// gateway URLs unless the host is
// a recognized loopback address. Sending payments over plaintext to a remote
// host would leak the Payment-Signature header and expose the request body to
// any on-path observer.
func validateGatewayURL(raw string) error {
	u, err := url.Parse(raw)
	if err != nil {
		return fmt.Errorf("invalid gateway URL: %w", err)
	}
	if u.Scheme != "http" && u.Scheme != "https" {
		return fmt.Errorf("gateway URL must use http or https, got %q", u.Scheme)
	}
	if u.Scheme == "http" {
		host := u.Hostname()
		if host != "localhost" && host != "127.0.0.1" && host != "::1" {
			return fmt.Errorf("gateway URL must use https:// for non-local endpoints, got %s://%s", u.Scheme, host)
		}
	}
	return nil
}

// SolvelaClient is the main client for interacting with the Solvela gateway.
type SolvelaClient struct {
	config       ClientConfig
	wallet       *Wallet
	signer       Signer
	transport    *Transport
	cache        *ResponseCache
	sessionStore *SessionStore
	sessionSalt  []byte

	balanceMu      sync.RWMutex
	lastBalance    float64
	lastBalanceSet bool

	balanceMonitor *BalanceMonitor
}

// NewClient creates a new SolvelaClient with functional options.
//
// The gateway URL is validated: plaintext http:// is only allowed for the
// loopback hosts localhost, 127.0.0.1, and ::1. All other endpoints must use
// https://. Misconfigured URLs return a [ClientError]; the client itself is
// returned only on success.
func NewClient(wallet *Wallet, signer Signer, opts ...Option) (*SolvelaClient, error) {
	cfg := DefaultConfig()
	for _, opt := range opts {
		opt(&cfg)
	}

	if err := validateGatewayURL(cfg.GatewayURL); err != nil {
		return nil, &ClientError{Message: err.Error()}
	}

	salt, err := newSessionSalt()
	if err != nil {
		return nil, &ClientError{Message: fmt.Sprintf("failed to seed session salt: %v", err)}
	}

	c := &SolvelaClient{
		config:      cfg,
		wallet:      wallet,
		signer:      signer,
		transport:   NewTransport(cfg.GatewayURL, cfg.Timeout),
		sessionSalt: salt,
	}
	if cfg.EnableCache {
		c.cache = NewResponseCache()
	}
	if cfg.EnableSessions {
		c.sessionStore = NewSessionStore(cfg.SessionTTL)
	}
	// Opt-in: only spin up a background poller when the caller has asked for
	// one via [WithBalanceMonitor]. Auto-starting would be a surprising RPC
	// tax for the common case where the free-fallback guard is unused.
	if cfg.BalancePollInterval > 0 && cfg.BalanceFetcher != nil {
		monitor := NewBalanceMonitor(cfg.BalanceFetcher, cfg.BalancePollInterval, nil, nil)
		monitor.SetOnPoll(func(b float64) { c.setLastBalance(b) })
		c.balanceMonitor = monitor
		monitor.Start()
	}
	return c, nil
}

// Close releases resources held by the client. It stops the background
// balance monitor (if one was started via [WithBalanceMonitor]). Safe to
// call multiple times. After Close the client should not be reused.
func (c *SolvelaClient) Close() {
	if c.balanceMonitor != nil {
		c.balanceMonitor.Stop()
	}
}

// setLastBalance stores the latest balance under the balance lock.
func (c *SolvelaClient) setLastBalance(b float64) {
	c.balanceMu.Lock()
	c.lastBalance = b
	c.lastBalanceSet = true
	c.balanceMu.Unlock()
}

// getLastBalance returns the most recent balance and whether one has been
// recorded. Reads under a shared lock so concurrent Chat / ChatStream calls
// do not race the BalanceMonitor goroutine.
func (c *SolvelaClient) getLastBalance() (float64, bool) {
	c.balanceMu.RLock()
	defer c.balanceMu.RUnlock()
	return c.lastBalance, c.lastBalanceSet
}

// Chat sends a non-streaming chat request with automatic payment, caching,
// session tracking, and quality checking.
func (c *SolvelaClient) Chat(ctx context.Context, request *ChatRequest) (*ChatResponse, error) {
	model := request.Model

	// Step 1: Balance guard
	if balance, ok := c.getLastBalance(); ok && balance == 0 && c.config.FreeFallbackModel != "" {
		model = c.config.FreeFallbackModel
	}

	// Step 2: Session lookup
	var sessionID string
	if c.sessionStore != nil {
		sessionID = DeriveSessionIDWithSalt(c.sessionSalt, request.Messages)
		info := c.sessionStore.GetOrCreate(sessionID, model)
		if model == request.Model { // not overridden by balance guard
			model = info.Model
		}
	}

	// Step 3: Cache check (after model finalization)
	var cacheKey uint64
	if c.cache != nil {
		cacheKey = CacheKey(model, request.Messages)
		if cached, ok := c.cache.Get(cacheKey); ok {
			return &cached, nil
		}
	}

	// Build effective request
	effectiveReq := &ChatRequest{
		Model:       model,
		Messages:    request.Messages,
		MaxTokens:   request.MaxTokens,
		Temperature: request.Temperature,
		TopP:        request.TopP,
		Stream:      false,
		Tools:       request.Tools,
		ToolChoice:  request.ToolChoice,
	}

	// Step 4: Send request
	response, err := c.sendWithPayment(ctx, effectiveReq, nil)
	if err != nil {
		return nil, err
	}

	// Step 5: Quality check + retry. After the retry loop exits we re-evaluate
	// the final response: a still-degraded result must surface as
	// [QualityDegradedError] rather than be silently returned. Caching a
	// degraded response would replay it forever; skip the cache write on the
	// error path.
	if c.config.EnableQualityCheck {
		var lastReason DegradedReason
		for i := 0; i <= c.config.MaxQualityRetries; i++ {
			if len(response.Choices) == 0 {
				lastReason = DegradedEmptyContent
			} else {
				lastReason = CheckDegraded(response.Choices[0].Message.Content)
			}
			if lastReason == "" {
				break
			}
			if i == c.config.MaxQualityRetries {
				// Exhausted retries; the response is still degraded. Do not
				// cache and do not record the session — return the typed
				// error so callers can branch on it.
				return nil, &QualityDegradedError{Reason: lastReason, Response: response}
			}
			response, err = c.sendWithPayment(ctx, effectiveReq, map[string]string{
				"X-Solvela-Retry-Reason": "degraded",
			})
			if err != nil {
				return nil, err
			}
		}
	}

	// Step 6: Cache store
	if c.cache != nil {
		c.cache.Put(cacheKey, *response)
	}

	// Step 7: Session update
	if c.sessionStore != nil && sessionID != "" {
		hash := CacheKey(model, request.Messages)
		c.sessionStore.RecordRequest(sessionID, hash)
	}

	return response, nil
}

// ChatStream sends a streaming chat request.
//
// Payment handshake: the streaming endpoint still returns a 402 on the initial
// POST when payment is required, so this method runs the same handshake as
// [SolvelaClient.Chat] before opening the stream — a non-streaming request is
// issued first to discover the price, the configured [Signer] (if any) signs
// the payment, and the streaming request is then re-sent with the
// Payment-Signature header attached. If no [Signer] is configured and a 402 is
// returned, this method surfaces a [PaymentRequiredError] rather than silently
// streaming an unauthenticated request.
func (c *SolvelaClient) ChatStream(ctx context.Context, request *ChatRequest) (<-chan ChatChunkOrError, error) {
	model := request.Model

	// Step 1: Balance guard
	if balance, ok := c.getLastBalance(); ok && balance == 0 && c.config.FreeFallbackModel != "" {
		model = c.config.FreeFallbackModel
	}

	// Step 2: Session lookup
	var sessionID string
	if c.sessionStore != nil {
		sessionID = DeriveSessionIDWithSalt(c.sessionSalt, request.Messages)
		info := c.sessionStore.GetOrCreate(sessionID, model)
		if model == request.Model {
			model = info.Model
		}
	}

	effectiveReq := &ChatRequest{
		Model:    model,
		Messages: request.Messages,
		Stream:   true,
	}

	// Step 3: Resolve payment (if any) before opening the stream.
	//
	// resolvePaymentSignature first issues a non-streaming probe so the gateway
	// can return 402 (price discovery) or 200 (no payment required, full
	// response). When the probe returns 200 the body IS already a complete
	// completion: opening a second streaming connection would cost the caller
	// a second paid request and discard the first. Reuse the probe response
	// by synthesizing a single-chunk SSE-like stream instead.
	sig, probeResp, err := c.resolvePaymentSignature(ctx, effectiveReq)
	if err != nil {
		return nil, err
	}
	if probeResp != nil {
		// Probe returned 200 — turn the ChatResponse into a one-shot channel
		// and avoid the second paid round trip. Session recording happens
		// synchronously here because there is no upstream goroutine to defer
		// it to.
		if c.sessionStore != nil && sessionID != "" {
			hash := CacheKey(model, request.Messages)
			c.sessionStore.RecordRequest(sessionID, hash)
		}
		return synthesizeStreamFromResponse(probeResp), nil
	}

	// Drive the upstream goroutine with a child context so cancellation here
	// (consumer abandons the stream early) propagates immediately. Without
	// this the transport goroutine would block forever on `ch <- chunk` once
	// the consumer stopped reading, leaking a goroutine plus the underlying
	// HTTP body.
	streamCtx, cancel := context.WithCancel(ctx)
	ch, err := c.transport.SendChatStream(streamCtx, effectiveReq, sig, nil)
	if err != nil {
		cancel()
		return nil, err
	}

	wrappedCh := make(chan ChatChunkOrError)
	go func() {
		defer close(wrappedCh)
		// Cancelling here unblocks the transport goroutine if the consumer
		// abandons wrappedCh before the stream completes.
		defer cancel()
		for {
			select {
			case <-ctx.Done():
				// Consumer's parent context cancelled. Cancel the child to
				// unblock the upstream goroutine; the deferred cancel above
				// would also do this, but doing it eagerly avoids holding
				// onto the body any longer than needed.
				cancel()
				// Drain remaining items so the upstream goroutine can return.
				go func() {
					for range ch {
					}
				}()
				return
			case item, ok := <-ch:
				if !ok {
					if c.sessionStore != nil && sessionID != "" {
						hash := CacheKey(model, request.Messages)
						c.sessionStore.RecordRequest(sessionID, hash)
					}
					return
				}
				select {
				case wrappedCh <- item:
				case <-ctx.Done():
					cancel()
					go func() {
						for range ch {
						}
					}()
					return
				}
			}
		}
	}()
	return wrappedCh, nil
}

// Models retrieves available models from the gateway.
func (c *SolvelaClient) Models(ctx context.Context) ([]ModelInfo, error) {
	return c.transport.FetchModels(ctx)
}

// LastKnownBalance returns the most recently known wallet balance and a
// boolean reporting whether one has been recorded yet. The boolean
// distinguishes "balance is zero" from "we have not polled yet" — the
// former triggers the free-fallback-model guard, the latter does not. The
// background poller configured via [WithBalanceMonitor] is responsible for
// populating this value; without that option the second return is always
// false.
func (c *SolvelaClient) LastKnownBalance() (float64, bool) {
	return c.getLastBalance()
}

// String returns a debug-safe representation with redacted secrets.
func (c *SolvelaClient) String() string {
	return fmt.Sprintf("SolvelaClient(gateway=%s, wallet=REDACTED)", c.config.GatewayURL)
}

func (c *SolvelaClient) sendWithPayment(ctx context.Context, request *ChatRequest, extraHeaders map[string]string) (*ChatResponse, error) {
	// Force non-streaming for the price-discovery probe; we will stream at the
	// caller layer if needed. This is also the live request when the caller
	// invokes Chat (non-streaming).
	probeReq := *request
	probeReq.Stream = false

	result, err := c.transport.SendChat(ctx, &probeReq, "", extraHeaders)
	if err != nil {
		return nil, err
	}

	if result.PaymentRequired == nil {
		return result.Response, nil
	}

	sig, err := c.signPaymentRequired(ctx, result.PaymentRequired)
	if err != nil {
		return nil, err
	}

	result, err = c.transport.SendChat(ctx, &probeReq, sig, extraHeaders)
	if err != nil {
		return nil, err
	}
	if result.PaymentRequired != nil {
		return nil, &PaymentRejectedError{Reason: "payment rejected after signing"}
	}
	return result.Response, nil
}

// resolvePaymentSignature issues a non-streaming probe to discover whether the
// gateway requires payment for the given request, and signs the payment if so.
// Returns:
//   - sig:        the encoded Payment-Signature header value, or "" if no
//                 payment is needed (or the probe already returned a fully
//                 resolved 200 response).
//   - probeResp:  non-nil when the probe returned 200 with a complete
//                 ChatResponse. Callers (ChatStream) MUST reuse this body
//                 instead of issuing a second request — opening a streaming
//                 follow-up would charge the caller twice and discard the
//                 already-paid completion.
//   - err:        any error encountered.
//
// The probe shares the same handshake logic as non-streaming Chat — the
// gateway returns 402 on the initial POST whether or not the response is
// streamed, so the same flow is safe to reuse.
func (c *SolvelaClient) resolvePaymentSignature(ctx context.Context, request *ChatRequest) (string, *ChatResponse, error) {
	probeReq := *request
	probeReq.Stream = false

	result, err := c.transport.SendChat(ctx, &probeReq, "", nil)
	if err != nil {
		return "", nil, err
	}
	if result.PaymentRequired == nil {
		// No payment required; surface the already-resolved response so the
		// streaming caller can replay it without a second paid request.
		return "", result.Response, nil
	}
	sig, err := c.signPaymentRequired(ctx, result.PaymentRequired)
	if err != nil {
		return "", nil, err
	}
	return sig, nil, nil
}

// synthesizeStreamFromResponse converts a fully-resolved [ChatResponse] into a
// closed [ChatChunkOrError] channel that emits one chunk per choice and then
// closes. This lets [SolvelaClient.ChatStream] reuse a 200 response from the
// price-discovery probe instead of issuing a second paid request just to get
// the bytes in streaming form.
func synthesizeStreamFromResponse(resp *ChatResponse) <-chan ChatChunkOrError {
	ch := make(chan ChatChunkOrError, len(resp.Choices)+1)
	for _, choice := range resp.Choices {
		// Snapshot per-iteration so each chunk owns its own role/content
		// pointers; addressing the loop variable directly would alias every
		// emitted chunk to the same backing storage.
		role := choice.Message.Role
		content := choice.Message.Content
		chunk := &ChatChunk{
			ID:      resp.ID,
			Object:  resp.Object,
			Created: resp.Created,
			Model:   resp.Model,
			Choices: []ChatChunkChoice{
				{
					Index: choice.Index,
					Delta: ChatDelta{
						Role:    &role,
						Content: &content,
					},
					FinishReason: choice.FinishReason,
				},
			},
		}
		ch <- ChatChunkOrError{Chunk: chunk}
	}
	close(ch)
	return ch
}

// signPaymentRequired runs the validation + signer pipeline for a 402 response
// and returns the base64-encoded Payment-Signature header value.
func (c *SolvelaClient) signPaymentRequired(ctx context.Context, pr *PaymentRequired) (string, error) {
	if c.signer == nil {
		return "", &PaymentRequiredError{PaymentRequired: *pr}
	}
	accepted := c.findCompatibleScheme(pr)
	if accepted == nil {
		return "", &PaymentRequiredError{PaymentRequired: *pr}
	}
	if err := c.validatePayment(accepted); err != nil {
		return "", err
	}
	if err := c.validateResourceOrigin(pr.Resource.URL); err != nil {
		return "", err
	}

	amountAtomic, err := parseAmount(accepted.Amount)
	if err != nil {
		return "", err
	}
	payload, err := c.signer.SignPayment(ctx, amountAtomic, accepted.PayTo, pr.Resource, *accepted)
	if err != nil {
		return "", err
	}
	payloadJSON, err := json.Marshal(payload)
	if err != nil {
		return "", fmt.Errorf("encode payment payload: %w", err)
	}
	return base64.StdEncoding.EncodeToString(payloadJSON), nil
}

// isCompatibleAccept reports whether an accepted scheme entry is one we can
// safely sign and pay. We require:
//   - scheme is "exact" or "escrow"
//   - network is the Solana mainnet identifier we support
//   - asset is the canonical USDC mint (no surprise SPL tokens)
//
// Asset and network must be validated here to prevent a malicious gateway from
// tricking the SDK into signing a transfer of a different token, or paying on
// a different chain.
func isCompatibleAccept(a PaymentAccept, scheme Scheme) bool {
	return a.Scheme == scheme && a.Network == SolanaNetwork && a.Asset == USDCMint
}

func (c *SolvelaClient) findCompatibleScheme(pr *PaymentRequired) *PaymentAccept {
	for i := range pr.Accepts {
		if isCompatibleAccept(pr.Accepts[i], SchemeExact) {
			return &pr.Accepts[i]
		}
	}
	for i := range pr.Accepts {
		if isCompatibleAccept(pr.Accepts[i], SchemeEscrow) {
			return &pr.Accepts[i]
		}
	}
	return nil
}

// validateResourceOrigin rejects a 402 Resource.URL whose origin (scheme+host)
// does not match the configured gateway. The Resource.URL is server-supplied
// and may be embedded into the signed payment payload (e.g., as a memo field),
// so we cannot let a rogue gateway substitute a foreign URL and trick the
// signer into binding payment metadata to an unintended endpoint.
//
// An empty Resource.URL is permitted: not every gateway populates it, and
// rejecting on absence would break legitimate flows. Only mismatches are
// fatal.
func (c *SolvelaClient) validateResourceOrigin(resourceURL string) error {
	if resourceURL == "" {
		return nil
	}
	ru, err := url.Parse(resourceURL)
	if err != nil {
		return &ClientError{Message: fmt.Sprintf("invalid resource URL %q: %v", resourceURL, err)}
	}
	gu, err := url.Parse(c.config.GatewayURL)
	if err != nil {
		return &ClientError{Message: fmt.Sprintf("invalid gateway URL %q: %v", c.config.GatewayURL, err)}
	}
	if ru.Scheme != gu.Scheme || ru.Host != gu.Host {
		return &ClientError{Message: fmt.Sprintf("resource URL origin does not match configured gateway: resource=%s://%s gateway=%s://%s", ru.Scheme, ru.Host, gu.Scheme, gu.Host)}
	}
	return nil
}

func (c *SolvelaClient) validatePayment(accepted *PaymentAccept) error {
	if c.config.ExpectedRecipient != "" && accepted.PayTo != c.config.ExpectedRecipient {
		return &RecipientMismatchError{Expected: c.config.ExpectedRecipient, Actual: accepted.PayTo}
	}
	// Defense-in-depth: re-verify network and asset here in case a caller bypassed
	// findCompatibleScheme.
	if accepted.Network != SolanaNetwork {
		return &ClientError{Message: fmt.Sprintf("unsupported network: %q (expected %q)", accepted.Network, SolanaNetwork)}
	}
	if accepted.Asset != USDCMint {
		return &ClientError{Message: fmt.Sprintf("unsupported asset: %q (expected USDC mint %q)", accepted.Asset, USDCMint)}
	}
	// Fail closed when no max is configured. A nil cap means "unbounded", which
	// turns the SDK into a footgun if a misconfigured gateway returns an
	// inflated amount; better to refuse than to drain the wallet.
	if c.config.MaxPaymentAmount == nil {
		return &ClientError{Message: "client misconfigured: MaxPaymentAmount must be set (use WithMaxPaymentAmount or DefaultConfig)"}
	}
	amount, err := parseAmount(accepted.Amount)
	if err != nil {
		return err
	}
	if amount > *c.config.MaxPaymentAmount {
		return &AmountExceedsMaxError{Amount: amount, MaxAmount: *c.config.MaxPaymentAmount}
	}
	return nil
}

// parseAmount parses a 402-supplied atomic-units amount string. It rejects
// empty, non-numeric, overflowing, and zero values. Zero is rejected because
// no legitimate 402 says "pay zero", and silently treating bad input as zero
// would let a malicious gateway slip past the MaxPaymentAmount cap.
func parseAmount(s string) (uint64, error) {
	trimmed := strings.TrimSpace(s)
	if trimmed == "" {
		return 0, &ClientError{Message: fmt.Sprintf("invalid payment amount %q: empty", s)}
	}
	n, err := strconv.ParseUint(trimmed, 10, 64)
	if err != nil {
		return 0, &ClientError{Message: fmt.Sprintf("invalid payment amount %q: %v", s, err)}
	}
	if n == 0 {
		return 0, &ClientError{Message: fmt.Sprintf("invalid payment amount %q: must be greater than zero", s)}
	}
	return n, nil
}
