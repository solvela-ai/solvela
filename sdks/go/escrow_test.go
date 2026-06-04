package solvela

// Unit tests for the escrow deposit-transaction builder.
//
// The golden-vector parity test is the contract: the Go deposit builder MUST
// reproduce crates/escrow-tx/src/deposit.rs's GOLDEN_VECTOR_B64 byte-for-byte
// for its fixed input. If it diverges, the on-chain layout disagreement is a
// fund-misdirection bug — fix the Go builder, NEVER edit the expected golden
// value (the Rust/on-chain value is ground truth).

import (
	"crypto/ed25519"
	"encoding/base64"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"

	"github.com/mr-tron/base58"
)

const (
	goldenProvider      = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM"
	goldenUSDCMint      = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
	goldenEscrowProgram = "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU"
)

// goldenKeypair builds the deterministic 64-byte agent keypair from seed
// [42]*32 (matches the Rust golden fixture: SigningKey::from_bytes([42; 32])).
func goldenKeypair() ed25519.PrivateKey {
	seed := make([]byte, 32)
	for i := range seed {
		seed[i] = 42
	}
	return ed25519.NewKeyFromSeed(seed)
}

func goldenParams() *DepositParams {
	var serviceID [32]byte
	for i := range serviceID {
		serviceID[i] = 7
	}
	var blockhash [32]byte
	for i := range blockhash {
		blockhash[i] = 0xAB
	}
	return &DepositParams{
		agentKeypair:       goldenKeypair(),
		ProviderWalletB58:  goldenProvider,
		USDCMintB58:        goldenUSDCMint,
		EscrowProgramIDB58: goldenEscrowProgram,
		Amount:             2625,
		ServiceID:          serviceID,
		ExpirySlot:         1_000_750,
		RecentBlockhash:    blockhash,
	}
}

// TestDepositTxMatchesGoldenVector is the sole correctness bar for the wire
// format: byte-for-byte parity with the canonical Rust builder.
func TestDepositTxMatchesGoldenVector(t *testing.T) {
	out, err := buildDepositTx(goldenParams())
	if err != nil {
		t.Fatalf("golden build should succeed: %v", err)
	}
	if out != GoldenVectorB64 {
		t.Fatalf("Go deposit-tx does not match the Rust golden vector.\n"+
			"The on-chain layout is ground truth — fix the Go builder, do NOT edit the expected value.\n"+
			"got:  %s\nwant: %s", out, GoldenVectorB64)
	}
}

// TestDepositTxDeterministic confirms the build is pure: same fixed input
// always yields the same bytes (no nonce, no clock).
func TestDepositTxDeterministic(t *testing.T) {
	a, err := buildDepositTx(goldenParams())
	if err != nil {
		t.Fatalf("build a: %v", err)
	}
	b, err := buildDepositTx(goldenParams())
	if err != nil {
		t.Fatalf("build b: %v", err)
	}
	if a != b {
		t.Fatal("deposit build is not deterministic")
	}
}

func TestGoldenVectorDecodesToSignedTx(t *testing.T) {
	raw, err := base64.StdEncoding.DecodeString(GoldenVectorB64)
	if err != nil {
		t.Fatalf("golden vector must be valid base64: %v", err)
	}
	// compact-u16(1) || sig(64) || message
	if raw[0] != 0x01 {
		t.Errorf("first byte should be 0x01 (1 signature), got 0x%02x", raw[0])
	}
	if len(raw) <= 1+ed25519.SignatureSize {
		t.Errorf("tx too short: %d bytes", len(raw))
	}
}

// TestGoldenVectorMatchesRustSource is the drift guard: the Go golden constant
// MUST equal the Rust source of truth. If the Rust GOLDEN_VECTOR_B64 ever
// changes, the on-chain wire layout changed and this test fails loudly, forcing
// a resync of the Go constant and builder rather than letting the SDKs silently
// diverge.
func TestGoldenVectorMatchesRustSource(t *testing.T) {
	// escrow_test.go -> sdks/go; worktree root is two levels up.
	root, err := filepath.Abs(filepath.Join("..", ".."))
	if err != nil {
		t.Fatalf("resolve worktree root: %v", err)
	}
	rustSrc := filepath.Join(root, "crates", "escrow-tx", "src", "deposit.rs")
	data, err := os.ReadFile(rustSrc)
	if err != nil {
		t.Fatalf("Rust deposit.rs not found at %s: %v", rustSrc, err)
	}
	re := regexp.MustCompile(`GOLDEN_VECTOR_B64:\s*&str\s*=\s*"([^"]+)"`)
	m := re.FindSubmatch(data)
	if m == nil {
		t.Fatalf("could not locate GOLDEN_VECTOR_B64 in %s", rustSrc)
	}
	rustValue := string(m[1])
	if rustValue != GoldenVectorB64 {
		t.Fatalf("Go GoldenVectorB64 has drifted from the Rust source of truth " +
			"(crates/escrow-tx/src/deposit.rs). The on-chain layout is ground truth: " +
			"resync the Go constant AND re-verify buildDepositTx byte-parity — do NOT " +
			"just paste the new value.")
	}
}

// ---------------------------------------------------------------------------
// Derivation parity (PDA seeds + vault ATA + discriminator).
// ---------------------------------------------------------------------------

func TestEscrowPDAExternalGroundTruth(t *testing.T) {
	// Mirrors crates/escrow-tx/src/pda.rs ground-truth test:
	// agent=[1]*32, service_id=[2]*32 -> BEAUsvsWvV4o6y7XkC1bkyTq4FtQnKErcV3dzTFPT5hX, bump 255.
	program, err := decodeBs58Pubkey(goldenEscrowProgram)
	if err != nil {
		t.Fatalf("decode program: %v", err)
	}
	var agent, serviceID [32]byte
	for i := range agent {
		agent[i] = 1
		serviceID[i] = 2
	}
	pda, bump, ok := findProgramAddress([][]byte{[]byte("escrow"), agent[:], serviceID[:]}, program)
	if !ok {
		t.Fatal("PDA derivation failed")
	}
	if got := base58.Encode(pda[:]); got != "BEAUsvsWvV4o6y7XkC1bkyTq4FtQnKErcV3dzTFPT5hX" {
		t.Errorf("PDA mismatch: got %s", got)
	}
	if bump != 255 {
		t.Errorf("bump mismatch: got %d, want 255", bump)
	}
}

func TestVaultATAKnownValue(t *testing.T) {
	// Mirrors crates/escrow-tx/src/pda.rs test_derive_ata_for_known_wallet.
	wallet, err := decodeBs58Pubkey("4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp")
	if err != nil {
		t.Fatalf("decode wallet: %v", err)
	}
	mint, err := decodeBs58Pubkey(goldenUSDCMint)
	if err != nil {
		t.Fatalf("decode mint: %v", err)
	}
	ata, ok := deriveATAAddress(wallet, mint)
	if !ok {
		t.Fatal("ATA derivation failed")
	}
	if got := base58.Encode(ata[:]); got != "CYHVCkLwiEjMBdRiz5MsrrCbVL2YTZuv57TjV3ggxoSN" {
		t.Errorf("ATA mismatch: got %s", got)
	}
}

func TestAnchorDiscriminatorDeterministic(t *testing.T) {
	d1 := anchorDiscriminator("deposit")
	d2 := anchorDiscriminator("deposit")
	if d1 != d2 {
		t.Error("discriminator not deterministic")
	}
	if anchorDiscriminator("deposit") == anchorDiscriminator("claim") {
		t.Error("distinct instruction names must give distinct discriminators")
	}
}

func TestIsOnEd25519CurveDetectsOffCurvePDA(t *testing.T) {
	// A derived PDA must be OFF the curve.
	program, _ := decodeBs58Pubkey(goldenEscrowProgram)
	var agent, serviceID [32]byte
	for i := range agent {
		agent[i] = 1
		serviceID[i] = 2
	}
	pda, _, _ := findProgramAddress([][]byte{[]byte("escrow"), agent[:], serviceID[:]}, program)
	if isOnEd25519Curve(pda) {
		t.Error("PDA must be off the ed25519 curve")
	}
	// A normal wallet pubkey (the golden agent) is ON the curve.
	var agentPub [32]byte
	copy(agentPub[:], goldenKeypair().Public().(ed25519.PublicKey))
	if !isOnEd25519Curve(agentPub) {
		t.Error("a real ed25519 pubkey must be on the curve")
	}
}

// ---------------------------------------------------------------------------
// Rejection / fail-closed behavior.
// ---------------------------------------------------------------------------

func TestDepositZeroAmountRejected(t *testing.T) {
	p := goldenParams()
	p.Amount = 0
	_, err := buildDepositTx(p)
	if err == nil {
		t.Fatal("zero amount should be rejected")
	}
	if _, ok := err.(*SignerError); !ok {
		t.Fatalf("expected *SignerError, got %T", err)
	}
}

func TestDepositInvalidKeypairLengthRejected(t *testing.T) {
	p := goldenParams()
	p.agentKeypair = ed25519.PrivateKey([]byte{1, 2, 3})
	_, err := buildDepositTx(p)
	if err == nil {
		t.Fatal("invalid keypair length should be rejected")
	}
}

func TestDepositInvalidProviderRejected(t *testing.T) {
	p := goldenParams()
	p.ProviderWalletB58 = "not-base58-0OIl!!!"
	_, err := buildDepositTx(p)
	if err == nil {
		t.Fatal("invalid provider address should be rejected")
	}
}

func TestDepositInvalidMintRejected(t *testing.T) {
	p := goldenParams()
	p.USDCMintB58 = "not-base58-0OIl!!!"
	_, err := buildDepositTx(p)
	if err == nil {
		t.Fatal("invalid mint address should be rejected")
	}
}

func TestDepositKeypairNotLeakedInError(t *testing.T) {
	// A malformed keypair must never echo secret bytes. We pass a wrong-length
	// key; the error message must not contain any hex/base58 of the bytes.
	p := goldenParams()
	p.agentKeypair = ed25519.PrivateKey([]byte{9, 9, 9, 9})
	_, err := buildDepositTx(p)
	if err == nil {
		t.Fatal("expected error")
	}
	if got := err.Error(); got != "signer error: invalid agent keypair: wrong length" {
		t.Errorf("error message leaked detail or changed: %q", got)
	}
}

func TestBuildLegacyMessageRejectsTooManyAccounts(t *testing.T) {
	// 127 data accounts + 1 program key = 128 total -> exceeds the limit.
	accounts := make([][32]byte, 127)
	_, err := buildLegacyMessage([3]byte{1, 0, 0}, accounts, [32]byte{}, [32]byte{}, 127, []byte{0}, []byte{0})
	if err == nil {
		t.Fatal("128 total accounts should be rejected")
	}
}

func TestBuildLegacyMessageRejectsLongIxData(t *testing.T) {
	_, err := buildLegacyMessage([3]byte{1, 0, 0}, [][32]byte{{}}, [32]byte{}, [32]byte{}, 1, []byte{0}, make([]byte, 128))
	if err == nil {
		t.Fatal("128 ix-data bytes should be rejected")
	}
}

// TestDepositCorruptedPubkeyHalfRejected mirrors the Rust guard
// (crates/escrow-tx/src/deposit.rs): ed25519.PrivateKey.Public() copies the
// stored bytes [32:64] rather than re-deriving from the seed, so a keypair with
// a corrupted public half would sign fine but seed the escrow PDA from the
// wrong identity, producing an un-claimable on-chain deposit. The builder must
// re-derive from the seed and reject the mismatch.
func TestDepositCorruptedPubkeyHalfRejected(t *testing.T) {
	p := goldenParams()
	corrupted := make(ed25519.PrivateKey, len(p.agentKeypair))
	copy(corrupted, p.agentKeypair)
	// Flip a bit in the stored public half (bytes 32..64) WITHOUT touching the
	// seed. The seed-derived pubkey will no longer match the stored one.
	corrupted[40] ^= 0xFF
	p.agentKeypair = corrupted

	_, err := buildDepositTx(p)
	if err == nil {
		t.Fatal("a keypair whose stored pubkey half does not match its seed must be rejected")
	}
	if _, ok := err.(*SignerError); !ok {
		t.Fatalf("expected *SignerError, got %T", err)
	}
	if !strings.Contains(err.Error(), "derived pubkey does not match stored pubkey") {
		t.Errorf("unexpected error: %q", err.Error())
	}
}

// TestDepositParamsRedactsKeypair verifies the secret keypair never appears in
// any fmt rendering of DepositParams (%v, %+v, %#v, %s). A raw 64-byte secret
// key landing in a log line is a key-disclosure bug.
func TestDepositParamsRedactsKeypair(t *testing.T) {
	// goldenParams() returns a *DepositParams; dereference so we can test both
	// the value and pointer forms below.
	pp := goldenParams()
	p := *pp
	// The first secret-seed byte of the golden keypair is 42 (0x2a). If the
	// struct were reflected normally, %v / %#v would print the keypair slice as
	// "[42 42 42 ...]" / "ed25519.PrivateKey{0x2a, ...}".
	for _, verb := range []string{"%v", "%+v", "%#v", "%s"} {
		// Test both the value and the *DepositParams pointer (production code
		// holds a *DepositParams and would log it directly).
		for _, arg := range []any{p, pp} {
			out := fmt.Sprintf(verb, arg)
			if !strings.Contains(out, "[REDACTED]") {
				t.Errorf("%s rendering of %T missing [REDACTED] marker: %q", verb, arg, out)
			}
			// Guard against the secret bytes leaking in either base-10 (reflected
			// slice) or 0x2a hex (%#v) form. "42 42 42" and "0x2a, 0x2a" are the
			// tell-tale shapes of a dumped 64-byte key.
			if strings.Contains(out, "42 42 42") || strings.Contains(out, "0x2a, 0x2a") {
				t.Errorf("%s rendering of %T leaked raw keypair bytes: %q", verb, arg, out)
			}
		}
	}
}
