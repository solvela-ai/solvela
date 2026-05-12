package solvela

import "time"

// DefaultMaxPaymentAmount is the default cap on a single payment, in atomic
// USDC units (10 USDC = 10_000_000). This is high enough to cover any
// reasonable single LLM call, but low enough to block obvious wallet drains
// when a misbehaving or malicious gateway returns a wildly inflated amount.
//
// Callers that need a higher per-request cap must override this explicitly via
// [WithMaxPaymentAmount]. Callers that want no cap at all should pass the
// largest uint64 value.
const DefaultMaxPaymentAmount uint64 = 10_000_000

// ClientConfig holds all configuration for a Solvela client.
type ClientConfig struct {
	GatewayURL         string
	RPCURL             string
	PreferEscrow       bool
	Timeout            time.Duration
	ExpectedRecipient  string
	MaxPaymentAmount   *uint64
	EnableCache        bool
	EnableSessions     bool
	SessionTTL         time.Duration
	EnableQualityCheck bool
	MaxQualityRetries  int
	FreeFallbackModel  string
	// BalancePollInterval, when non-zero, instructs [NewClient] to start a
	// background [BalanceMonitor] that periodically refreshes the client's
	// last-known wallet balance. The free-fallback-model guard in
	// [SolvelaClient.Chat] / [SolvelaClient.ChatStream] reads that value to
	// detect a depleted wallet and substitute [ClientConfig.FreeFallbackModel].
	// Without a non-zero interval and a [BalanceFetcher], lastBalance is
	// never populated and the guard is dormant. Set via [WithBalanceMonitor].
	BalancePollInterval time.Duration
	// BalanceFetcher returns the wallet's current balance for the background
	// [BalanceMonitor]. Required when [BalancePollInterval] is non-zero. Set
	// via [WithBalanceMonitor].
	BalanceFetcher func() (float64, error)
}

// DefaultConfig returns a ClientConfig with sensible defaults.
//
// Security defaults:
//   - GatewayURL points to the production HTTPS endpoint.
//   - MaxPaymentAmount is set to [DefaultMaxPaymentAmount] (10 USDC atomic) to
//     prevent wallet drains from a misconfigured or malicious gateway.
func DefaultConfig() ClientConfig {
	defaultMax := DefaultMaxPaymentAmount
	return ClientConfig{
		GatewayURL:        "https://api.solvela.ai",
		RPCURL:            "https://api.mainnet-beta.solana.com",
		Timeout:           180 * time.Second,
		SessionTTL:        1800 * time.Second,
		MaxQualityRetries: 1,
		MaxPaymentAmount:  &defaultMax,
	}
}

// Option is a functional option for configuring ClientConfig.
type Option func(*ClientConfig)

// WithGatewayURL sets the gateway URL.
func WithGatewayURL(url string) Option { return func(c *ClientConfig) { c.GatewayURL = url } }

// WithRPCURL sets the Solana RPC URL.
func WithRPCURL(url string) Option { return func(c *ClientConfig) { c.RPCURL = url } }

// WithTimeout sets the request timeout.
func WithTimeout(d time.Duration) Option { return func(c *ClientConfig) { c.Timeout = d } }

// WithExpectedRecipient sets the expected payment recipient for verification.
func WithExpectedRecipient(r string) Option {
	return func(c *ClientConfig) { c.ExpectedRecipient = r }
}

// WithMaxPaymentAmount sets the maximum payment amount in atomic units.
func WithMaxPaymentAmount(max uint64) Option {
	return func(c *ClientConfig) { m := max; c.MaxPaymentAmount = &m }
}

// WithCache enables or disables response caching.
func WithCache(enable bool) Option { return func(c *ClientConfig) { c.EnableCache = enable } }

// WithSessions enables or disables session tracking.
func WithSessions(enable bool) Option { return func(c *ClientConfig) { c.EnableSessions = enable } }

// WithSessionTTL sets the session time-to-live.
func WithSessionTTL(d time.Duration) Option { return func(c *ClientConfig) { c.SessionTTL = d } }

// WithQualityCheck enables or disables quality checking.
func WithQualityCheck(enable bool) Option {
	return func(c *ClientConfig) { c.EnableQualityCheck = enable }
}

// WithMaxQualityRetries sets the maximum number of quality retries.
func WithMaxQualityRetries(n int) Option {
	return func(c *ClientConfig) { c.MaxQualityRetries = n }
}

// WithFreeFallbackModel sets the free model to fall back to.
func WithFreeFallbackModel(model string) Option {
	return func(c *ClientConfig) { c.FreeFallbackModel = model }
}

// WithBalanceMonitor enables a background goroutine that polls the wallet
// balance every interval using fetcher and feeds the result into the
// client's last-known balance cache. This is required for the free-fallback
// model guard configured by [WithFreeFallbackModel] to fire — without a
// monitor the cache stays empty and the guard is dormant.
//
// The monitor is opt-in by design: NewClient does not auto-start network
// pollers, so callers that do not care about balance-aware fallback pay no
// background-RPC tax. Call [SolvelaClient.Close] to stop the monitor when
// the client is no longer needed.
//
// interval and fetcher must both be non-zero/non-nil; passing zero or nil
// is a no-op for safety.
func WithBalanceMonitor(interval time.Duration, fetcher func() (float64, error)) Option {
	return func(c *ClientConfig) {
		c.BalancePollInterval = interval
		c.BalanceFetcher = fetcher
	}
}
