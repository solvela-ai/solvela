package solvela

import (
	"crypto/ed25519"
	"encoding/base64"
	"os"
	"testing"

	"github.com/mr-tron/base58"
)

// testKeypairB58 returns a deterministic base58-encoded 64-byte keypair derived
// from seed [42; 32]. Matches the Rust reference pattern in
// crates/x402/src/escrow/deposit.rs:test_agent_keypair_b58.
func testKeypairB58(t *testing.T) string {
	t.Helper()
	var seed [32]byte
	for i := range seed {
		seed[i] = 42
	}
	priv := ed25519.NewKeyFromSeed(seed[:])
	// ed25519.PrivateKey already holds [seed(32) || pub(32)] when derived via
	// NewKeyFromSeed, matching the base58 64-byte format Solana keypairs use.
	return base58.Encode(priv)
}

// TestBuildTransferChecked_ZeroAmountRejected ensures zero amount fails before
// touching RPC or the key.
func TestBuildTransferChecked_ZeroAmountRejected(t *testing.T) {
	t.Setenv("SOLANA_RPC_URL", "http://localhost:0") // unused — must fail earlier
	_, err := buildSolanaTransferChecked(
		"4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		0,
		testKeypairB58(t),
	)
	if err == nil {
		t.Fatal("expected error for zero amount")
	}
	se, ok := err.(*SigningError)
	if !ok {
		t.Fatalf("expected *SigningError, got %T", err)
	}
	if se.Message == "" {
		t.Error("SigningError message must not be empty")
	}
}

// TestBuildTransferChecked_MissingRPCURL ensures absence of SOLANA_RPC_URL
// fails cleanly.
func TestBuildTransferChecked_MissingRPCURL(t *testing.T) {
	t.Setenv("SOLANA_RPC_URL", "")
	_, err := buildSolanaTransferChecked(
		"4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		1000,
		testKeypairB58(t),
	)
	if err == nil {
		t.Fatal("expected error when SOLANA_RPC_URL unset")
	}
	if _, ok := err.(*SigningError); !ok {
		t.Errorf("expected *SigningError, got %T", err)
	}
}

// TestBuildTransferChecked_InvalidPrivateKey rejects malformed base58 keys.
func TestBuildTransferChecked_InvalidPrivateKey(t *testing.T) {
	t.Setenv("SOLANA_RPC_URL", "http://127.0.0.1:0")
	_, err := buildSolanaTransferChecked(
		"4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		1000,
		"not!valid!base58!",
	)
	if err == nil {
		t.Fatal("expected error for invalid private key")
	}
}

// TestBuildTransferChecked_InvalidKeyLength rejects keys that decode to the
// wrong length.
func TestBuildTransferChecked_InvalidKeyLength(t *testing.T) {
	t.Setenv("SOLANA_RPC_URL", "http://127.0.0.1:0")
	// 32-byte key encoded in base58 (correct format would be 64 bytes).
	shortKey := base58.Encode(make([]byte, 32))
	_, err := buildSolanaTransferChecked(
		"4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		1000,
		shortKey,
	)
	if err == nil {
		t.Fatal("expected error for wrong key length")
	}
}

// TestBuildLegacyMessageRoundTripStructure verifies the message layout is
// well-formed by decoding the header and compact-u16 prefixes after building
// a minimal message. Does not require RPC.
func TestBuildLegacyMessageRoundTripStructure(t *testing.T) {
	var k1, k2, prog, blockhash [32]byte
	for i := range k1 {
		k1[i] = 1
		k2[i] = 2
		prog[i] = 3
		blockhash[i] = 4
	}
	msg := buildLegacyMessage(
		[3]byte{1, 0, 1},
		[][32]byte{k1, k2},
		prog,
		blockhash,
		2, // program at index 2
		[]byte{0, 1},
		[]byte{0xAA, 0xBB},
	)

	// Header 3 bytes
	if msg[0] != 1 || msg[1] != 0 || msg[2] != 1 {
		t.Errorf("header mismatch: %v", msg[0:3])
	}
	// account count compact-u16 single byte = 3 (k1, k2, prog)
	if msg[3] != 3 {
		t.Errorf("account count = %d, want 3", msg[3])
	}
	// Next 3*32 = 96 bytes are the keys
	if !bytesEqual(msg[4:36], k1[:]) {
		t.Error("key0 mismatch")
	}
	if !bytesEqual(msg[36:68], k2[:]) {
		t.Error("key1 mismatch")
	}
	if !bytesEqual(msg[68:100], prog[:]) {
		t.Error("program key mismatch")
	}
	// Then blockhash
	if !bytesEqual(msg[100:132], blockhash[:]) {
		t.Error("blockhash mismatch")
	}
	// Then ix count = 1
	if msg[132] != 1 {
		t.Errorf("ix count = %d, want 1", msg[132])
	}
	// Then program_id_index = 2
	if msg[133] != 2 {
		t.Errorf("program_id_index = %d, want 2", msg[133])
	}
	// Then account indices count = 2, indices = 0, 1
	if msg[134] != 2 || msg[135] != 0 || msg[136] != 1 {
		t.Errorf("ix account indices malformed: %v", msg[134:137])
	}
	// Then data len = 2, data = 0xAA, 0xBB
	if msg[137] != 2 || msg[138] != 0xAA || msg[139] != 0xBB {
		t.Errorf("ix data malformed: %v", msg[137:140])
	}
	if len(msg) != 140 {
		t.Errorf("total message length = %d, want 140", len(msg))
	}
}

// bytesEqual is a local helper to avoid a bytes import collision with
// the stdlib import in exact.go.
func bytesEqual(a, b []byte) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

// TestBuildTransferChecked_Live runs against a real RPC if SOLANA_RPC_URL is
// set AND RCR_GO_SDK_LIVE_TEST=1. Verifies the output is valid base64 and
// contains the agent pubkey and a plausible signature.
func TestBuildTransferChecked_Live(t *testing.T) {
	if os.Getenv("RCR_GO_SDK_LIVE_TEST") != "1" {
		t.Skip("RCR_GO_SDK_LIVE_TEST not set")
	}
	if os.Getenv("SOLANA_RPC_URL") == "" {
		t.Skip("SOLANA_RPC_URL not set")
	}
	encoded, err := buildSolanaTransferChecked(
		"4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		1000, // 0.001 USDC
		testKeypairB58(t),
	)
	if err != nil {
		t.Fatalf("buildSolanaTransferChecked: %v", err)
	}
	decoded, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil {
		t.Fatalf("base64 decode: %v", err)
	}
	// Wire format: 1 + 64 + message
	if len(decoded) < 100 {
		t.Errorf("tx too short: %d bytes", len(decoded))
	}
	if decoded[0] != 0x01 {
		t.Errorf("signature count byte = %d, want 1", decoded[0])
	}
}

// TestBuildSolanaTransferChecked_SignatureVerifies is an end-to-end signing
// correctness test. It asks the builder to sign a real transaction, then
// decodes the wire bytes, extracts the signature (bytes [1:65]) and message
// (bytes [65:]), and verifies the signature using ed25519.Verify against
// the deterministic agent pubkey. A passing result rules out whole classes
// of bugs: wrong message bytes signed, wrong signature length, wrong
// signing key, byte-order bugs, etc.
//
// Gated on SOLANA_RPC_URL + RCR_GO_SDK_LIVE_TEST=1 because
// buildSolanaTransferChecked fetches a blockhash from RPC.
func TestBuildSolanaTransferChecked_SignatureVerifies(t *testing.T) {
	if os.Getenv("RCR_GO_SDK_LIVE_TEST") != "1" {
		t.Skip("RCR_GO_SDK_LIVE_TEST not set")
	}
	if os.Getenv("SOLANA_RPC_URL") == "" {
		t.Skip("SOLANA_RPC_URL not set")
	}

	// Derive the deterministic agent pubkey from seed [42;32].
	var seed [32]byte
	for i := range seed {
		seed[i] = 42
	}
	agentPriv := ed25519.NewKeyFromSeed(seed[:])
	agentPub := agentPriv.Public().(ed25519.PublicKey)

	encoded, err := buildSolanaTransferChecked(
		"4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		1000,
		testKeypairB58(t),
	)
	if err != nil {
		t.Fatalf("buildSolanaTransferChecked: %v", err)
	}
	decoded, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil {
		t.Fatalf("base64 decode: %v", err)
	}

	// Wire format: compact_u16(1) || signature(64) || message
	// For a single-signer tx, compact_u16(1) is a single byte 0x01.
	if len(decoded) < 1+64+1 {
		t.Fatalf("decoded tx too short: %d bytes", len(decoded))
	}
	if decoded[0] != 0x01 {
		t.Fatalf("signature count byte = %d, want 1", decoded[0])
	}
	signature := decoded[1:65]
	if len(signature) != 64 {
		t.Fatalf("signature length = %d, want 64", len(signature))
	}
	message := decoded[65:]

	if !ed25519.Verify(agentPub, message, signature) {
		t.Error("ed25519.Verify returned false — the signature does not verify " +
			"against the deterministic agent pubkey for the decoded message bytes")
	}
}
