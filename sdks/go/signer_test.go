package solvela

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

const testEscrowProgram = "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU"

// A valid base58 32-byte hash usable as a canned blockhash (the system program
// ID — 32 zero bytes encode to a valid 32-byte base58 string).
const fakeBlockhash = "11111111111111111111111111111111"

func newTestSigner(t *testing.T, rpcURL string) *KeypairSigner {
	t.Helper()
	wallet, _, err := CreateWallet()
	if err != nil {
		t.Fatalf("create wallet: %v", err)
	}
	return NewKeypairSigner(wallet, rpcURL)
}

func exactAccept() PaymentAccept {
	return PaymentAccept{
		Scheme:            SchemeExact,
		Network:           SolanaNetwork,
		Amount:            "1000000",
		Asset:             USDCMint,
		PayTo:             "11111111111111111111111111111112",
		MaxTimeoutSeconds: 300,
	}
}

func escrowAccept(programID *string) PaymentAccept {
	return PaymentAccept{
		Scheme:            SchemeEscrow,
		Network:           SolanaNetwork,
		Amount:            "2625",
		Asset:             USDCMint,
		PayTo:             goldenProvider,
		MaxTimeoutSeconds: 300,
		EscrowProgramID:   programID,
	}
}

func testResource() Resource {
	return Resource{URL: "https://rpc.test.local/v1/chat/completions", Method: "POST"}
}

func ptr(s string) *string { return &s }

// rpcMock is a test JSON-RPC server that returns queued responses per method.
type rpcMock struct {
	server   *httptest.Server
	requests int
}

// newRPCMock builds a server. getSlot and getLatestBlockhash routing is by the
// "method" field in the request body. Responses are supplied as raw JSON.
func newRPCMock(t *testing.T, getSlotJSON, getBlockhashJSON string, status int) *rpcMock {
	t.Helper()
	m := &rpcMock{}
	m.server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		m.requests++
		if status != 0 && status != http.StatusOK {
			w.WriteHeader(status)
			_, _ = w.Write([]byte("<html>error</html>"))
			return
		}
		var req struct {
			Method string `json:"method"`
		}
		body, _ := readAllBody(r)
		_ = json.Unmarshal(body, &req)
		switch req.Method {
		case "getSlot":
			_, _ = w.Write([]byte(getSlotJSON))
		case "getLatestBlockhash":
			_, _ = w.Write([]byte(getBlockhashJSON))
		default:
			w.WriteHeader(http.StatusBadRequest)
		}
	}))
	t.Cleanup(m.server.Close)
	return m
}

func readAllBody(r *http.Request) ([]byte, error) {
	defer func() { _ = r.Body.Close() }()
	// io.ReadAll drains the whole body; a single r.Body.Read can return a
	// partial body, which would make method-routing (and thus RPC-error tests)
	// assert against a truncated request.
	return io.ReadAll(r.Body)
}

func goodSlotJSON(slot int) string {
	return `{"jsonrpc":"2.0","id":1,"result":` + itoa(slot) + `}`
}

func goodBlockhashJSON() string {
	return `{"jsonrpc":"2.0","id":1,"result":{"context":{"slot":1000000},"value":{"blockhash":"` + fakeBlockhash + `","lastValidBlockHeight":1000000}}}`
}

func itoa(i int) string {
	if i == 0 {
		return "0"
	}
	neg := i < 0
	if neg {
		i = -i
	}
	var b []byte
	for i > 0 {
		b = append([]byte{byte('0' + i%10)}, b...)
		i /= 10
	}
	if neg {
		b = append([]byte{'-'}, b...)
	}
	return string(b)
}

// --- Constructor ---

func TestNewKeypairSignerDefaultRPCURL(t *testing.T) {
	signer := newTestSigner(t, "")
	if signer.rpcURL != defaultRPCURL {
		t.Errorf("rpcURL: got %q, want default mainnet", signer.rpcURL)
	}
}

func TestNewKeypairSignerCustomRPCURL(t *testing.T) {
	signer := newTestSigner(t, "https://api.devnet.solana.com")
	if signer.rpcURL != "https://api.devnet.solana.com" {
		t.Errorf("rpcURL: got %q, want devnet", signer.rpcURL)
	}
}

// --- Scheme branching (no silent fallback) ---

func TestEscrowSchemeReturnsEscrowPayload(t *testing.T) {
	m := newRPCMock(t, goodSlotJSON(1_000_000), goodBlockhashJSON(), http.StatusOK)
	signer := newTestSigner(t, m.server.URL)

	payload, err := signer.SignPayment(context.Background(), 2625, goldenProvider, testResource(), escrowAccept(ptr(testEscrowProgram)))
	if err != nil {
		t.Fatalf("escrow sign should succeed: %v", err)
	}
	if payload.Accepted.Scheme != SchemeEscrow {
		t.Errorf("scheme: got %q, want escrow", payload.Accepted.Scheme)
	}
	ep, ok := payload.Payload.(EscrowPayload)
	if !ok {
		t.Fatalf("payload type: got %T, want EscrowPayload", payload.Payload)
	}
	txBytes, err := base64.StdEncoding.DecodeString(ep.DepositTx)
	if err != nil {
		t.Fatalf("deposit_tx not valid base64: %v", err)
	}
	if txBytes[0] != 0x01 {
		t.Errorf("deposit_tx first byte: got 0x%02x, want 0x01 (1 sig)", txBytes[0])
	}
	sid, err := base64.StdEncoding.DecodeString(ep.ServiceID)
	if err != nil || len(sid) != 32 {
		t.Errorf("service_id must be base64 of 32 bytes, got len=%d err=%v", len(sid), err)
	}
	if ep.AgentPubkey != signer.wallet.Address() {
		t.Errorf("agent_pubkey: got %q, want wallet address %q", ep.AgentPubkey, signer.wallet.Address())
	}
}

func TestEscrowServiceIDUniquePerCall(t *testing.T) {
	m := newRPCMock(t, goodSlotJSON(1_000_000), goodBlockhashJSON(), http.StatusOK)
	signer := newTestSigner(t, m.server.URL)

	p1, err := signer.SignPayment(context.Background(), 2625, goldenProvider, testResource(), escrowAccept(ptr(testEscrowProgram)))
	if err != nil {
		t.Fatalf("p1: %v", err)
	}
	p2, err := signer.SignPayment(context.Background(), 2625, goldenProvider, testResource(), escrowAccept(ptr(testEscrowProgram)))
	if err != nil {
		t.Fatalf("p2: %v", err)
	}
	s1 := p1.Payload.(EscrowPayload).ServiceID
	s2 := p2.Payload.(EscrowPayload).ServiceID
	if s1 == s2 {
		t.Error("service_id must differ across calls (front-running / confidentiality invariant)")
	}
}

func TestExactSchemeNotImplementedNoSilentFallback(t *testing.T) {
	// The Go SDK does not build exact transfers. It must return a clear error,
	// NOT silently produce an escrow deposit or any other scheme. No RPC may be
	// issued.
	m := newRPCMock(t, goodSlotJSON(1_000_000), goodBlockhashJSON(), http.StatusOK)
	signer := newTestSigner(t, m.server.URL)

	_, err := signer.SignPayment(context.Background(), 1_000_000, "11111111111111111111111111111112", testResource(), exactAccept())
	if err == nil {
		t.Fatal("exact scheme should return an error in the Go SDK")
	}
	if _, ok := err.(*SignerError); !ok {
		t.Fatalf("expected *SignerError, got %T", err)
	}
	if !strings.Contains(err.Error(), "exact-scheme signing is not implemented") {
		t.Errorf("unexpected error message: %q", err.Error())
	}
	if m.requests != 0 {
		t.Errorf("no RPC should be issued for unimplemented exact path, got %d", m.requests)
	}
}

func TestUnknownSchemeRejected(t *testing.T) {
	m := newRPCMock(t, goodSlotJSON(1_000_000), goodBlockhashJSON(), http.StatusOK)
	signer := newTestSigner(t, m.server.URL)
	accept := exactAccept()
	accept.Scheme = Scheme("upto") // force an out-of-domain scheme past the parser

	_, err := signer.SignPayment(context.Background(), 1_000_000, "11111111111111111111111111111112", testResource(), accept)
	if err == nil {
		t.Fatal("unknown scheme must be rejected, never default-routed")
	}
	if !strings.Contains(err.Error(), "unsupported payment scheme") {
		t.Errorf("unexpected error: %q", err.Error())
	}
	if m.requests != 0 {
		t.Errorf("no RPC should be issued for an unknown scheme, got %d", m.requests)
	}
}

// --- Scheme selection prefers a signable scheme (DOA regression) ---

// TestFindCompatibleSchemePrefersEscrowForKeypairSigner pins the fix for the
// dead-on-arrival bug: the gateway advertises ["exact","escrow"] (exact first),
// but KeypairSigner can only sign escrow. Scheme selection must therefore pick
// the escrow entry, never the unsignable first-listed exact.
func TestFindCompatibleSchemePrefersEscrowForKeypairSigner(t *testing.T) {
	signer := newTestSigner(t, "")
	wallet, _, err := CreateWallet()
	if err != nil {
		t.Fatalf("wallet: %v", err)
	}
	client, err := NewClient(wallet, signer, WithGatewayURL("https://gw.test.local"))
	if err != nil {
		t.Fatalf("client: %v", err)
	}

	pr := &PaymentRequired{
		X402Version: X402Version,
		Accepts: []PaymentAccept{
			exactAccept(), // gateway lists exact FIRST
			escrowAccept(ptr(testEscrowProgram)),
		},
	}
	chosen := client.findCompatibleScheme(pr)
	if chosen == nil {
		t.Fatal("expected an escrow scheme to be selected, got nil")
	}
	if chosen.Scheme != SchemeEscrow {
		t.Errorf("scheme: got %q, want escrow (must not auto-select unsignable exact)", chosen.Scheme)
	}
}

// TestSignPaymentRequiredYieldsEscrowPayloadWhenBothOffered is the end-to-end
// assertion: feeding a 402 that offers both schemes to a KeypairSigner produces
// an escrow PaymentPayload, NOT a SignerError. Before the fix this errored
// because exact was selected and KeypairSigner cannot sign exact.
func TestSignPaymentRequiredYieldsEscrowPayloadWhenBothOffered(t *testing.T) {
	m := newRPCMock(t, goodSlotJSON(1_000_000), goodBlockhashJSON(), http.StatusOK)
	signer := newTestSigner(t, m.server.URL)
	wallet := signer.wallet
	maxAmt := uint64(1_000_000_000)
	client, err := NewClient(wallet, signer,
		WithGatewayURL("https://rpc.test.local"),
		WithMaxPaymentAmount(maxAmt),
	)
	if err != nil {
		t.Fatalf("client: %v", err)
	}

	pr := &PaymentRequired{
		X402Version: X402Version,
		Resource:    testResource(),
		Accepts: []PaymentAccept{
			exactAccept(),
			escrowAccept(ptr(testEscrowProgram)),
		},
	}
	sigHeader, err := client.signPaymentRequired(context.Background(), pr)
	if err != nil {
		t.Fatalf("signPaymentRequired should succeed with an escrow-capable signer, got: %v", err)
	}
	decoded, err := base64.StdEncoding.DecodeString(sigHeader)
	if err != nil {
		t.Fatalf("header not valid base64: %v", err)
	}
	var payload struct {
		Accepted PaymentAccept `json:"accepted"`
		Payload  struct {
			DepositTx string `json:"deposit_tx"`
		} `json:"payload"`
	}
	if err := json.Unmarshal(decoded, &payload); err != nil {
		t.Fatalf("decode payment payload: %v", err)
	}
	if payload.Accepted.Scheme != SchemeEscrow {
		t.Errorf("selected scheme: got %q, want escrow", payload.Accepted.Scheme)
	}
	if payload.Payload.DepositTx == "" {
		t.Error("expected a non-empty escrow deposit_tx in the signed payload")
	}
}

// TestCanSignSchemeKeypairSigner pins the capability declaration directly.
func TestCanSignSchemeKeypairSigner(t *testing.T) {
	signer := newTestSigner(t, "")
	if !signer.CanSignScheme(SchemeEscrow) {
		t.Error("KeypairSigner must report it can sign escrow")
	}
	if signer.CanSignScheme(SchemeExact) {
		t.Error("KeypairSigner must report it cannot sign exact (unimplemented)")
	}
}

// --- Escrow rejection / fail-closed paths ---

func TestEscrowMissingProgramIDRejected(t *testing.T) {
	m := newRPCMock(t, goodSlotJSON(1_000_000), goodBlockhashJSON(), http.StatusOK)
	signer := newTestSigner(t, m.server.URL)

	_, err := signer.SignPayment(context.Background(), 2625, goldenProvider, testResource(), escrowAccept(nil))
	if err == nil || !strings.Contains(err.Error(), "escrow_program_id is missing") {
		t.Fatalf("expected escrow_program_id missing error, got %v", err)
	}
	if m.requests != 0 {
		t.Errorf("rejection must occur before any RPC, got %d requests", m.requests)
	}
}

func TestEscrowEmptyProgramIDRejected(t *testing.T) {
	m := newRPCMock(t, goodSlotJSON(1_000_000), goodBlockhashJSON(), http.StatusOK)
	signer := newTestSigner(t, m.server.URL)

	_, err := signer.SignPayment(context.Background(), 2625, goldenProvider, testResource(), escrowAccept(ptr("")))
	if err == nil || !strings.Contains(err.Error(), "escrow_program_id is missing") {
		t.Fatalf("expected escrow_program_id missing error, got %v", err)
	}
	if m.requests != 0 {
		t.Errorf("rejection must occur before any RPC, got %d requests", m.requests)
	}
}

func TestEscrowZeroAmountRejected(t *testing.T) {
	m := newRPCMock(t, goodSlotJSON(1_000_000), goodBlockhashJSON(), http.StatusOK)
	signer := newTestSigner(t, m.server.URL)

	_, err := signer.SignPayment(context.Background(), 0, goldenProvider, testResource(), escrowAccept(ptr(testEscrowProgram)))
	if err == nil || !strings.Contains(err.Error(), "greater than zero") {
		t.Fatalf("expected greater-than-zero error, got %v", err)
	}
	if m.requests != 0 {
		t.Errorf("no RPC for a zero amount we reject before building, got %d", m.requests)
	}
}

func TestEscrowStubLowSlotRejected(t *testing.T) {
	// A stub/genesis node returning a near-zero slot must be refused, not used
	// to compute an instantly-expired escrow deposit.
	m := newRPCMock(t, goodSlotJSON(0), goodBlockhashJSON(), http.StatusOK)
	signer := newTestSigner(t, m.server.URL)

	_, err := signer.SignPayment(context.Background(), 2625, goldenProvider, testResource(), escrowAccept(ptr(testEscrowProgram)))
	if err == nil || !strings.Contains(err.Error(), "implausibly low slot") {
		t.Fatalf("expected implausibly-low-slot error, got %v", err)
	}
}

func TestEscrowGetSlot429Rejected(t *testing.T) {
	m := newRPCMock(t, "", "", http.StatusTooManyRequests)
	signer := newTestSigner(t, m.server.URL)

	_, err := signer.SignPayment(context.Background(), 2625, goldenProvider, testResource(), escrowAccept(ptr(testEscrowProgram)))
	if err == nil || !strings.Contains(err.Error(), "getSlot RPC HTTP 429") {
		t.Fatalf("expected getSlot HTTP 429 error, got %v", err)
	}
}

func TestEscrowGetSlotMalformedJSONRejected(t *testing.T) {
	m := newRPCMock(t, "not valid json", goodBlockhashJSON(), http.StatusOK)
	signer := newTestSigner(t, m.server.URL)

	_, err := signer.SignPayment(context.Background(), 2625, goldenProvider, testResource(), escrowAccept(ptr(testEscrowProgram)))
	if err == nil || !strings.Contains(err.Error(), "getSlot RPC: malformed JSON") {
		t.Fatalf("expected getSlot malformed JSON error, got %v", err)
	}
}

func TestEscrowGetSlotJSONRPCErrorRejected(t *testing.T) {
	m := newRPCMock(t, `{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"node behind"}}`, goodBlockhashJSON(), http.StatusOK)
	signer := newTestSigner(t, m.server.URL)

	_, err := signer.SignPayment(context.Background(), 2625, goldenProvider, testResource(), escrowAccept(ptr(testEscrowProgram)))
	if err == nil || !strings.Contains(err.Error(), "getSlot RPC error") {
		t.Fatalf("expected getSlot RPC error, got %v", err)
	}
	// The numeric code may appear, but the message text must not leak.
	if strings.Contains(err.Error(), "node behind") {
		t.Errorf("RPC error message text leaked: %q", err.Error())
	}
}

// TestEscrowGetSlotCodelessErrorNotMisleadingZero pins the LOW finding fix: a
// JSON-RPC error object with no numeric code must NOT be reported as
// "code: 0" (which looks like a real code). It should be flagged as
// unparseable / code-less instead.
func TestEscrowGetSlotCodelessErrorNotMisleadingZero(t *testing.T) {
	// An error object with a message but no "code" field.
	m := newRPCMock(t, `{"jsonrpc":"2.0","id":1,"error":{"message":"node behind"}}`, goodBlockhashJSON(), http.StatusOK)
	signer := newTestSigner(t, m.server.URL)

	_, err := signer.SignPayment(context.Background(), 2625, goldenProvider, testResource(), escrowAccept(ptr(testEscrowProgram)))
	if err == nil || !strings.Contains(err.Error(), "getSlot RPC error") {
		t.Fatalf("expected getSlot RPC error, got %v", err)
	}
	if strings.Contains(err.Error(), "code: 0") {
		t.Errorf("code-less error must not be rendered as 'code: 0': %q", err.Error())
	}
	if !strings.Contains(err.Error(), "no numeric code") {
		t.Errorf("expected a code-less marker in the error, got: %q", err.Error())
	}
	if strings.Contains(err.Error(), "node behind") {
		t.Errorf("RPC error message text leaked: %q", err.Error())
	}
}

func TestEscrowBlockhashMissingRejected(t *testing.T) {
	m := newRPCMock(t, goodSlotJSON(1_000_000), `{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"internal"}}`, http.StatusOK)
	signer := newTestSigner(t, m.server.URL)

	_, err := signer.SignPayment(context.Background(), 2625, goldenProvider, testResource(), escrowAccept(ptr(testEscrowProgram)))
	if err == nil || !strings.Contains(err.Error(), "did not return a blockhash") {
		t.Fatalf("expected blockhash-missing error, got %v", err)
	}
}

// TestEscrowBlockhashWrongLengthRejected pins the production guard at
// fetchLatestBlockhash: a base58-VALID but non-32-byte blockhash must be
// rejected as malformed, never copied into the 32-byte array (a short/long hash
// would produce an unverifiable transaction). "1111" decodes to 4 zero bytes
// under base58 — valid base58, wrong length.
func TestEscrowBlockhashWrongLengthRejected(t *testing.T) {
	shortHashJSON := `{"jsonrpc":"2.0","id":1,"result":{"context":{"slot":1000000},"value":{"blockhash":"1111","lastValidBlockHeight":1000000}}}`
	m := newRPCMock(t, goodSlotJSON(1_000_000), shortHashJSON, http.StatusOK)
	signer := newTestSigner(t, m.server.URL)

	_, err := signer.SignPayment(context.Background(), 2625, goldenProvider, testResource(), escrowAccept(ptr(testEscrowProgram)))
	if err == nil || !strings.Contains(err.Error(), "malformed blockhash") {
		t.Fatalf("expected malformed-blockhash error for a wrong-length hash, got %v", err)
	}
}

// --- escrowExpirySlot math (mirrors Rust/Python SDK tests) ---

func TestEscrowExpirySlotTypical300s(t *testing.T) {
	if got := escrowExpirySlot(1_000_000, 300); got != 1_000_750 {
		t.Errorf("got %d, want 1_000_750", got)
	}
}

func TestEscrowExpirySlotZeroSecondsAppliesMinFloor(t *testing.T) {
	if got := escrowExpirySlot(1_000_000, 0); got != 1_000_150 {
		t.Errorf("got %d, want 1_000_150", got)
	}
}

func TestEscrowExpirySlotHugeTimeoutClampedToCap(t *testing.T) {
	if got := escrowExpirySlot(1_000_000, 1<<40); got != 1_010_000 {
		t.Errorf("got %d, want 1_010_000", got)
	}
}

func TestEscrowExpirySlotNegativeTimeoutClampsToFloorNotBackwards(t *testing.T) {
	if got := escrowExpirySlot(1_000_000, -1); got != 1_000_150 {
		t.Errorf("got %d, want 1_000_150 (negative timeout must not push expiry backwards)", got)
	}
}

func TestEscrowExpirySlotSaturatesCurrentSlotOverflow(t *testing.T) {
	if got := escrowExpirySlot(^uint64(0)-100, 300); got != ^uint64(0) {
		t.Errorf("got %d, want u64::MAX (saturating add)", got)
	}
}
