package solvela

// Cross-SDK 402-parse smoke (channel plan PR-B, invariant 12 / HALT 12).
//
// Parses the LIVE gateway 402 challenge fixture shared with the TS and Python
// SDK suites (crates/gateway/tests/fixtures/chat_402_challenge.json, pinned
// byte-shape-identical to the real route by the gateway integration test
// x402_challenge_smoke_tests::live_402_body_matches_cross_sdk_fixture).
//
// PaymentAccept.UnmarshalJSON rejects the ENTIRE 402 on any unknown scheme, so
// a gateway that ever advertised a new scheme (e.g. "channel") in accepts[]
// would break every deployed paid call at parse time — the memorialized
// cross-repo wire-drift failure mode. This smoke is the standing tripwire.

import (
	"encoding/json"
	"os"
	"testing"
)

const x402FixturePath = "../../crates/gateway/tests/fixtures/chat_402_challenge.json"

func TestLive402FixtureParses(t *testing.T) {
	data, err := os.ReadFile(x402FixturePath)
	if err != nil {
		t.Fatalf("read shared 402 fixture: %v", err)
	}
	var pr PaymentRequired
	if err := json.Unmarshal(data, &pr); err != nil {
		t.Fatalf("live 402 fixture must parse with the strict scheme guard: %v", err)
	}
	if len(pr.Accepts) == 0 {
		t.Fatal("fixture must advertise at least one accepts entry")
	}
	for _, a := range pr.Accepts {
		if a.Scheme != SchemeExact && a.Scheme != SchemeEscrow {
			t.Fatalf("unexpected scheme in live fixture: %q", a.Scheme)
		}
	}
	if pr.Resource.URL != "/v1/chat/completions" {
		t.Fatalf("unexpected resource url: %q", pr.Resource.URL)
	}
}

func TestLive402FixtureWithChannelSchemeRejected(t *testing.T) {
	data, err := os.ReadFile(x402FixturePath)
	if err != nil {
		t.Fatalf("read shared 402 fixture: %v", err)
	}
	var raw map[string]json.RawMessage
	if err := json.Unmarshal(data, &raw); err != nil {
		t.Fatalf("fixture must be a JSON object: %v", err)
	}
	var accepts []map[string]any
	if err := json.Unmarshal(raw["accepts"], &accepts); err != nil {
		t.Fatalf("fixture accepts must parse: %v", err)
	}
	// Clone the first entry as a hypothetical "channel" advert.
	channelEntry := map[string]any{}
	for k, v := range accepts[0] {
		channelEntry[k] = v
	}
	channelEntry["scheme"] = "channel"
	accepts = append(accepts, channelEntry)
	mutated, err := json.Marshal(accepts)
	if err != nil {
		t.Fatalf("marshal mutated accepts: %v", err)
	}
	raw["accepts"] = mutated
	body, err := json.Marshal(raw)
	if err != nil {
		t.Fatalf("marshal mutated fixture: %v", err)
	}
	var pr PaymentRequired
	if err := json.Unmarshal(body, &pr); err == nil {
		t.Fatal("a 402 carrying an unknown scheme must be rejected at parse time — " +
			"this strictness is why the gateway must never advertise 'channel' " +
			"in accepts[] (invariant 12 / HALT 12)")
	}
}
