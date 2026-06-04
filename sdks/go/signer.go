package solvela

import (
	"bytes"
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"

	"github.com/mr-tron/base58"
)

// Signer is a pluggable interface for signing payment transactions.
type Signer interface {
	SignPayment(ctx context.Context, amountAtomic uint64, recipient string, resource Resource, accepted PaymentAccept) (*PaymentPayload, error)
}

// SchemeCapable is an OPTIONAL interface a [Signer] may implement to declare
// which x402 payment schemes it can actually fulfill. Scheme selection
// ([SolvelaClient.findCompatibleScheme]) consults this so it never picks a
// scheme the active signer cannot sign.
//
// This matters because the gateway advertises BOTH `exact` and `escrow` (with
// `exact` listed first; see crates/gateway/src/routes/chat/mod.rs). A signer
// that only implements `escrow` (like [KeypairSigner]) would otherwise have the
// client auto-select `exact` and fail every payment — dead on arrival. Honoring
// CanSignScheme lets selection prefer `escrow` for such a signer.
//
// A signer that does NOT implement this interface is treated as scheme-agnostic
// (assumed able to sign any compatible scheme), preserving the legacy
// "first compatible scheme wins" behavior for custom signers.
type SchemeCapable interface {
	// CanSignScheme reports whether this signer can produce a signed payload for
	// the given scheme. It must not perform I/O.
	CanSignScheme(scheme Scheme) bool
}

// Compile-time assertion that KeypairSigner satisfies Signer and SchemeCapable.
// If either interface drifts (e.g., a method is renamed), the build fails here
// rather than at first runtime call.
var (
	_ Signer        = (*KeypairSigner)(nil)
	_ SchemeCapable = (*KeypairSigner)(nil)
)

// --- Escrow expiry-slot bounds (mirror the Rust/Python SDK signers) ---
//
// Solana slots are ~400 ms. These bounds are ported verbatim from
// sdks/rust/crates/solvela-client/src/signer.rs and sdks/python/.../signer.py so
// all SDKs choose compatible expiry slots and none is bounced by the gateway.
const (
	// maxEscrowExpirySlotsAhead is the upper bound on how far ahead an escrow
	// may expire (~66 min). Rejects a malicious/misconfigured gateway from
	// pushing expiry to "never".
	maxEscrowExpirySlotsAhead = 10_000

	// minEscrowExpirySlotsAhead is the lower bound on expiry distance. Mirrors
	// AND exceeds the gateway's MIN_EXPIRY_BUFFER_SLOTS = 50
	// (crates/x402/src/escrow/verifier.rs); the extra headroom (150 vs 50)
	// absorbs slot-skew between the slot we read and the slot the gateway
	// verifies against. ~150 slots ~= 60 s.
	minEscrowExpirySlotsAhead = 150

	// minPlausibleSlot is the floor below which a getSlot result is implausible
	// and rejected. A stub/genesis/freshly-reset validator can answer getSlot
	// with a near-zero slot; computing an escrow expiry from that base yields
	// an already-expired (or trivially-near-expiry) deposit the gateway would
	// bounce. Mainnet/devnet have been well past this for years. Fail closed
	// rather than silently signing a dead-on-arrival escrow deposit.
	minPlausibleSlot = 1_000_000

	defaultRPCURL = "https://api.mainnet-beta.solana.com"

	// maxRPCBodyBytes caps a successful (200 OK) JSON-RPC response body. The
	// getSlot/getLatestBlockhash responses this signer reads are tiny (a few
	// hundred bytes), but we still bound the read so a misbehaving/hostile RPC
	// endpoint cannot stream an unbounded body into memory. This is the
	// success-path cap; maxErrorBodyBytes (4 KB) is the tighter error-path cap.
	maxRPCBodyBytes = 64 << 10 // 64 KB
)

// KeypairSigner is the default Signer. It builds real Solana transactions,
// branching on the x402 payment scheme:
//
//	exact  -> NOT yet implemented in the Go SDK (returns a clear error; never
//	          silently substitutes another scheme). For exact-scheme payments use
//	          the Rust SDK (the canonical reference implementation) or supply a
//	          custom Signer.
//	escrow -> on-chain deposit into a per-request escrow PDA (returns an
//	          EscrowPayload), byte-exact with the canonical builder. This is the
//	          scheme the Go SDK signs; scheme selection prefers it (see
//	          CanSignScheme / SchemeCapable).
//
// There is NO silent fallback: an escrow-selected payment always produces an
// escrow deposit, never an exact transfer, and an unknown scheme is rejected.
// Routing an escrow-selected payment as an exact transfer is the scheme-mismatch
// money-path bug this guards against (per the solvela-x402 skill).
type KeypairSigner struct {
	wallet *Wallet
	rpcURL string
	client *http.Client
}

// NewKeypairSigner creates a [KeypairSigner] from a wallet and optional Solana
// RPC URL. If rpcURL is empty, defaults to mainnet-beta.
func NewKeypairSigner(wallet *Wallet, rpcURL string) *KeypairSigner {
	if rpcURL == "" {
		rpcURL = defaultRPCURL
	}
	return &KeypairSigner{
		wallet: wallet,
		rpcURL: rpcURL,
		client: &http.Client{Timeout: 30 * time.Second},
	}
}

// CanSignScheme reports which schemes this signer can fulfill. The Go
// KeypairSigner implements `escrow` only; `exact` is intentionally
// unimplemented (see signExactPayment), so it reports false for exact. This
// keeps scheme selection from auto-picking the gateway's first-listed `exact`
// offer and failing every payment.
func (s *KeypairSigner) CanSignScheme(scheme Scheme) bool {
	return scheme == SchemeEscrow
}

// SignPayment builds and signs a payment transaction, branching on the scheme.
func (s *KeypairSigner) SignPayment(
	ctx context.Context,
	amountAtomic uint64,
	recipient string,
	resource Resource,
	accepted PaymentAccept,
) (*PaymentPayload, error) {
	switch accepted.Scheme {
	case SchemeExact:
		return s.signExactPayment(ctx, amountAtomic, recipient, resource, accepted)
	case SchemeEscrow:
		return s.signEscrowPayment(ctx, amountAtomic, resource, accepted)
	default:
		// Scheme is a closed type; PaymentAccept.UnmarshalJSON already rejects
		// unknown wire schemes. This branch fails closed if a new scheme is
		// added to the enum without a financial branch here — never defaults to
		// a transfer.
		return nil, &SignerError{Message: fmt.Sprintf("unsupported payment scheme: %q", accepted.Scheme)}
	}
}

// signExactPayment handles the exact (USDC-SPL transfer) scheme.
//
// The Go SDK does not yet build exact-scheme transfers. Critically, it does NOT
// silently fall back to an escrow deposit or any other scheme — it returns a
// clear typed error so the caller knows exact signing is unavailable here. The
// Rust SDK is the reference implementation for the exact path; the byte layout
// has no pinned golden vector, so it is intentionally left unimplemented in Go
// rather than guessed at on the money path.
func (s *KeypairSigner) signExactPayment(
	_ context.Context,
	_ uint64,
	_ string,
	_ Resource,
	_ PaymentAccept,
) (*PaymentPayload, error) {
	return nil, &SignerError{Message: "exact-scheme signing is not implemented in the Go SDK; " +
		"use the Rust SDK, or supply a custom Signer (the Go SDK does build escrow-scheme deposits)"}
}

// signEscrowPayment builds and signs an escrow deposit transaction (escrow
// scheme).
//
// Generates a fresh CSPRNG service_id, computes the expiry slot from the current
// slot and accepted.MaxTimeoutSeconds (mirroring the Rust/Python SDKs), fetches a
// recent blockhash, and delegates byte-exact transaction construction to the
// shared, golden-vector-pinned buildDepositTx.
func (s *KeypairSigner) signEscrowPayment(
	ctx context.Context,
	amountAtomic uint64,
	resource Resource,
	accepted PaymentAccept,
) (*PaymentPayload, error) {
	// Fail clearly if escrow was selected but no program ID was offered — never
	// silently fall back to an exact transfer.
	if accepted.EscrowProgramID == nil || *accepted.EscrowProgramID == "" {
		return nil, &SignerError{Message: "escrow scheme selected but escrow_program_id is missing"}
	}

	// amountAtomic is a uint64: a negative/float amount is structurally
	// impossible (unlike the dynamically-typed Python port). We still reject
	// zero before issuing any RPC — a zero-value escrow deposit is never
	// legitimate, and we must not burn an RPC round-trip on it.
	if amountAtomic == 0 {
		return nil, &SignerError{Message: "escrow deposit amount must be greater than zero"}
	}

	if s.wallet == nil {
		return nil, &SignerError{Message: "signer has no wallet configured"}
	}

	// Per-request CSPRNG service_id (#118 invariant): two identical requests
	// must never share a service_id -> escrow PDA -> vault ATA.
	serviceID, err := generateServiceID()
	if err != nil {
		return nil, &SignerError{Message: fmt.Sprintf("failed to generate service_id: %v", err)}
	}

	currentSlot, err := s.fetchCurrentSlot(ctx)
	if err != nil {
		return nil, err
	}
	expirySlot := escrowExpirySlot(currentSlot, accepted.MaxTimeoutSeconds)

	blockhash, err := s.fetchLatestBlockhash(ctx)
	if err != nil {
		return nil, err
	}

	depositTx, err := buildDepositTx(&DepositParams{
		agentKeypair:       s.wallet.privateKeyBytes(),
		ProviderWalletB58:  accepted.PayTo,
		USDCMintB58:        USDCMint,
		EscrowProgramIDB58: *accepted.EscrowProgramID,
		Amount:             amountAtomic,
		ServiceID:          serviceID,
		ExpirySlot:         expirySlot,
		RecentBlockhash:    blockhash,
	})
	if err != nil {
		return nil, err
	}

	payload := EscrowPayload{
		DepositTx:   depositTx,
		ServiceID:   base64.StdEncoding.EncodeToString(serviceID[:]),
		AgentPubkey: s.wallet.Address(),
	}
	return &PaymentPayload{
		X402Version: X402Version,
		Resource:    resource,
		Accepted:    accepted,
		Payload:     payload,
	}, nil
}

// escrowExpirySlot computes the slot at which a now-created escrow PDA should
// expire. Ported from the Rust SDK's escrow_expiry_slot: ~2.5 slots/s
// (1000 ms / 400 ms), clamped into
// [minEscrowExpirySlotsAhead, maxEscrowExpirySlotsAhead]. The floor mirrors and
// exceeds the gateway's MIN_EXPIRY_BUFFER_SLOTS so a too-near expiry is never
// bounced; the cap rejects an unbounded-future expiry. A negative timeout is
// floored to 0 (never pushes expiry backwards into a dead-on-arrival deposit).
// Arithmetic saturates to avoid wrap-to-zero.
func escrowExpirySlot(currentSlot uint64, maxTimeoutSeconds int) uint64 {
	timeout := maxTimeoutSeconds
	if timeout < 0 {
		timeout = 0
	}
	// timeout * 1000 / 400, with saturation on the multiply.
	timeoutSlots := satMul(uint64(timeout), 1000) / 400
	effective := timeoutSlots
	if effective < minEscrowExpirySlotsAhead {
		effective = minEscrowExpirySlotsAhead
	}
	if effective > maxEscrowExpirySlotsAhead {
		effective = maxEscrowExpirySlotsAhead
	}
	return satAdd(currentSlot, effective)
}

func satMul(a, b uint64) uint64 {
	if a == 0 || b == 0 {
		return 0
	}
	if a > ^uint64(0)/b {
		return ^uint64(0)
	}
	return a * b
}

func satAdd(a, b uint64) uint64 {
	if a > ^uint64(0)-b {
		return ^uint64(0)
	}
	return a + b
}

// generateServiceID returns a unique 32-byte service_id. Mirrors the Rust SDK:
// hash 32 CSPRNG bytes so the result is distinct per call (#118 invariant — two
// identical requests must never share a service_id).
func generateServiceID() ([32]byte, error) {
	var nonce [32]byte
	if _, err := rand.Read(nonce[:]); err != nil {
		return [32]byte{}, err
	}
	return sha256.Sum256(nonce[:]), nil
}

// rpcRequest is the minimal JSON-RPC envelope this signer sends.
type rpcRequest struct {
	JSONRPC string `json:"jsonrpc"`
	ID      int    `json:"id"`
	Method  string `json:"method"`
	Params  []any  `json:"params"`
}

// rpcError is the JSON-RPC error object; we surface only the numeric code,
// never the message (which can echo node internals — GHSA-cgqx-mg48-949v).
// Code is a pointer so we can distinguish an absent/unparseable code (nil) from
// a real code of 0 — emitting "code: 0" for a code-less error is misleading.
type rpcError struct {
	Code *int `json:"code"`
}

// rpcErrorCodeSuffix renders a parenthesized, message-free description of a
// JSON-RPC error object suitable for appending to a SignerError. It surfaces
// only the numeric code (never the RPC message text — GHSA-cgqx-mg48-949v), and
// clearly distinguishes an unparseable / code-less error from a real code.
func rpcErrorCodeSuffix(rawErr json.RawMessage) string {
	var e rpcError
	if err := json.Unmarshal(rawErr, &e); err != nil || e.Code == nil {
		return "(unparseable error object, no numeric code)"
	}
	return fmt.Sprintf("(code: %d)", *e.Code)
}

// postRPC issues a JSON-RPC POST and returns the decoded body. HTTP-level
// failures, non-200 status, and malformed JSON are surfaced as *SignerError
// before any field access — so a 429/5xx HTML body never leaks node internals
// into a panic or error string.
func (s *KeypairSigner) postRPC(ctx context.Context, method string, params []any, label string) (map[string]json.RawMessage, error) {
	body, err := json.Marshal(rpcRequest{JSONRPC: "2.0", ID: 1, Method: method, Params: params})
	if err != nil {
		return nil, &SignerError{Message: fmt.Sprintf("%s RPC: encode request: %v", label, err)}
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, s.rpcURL, bytes.NewReader(body))
	if err != nil {
		return nil, &SignerError{Message: fmt.Sprintf("%s RPC: build request: %v", label, err)}
	}
	req.Header.Set("Content-Type", "application/json")
	resp, err := s.client.Do(req)
	if err != nil {
		return nil, &SignerError{Message: fmt.Sprintf("%s RPC request failed", label)}
	}
	defer func() { _ = resp.Body.Close() }()
	// Check status BEFORE reading. A non-200 body is an error page (possibly an
	// HTML 429/5xx) we only bound and discard — so the tight error-path cap
	// applies. The status itself is the surfaced signal; the body text is never
	// echoed (it can leak node internals — GHSA-cgqx-mg48-949v).
	if resp.StatusCode != http.StatusOK {
		_, _ = io.Copy(io.Discard, io.LimitReader(resp.Body, maxErrorBodyBytes))
		return nil, &SignerError{Message: fmt.Sprintf("%s RPC HTTP %d", label, resp.StatusCode)}
	}
	// Success: read the JSON body under the generous success-path cap so we do
	// not truncate a legitimate 200 result (the old code applied the 4 KB
	// error-path cap here, which could corrupt a large result body).
	raw, err := io.ReadAll(io.LimitReader(resp.Body, maxRPCBodyBytes))
	if err != nil {
		return nil, &SignerError{Message: fmt.Sprintf("%s RPC: read body", label)}
	}
	var decoded map[string]json.RawMessage
	if err := json.Unmarshal(raw, &decoded); err != nil {
		return nil, &SignerError{Message: fmt.Sprintf("%s RPC: malformed JSON body", label)}
	}
	return decoded, nil
}

// fetchCurrentSlot fetches the current slot via getSlot (commitment confirmed).
// Fails closed: any HTTP failure, malformed body, RPC error, or
// missing/invalid result is a *SignerError — never a silent default slot, which
// would let the escrow expiry be computed from a bogus base. Also rejects an
// implausibly-low slot (stub/genesis node).
func (s *KeypairSigner) fetchCurrentSlot(ctx context.Context) (uint64, error) {
	decoded, err := s.postRPC(ctx, "getSlot", []any{map[string]string{"commitment": "confirmed"}}, "getSlot")
	if err != nil {
		return 0, err
	}
	if rawErr, ok := decoded["error"]; ok {
		return 0, &SignerError{Message: fmt.Sprintf("getSlot RPC error %s", rpcErrorCodeSuffix(rawErr))}
	}
	rawResult, ok := decoded["result"]
	if !ok {
		return 0, &SignerError{Message: "getSlot RPC: missing result"}
	}
	var slot uint64
	if err := json.Unmarshal(rawResult, &slot); err != nil {
		return 0, &SignerError{Message: "getSlot RPC: missing or invalid result"}
	}
	if slot < minPlausibleSlot {
		return 0, &SignerError{Message: fmt.Sprintf(
			"getSlot RPC returned implausibly low slot %d (< %d); refusing to build an escrow deposit",
			slot, minPlausibleSlot)}
	}
	return slot, nil
}

// fetchLatestBlockhash fetches a recent blockhash via getLatestBlockhash and
// decodes it to 32 raw bytes. Fails closed on any HTTP/JSON/RPC failure or a
// missing/malformed blockhash, never returning a default.
func (s *KeypairSigner) fetchLatestBlockhash(ctx context.Context) ([32]byte, error) {
	var out [32]byte
	decoded, err := s.postRPC(ctx, "getLatestBlockhash", []any{map[string]string{"commitment": "finalized"}}, "Blockhash")
	if err != nil {
		return out, err
	}
	rawResult, ok := decoded["result"]
	if !ok {
		// Surface the RPC error code if present, else a generic miss.
		if rawErr, hasErr := decoded["error"]; hasErr {
			return out, &SignerError{Message: fmt.Sprintf("RPC did not return a blockhash %s", rpcErrorCodeSuffix(rawErr))}
		}
		return out, &SignerError{Message: "RPC did not return a blockhash"}
	}
	var result struct {
		Value struct {
			Blockhash string `json:"blockhash"`
		} `json:"value"`
	}
	if err := json.Unmarshal(rawResult, &result); err != nil || result.Value.Blockhash == "" {
		return out, &SignerError{Message: "RPC did not return a blockhash"}
	}
	raw, err := base58.Decode(result.Value.Blockhash)
	if err != nil || len(raw) != 32 {
		return out, &SignerError{Message: "RPC returned a malformed blockhash"}
	}
	copy(out[:], raw)
	return out, nil
}
