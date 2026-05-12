package solvela

import (
	"testing"
	"time"
)

func TestDefaultConfig(t *testing.T) {
	cfg := DefaultConfig()

	if cfg.GatewayURL != "https://api.solvela.ai" {
		t.Errorf("GatewayURL: got %q", cfg.GatewayURL)
	}
	if cfg.RPCURL != "https://api.mainnet-beta.solana.com" {
		t.Errorf("RPCURL: got %q", cfg.RPCURL)
	}
	if cfg.Timeout != 180*time.Second {
		t.Errorf("Timeout: got %v", cfg.Timeout)
	}
	if cfg.SessionTTL != 1800*time.Second {
		t.Errorf("SessionTTL: got %v", cfg.SessionTTL)
	}
	if cfg.MaxQualityRetries != 1 {
		t.Errorf("MaxQualityRetries: got %d", cfg.MaxQualityRetries)
	}
	if cfg.PreferEscrow {
		t.Error("PreferEscrow should default to false")
	}
	if cfg.EnableCache {
		t.Error("EnableCache should default to false")
	}
	if cfg.EnableSessions {
		t.Error("EnableSessions should default to false")
	}
	if cfg.EnableQualityCheck {
		t.Error("EnableQualityCheck should default to false")
	}
	if cfg.MaxPaymentAmount == nil {
		t.Error("MaxPaymentAmount should default to a non-nil cap (security)")
	} else if *cfg.MaxPaymentAmount != DefaultMaxPaymentAmount {
		t.Errorf("MaxPaymentAmount default: got %d, want %d", *cfg.MaxPaymentAmount, DefaultMaxPaymentAmount)
	}
	if cfg.ExpectedRecipient != "" {
		t.Error("ExpectedRecipient should default to empty")
	}
	if cfg.FreeFallbackModel != "" {
		t.Error("FreeFallbackModel should default to empty")
	}
}

func TestWithGatewayURL(t *testing.T) {
	cfg := DefaultConfig()
	WithGatewayURL("https://gateway.example.com")(&cfg)
	if cfg.GatewayURL != "https://gateway.example.com" {
		t.Errorf("got %q", cfg.GatewayURL)
	}
}

func TestWithRPCURL(t *testing.T) {
	cfg := DefaultConfig()
	WithRPCURL("https://rpc.example.com")(&cfg)
	if cfg.RPCURL != "https://rpc.example.com" {
		t.Errorf("got %q", cfg.RPCURL)
	}
}

func TestWithTimeout(t *testing.T) {
	cfg := DefaultConfig()
	WithTimeout(60 * time.Second)(&cfg)
	if cfg.Timeout != 60*time.Second {
		t.Errorf("got %v", cfg.Timeout)
	}
}

func TestWithExpectedRecipient(t *testing.T) {
	cfg := DefaultConfig()
	WithExpectedRecipient("recipient123")(&cfg)
	if cfg.ExpectedRecipient != "recipient123" {
		t.Errorf("got %q", cfg.ExpectedRecipient)
	}
}

func TestWithMaxPaymentAmount(t *testing.T) {
	cfg := DefaultConfig()
	WithMaxPaymentAmount(5000)(&cfg)
	if cfg.MaxPaymentAmount == nil || *cfg.MaxPaymentAmount != 5000 {
		t.Errorf("got %v", cfg.MaxPaymentAmount)
	}
}

func TestWithCache(t *testing.T) {
	cfg := DefaultConfig()
	WithCache(true)(&cfg)
	if !cfg.EnableCache {
		t.Error("expected EnableCache to be true")
	}
}

func TestWithSessions(t *testing.T) {
	cfg := DefaultConfig()
	WithSessions(true)(&cfg)
	if !cfg.EnableSessions {
		t.Error("expected EnableSessions to be true")
	}
}

func TestWithSessionTTL(t *testing.T) {
	cfg := DefaultConfig()
	WithSessionTTL(600 * time.Second)(&cfg)
	if cfg.SessionTTL != 600*time.Second {
		t.Errorf("got %v", cfg.SessionTTL)
	}
}

func TestWithQualityCheck(t *testing.T) {
	cfg := DefaultConfig()
	WithQualityCheck(true)(&cfg)
	if !cfg.EnableQualityCheck {
		t.Error("expected EnableQualityCheck to be true")
	}
}

func TestWithMaxQualityRetries(t *testing.T) {
	cfg := DefaultConfig()
	WithMaxQualityRetries(3)(&cfg)
	if cfg.MaxQualityRetries != 3 {
		t.Errorf("got %d", cfg.MaxQualityRetries)
	}
}

func TestWithFreeFallbackModel(t *testing.T) {
	cfg := DefaultConfig()
	WithFreeFallbackModel("free/gemini-2.0-flash")(&cfg)
	if cfg.FreeFallbackModel != "free/gemini-2.0-flash" {
		t.Errorf("got %q", cfg.FreeFallbackModel)
	}
}

func TestMultipleOptions(t *testing.T) {
	cfg := DefaultConfig()
	opts := []Option{
		WithGatewayURL("https://gw.test"),
		WithTimeout(30 * time.Second),
		WithCache(true),
		WithSessions(true),
		WithMaxPaymentAmount(10000),
	}
	for _, opt := range opts {
		opt(&cfg)
	}

	if cfg.GatewayURL != "https://gw.test" {
		t.Errorf("GatewayURL: got %q", cfg.GatewayURL)
	}
	if cfg.Timeout != 30*time.Second {
		t.Errorf("Timeout: got %v", cfg.Timeout)
	}
	if !cfg.EnableCache {
		t.Error("EnableCache should be true")
	}
	if !cfg.EnableSessions {
		t.Error("EnableSessions should be true")
	}
	if cfg.MaxPaymentAmount == nil || *cfg.MaxPaymentAmount != 10000 {
		t.Errorf("MaxPaymentAmount: got %v", cfg.MaxPaymentAmount)
	}
}
