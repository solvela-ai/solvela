package rcr

import (
	"bytes"
	"crypto/ed25519"
	"encoding/base64"
	"os"
	"strings"
	"testing"

	"github.com/mr-tron/base58"
)

// expectedEscrowPDA is the externally-computed escrow PDA for the deterministic
// inputs used by TestBuildEscrowDeposit_BuilderPDAMatchesExternal. See Task 7
// Step 0 for the Python+solders snippet used to compute it. Leaving this as
// the "<FILL ME IN FROM PYTHON OUTPUT>" placeholder is a HARD FAILURE — the
// init() guard below will panic on `go test` startup so the bug cannot slip
// past when SOLANA_RPC_URL is unset and the test silently skips.
//
// EXTERNAL GROUND TRUTH — computed 2026-04-09 by task-7-test-author
// via the solders+pynacl snippet in Task 7 Step 0 with seed=[42]*32,
// service_id=[0xAB]*32, program=9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU.
// DO NOT update this value without recomputing externally first.
const expectedEscrowPDA = "9zicYQTKaNyFvnTbpBXoeA1DkzUmrf8yHMHdhzFVTG8X"

// init is a compile-time-ish placeholder guard. It fires at `go test`
// startup BEFORE any test runs, so if the author left the placeholder
// value in place this entire test binary will panic with a loud message.
// This catches a failure mode the reviewer flagged: a `const expectedPDA
// = "<FILL ME IN...>"` inside a test body compiles fine but never triggers
// if the test itself skips (e.g. because SOLANA_RPC_URL is unset), so a
// placeholder could silently ship. Running the guard from init() removes
// the "test skipped" escape hatch.
func init() {
	if strings.Contains(expectedEscrowPDA, "FILL ME IN") {
		panic("expectedEscrowPDA in sdks/go/escrow_test.go was not filled in " +
			"from external computation; see Task 7 Step 0 in the plan for the " +
			"exact Python+solders snippet to run")
	}
}

// escrowTestKeypairB58 returns the same deterministic base58 64-byte keypair
// used by Task 5 (seed [42; 32]) so the expected agent pubkey is stable
// across all escrow tests.
func escrowTestKeypairB58(t *testing.T) string {
	t.Helper()
	var seed [32]byte
	for i := range seed {
		seed[i] = 42
	}
	priv := ed25519.NewKeyFromSeed(seed[:])
	return base58.Encode(priv)
}

// escrowTestAgentPubkey returns the derived pubkey bytes for the seed-42
// keypair.
func escrowTestAgentPubkey(t *testing.T) [32]byte {
	t.Helper()
	var seed [32]byte
	for i := range seed {
		seed[i] = 42
	}
	priv := ed25519.NewKeyFromSeed(seed[:])
	pub := priv.Public().(ed25519.PublicKey)
	var out [32]byte
	copy(out[:], pub)
	return out
}

func TestBuildEscrowDeposit_ZeroAmountRejected(t *testing.T) {
	t.Setenv("SOLANA_RPC_URL", "http://127.0.0.1:0")
	_, err := BuildEscrowDeposit(EscrowDepositParams{
		AgentKeypairB58:    escrowTestKeypairB58(t),
		ProviderWalletB58:  "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		EscrowProgramIDB58: "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
		Amount:             0,
		ServiceID:          [32]byte{1},
		ExpirySlot:         99999999,
	})
	if err == nil {
		t.Fatal("expected error for zero amount")
	}
	if _, ok := err.(*SigningError); !ok {
		t.Errorf("expected *SigningError, got %T", err)
	}
}

func TestBuildEscrowDeposit_MissingRPCURL(t *testing.T) {
	t.Setenv("SOLANA_RPC_URL", "")
	_, err := BuildEscrowDeposit(EscrowDepositParams{
		AgentKeypairB58:    escrowTestKeypairB58(t),
		ProviderWalletB58:  "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		EscrowProgramIDB58: "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
		Amount:             1000,
		ServiceID:          [32]byte{1},
		ExpirySlot:         99999999,
	})
	if err == nil {
		t.Fatal("expected error for missing SOLANA_RPC_URL")
	}
}

func TestBuildEscrowDeposit_InvalidProviderAddress(t *testing.T) {
	t.Setenv("SOLANA_RPC_URL", "http://127.0.0.1:0")
	_, err := BuildEscrowDeposit(EscrowDepositParams{
		AgentKeypairB58:    escrowTestKeypairB58(t),
		ProviderWalletB58:  "not!valid!",
		EscrowProgramIDB58: "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
		Amount:             1000,
		ServiceID:          [32]byte{1},
		ExpirySlot:         99999999,
	})
	if err == nil {
		t.Fatal("expected error for invalid provider address")
	}
}

func TestBuildEscrowDeposit_InvalidEscrowProgramID(t *testing.T) {
	t.Setenv("SOLANA_RPC_URL", "http://127.0.0.1:0")
	_, err := BuildEscrowDeposit(EscrowDepositParams{
		AgentKeypairB58:    escrowTestKeypairB58(t),
		ProviderWalletB58:  "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		EscrowProgramIDB58: "not!valid!",
		Amount:             1000,
		ServiceID:          [32]byte{1},
		ExpirySlot:         99999999,
	})
	if err == nil {
		t.Fatal("expected error for invalid escrow program id")
	}
}

// TestBuildEscrowDeposit_BuilderPDAMatchesExternal verifies that the escrow
// PDA slot produced by `BuildEscrowDeposit` (slot [1] of the sorted account
// keys in the wire-format message) matches the package-level
// `expectedEscrowPDA` constant, which was computed EXTERNALLY via the
// Python+solders snippet in Task 7 Step 0 of the plan.
//
// This test intentionally decodes the wire bytes instead of calling
// findProgramAddress — we are testing the builder's output, not the helper.
// (findProgramAddress is exercised separately by Task 3.)
//
// IMPORTANT: `expectedEscrowPDA` is referenced unconditionally and also
// guarded by the init() check at the top of this file. If the test author
// forgot to fill in the Python-computed PDA, the init guard panics at
// `go test` startup — so there is no way to ship a placeholder value even
// if SOLANA_RPC_URL is unset and this test body skips.
//
// This test requires `SOLANA_RPC_URL` to be set to a working endpoint because
// `BuildEscrowDeposit` calls `getRecentBlockhash`. When running locally
// without devnet access, skip with `t.Skip` if the RPC is unreachable.
func TestBuildEscrowDeposit_BuilderPDAMatchesExternal(t *testing.T) {
	rpcURL := os.Getenv("SOLANA_RPC_URL")
	if rpcURL == "" {
		t.Skip("SOLANA_RPC_URL not set — needed for blockhash fetch")
	}
	t.Setenv("SOLANA_RPC_URL", rpcURL)

	// Deterministic inputs — same seed pattern the rest of the suite uses.
	var serviceID [32]byte
	for i := range serviceID {
		serviceID[i] = 0xAB
	}

	result, err := BuildEscrowDeposit(EscrowDepositParams{
		AgentKeypairB58:    escrowTestKeypairB58(t),
		ProviderWalletB58:  "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		EscrowProgramIDB58: "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
		Amount:             1000,
		ServiceID:          serviceID,
		ExpirySlot:         99999999,
	})
	if err != nil {
		t.Fatalf("BuildEscrowDeposit: %v", err)
	}

	decoded, err := base64.StdEncoding.DecodeString(result.DepositTxB64)
	if err != nil {
		t.Fatalf("base64 decode: %v", err)
	}

	// Parse the wire format to extract account keys.
	// Layout:
	//   compact_u16(sig_count=1)  → 1 byte
	//   signature                 → 64 bytes
	//   message:
	//     header                  → 3 bytes
	//     compact_u16(key_count)  → 1 byte (our count is 10, fits in 1 byte)
	//     account keys            → key_count * 32 bytes
	//     ... (rest ignored)
	//
	// Slot [0] is agent (signer+writable). Slot [1] is the escrow PDA
	// (writable non-signer appended immediately after signers).
	keys := decodeAccountKeysFromWire(t, decoded)
	if len(keys) < 2 {
		t.Fatalf("decoded account keys = %d, want ≥ 2", len(keys))
	}
	pdaSlot := base58EncodePubkey(keys[1])
	if pdaSlot != expectedEscrowPDA {
		t.Errorf("escrow PDA at slot [1] = %s, want %s", pdaSlot, expectedEscrowPDA)
	}
}

// decodeAccountKeysFromWire is a minimal wire-format decoder used only by the
// builder verification test above. It deliberately does NOT share code with
// the builder under test — the whole point is to parse the bytes independently
// and assert that the builder put the right keys at the right slots.
func decodeAccountKeysFromWire(t *testing.T, tx []byte) [][32]byte {
	t.Helper()
	if len(tx) < 1+64+3+1+32 {
		t.Fatalf("tx too short: %d bytes", len(tx))
	}
	// Skip compact-u16 signature count. For a single-signer tx this is 1 byte (0x01).
	if tx[0] != 0x01 {
		t.Fatalf("signature count byte = %d, want 1", tx[0])
	}
	offset := 1
	// Skip the 64-byte signature.
	offset += 64
	// Skip the 3-byte message header.
	offset += 3
	// Read the compact-u16 account count. Our count is always ≤ 127,
	// so it fits in one byte.
	if tx[offset]&0x80 != 0 {
		t.Fatalf("account count is multi-byte compact-u16, not supported here: %#x", tx[offset])
	}
	keyCount := int(tx[offset])
	offset++
	if len(tx) < offset+keyCount*32 {
		t.Fatalf("truncated account keys section: need %d bytes from offset %d, have %d",
			keyCount*32, offset, len(tx)-offset)
	}
	out := make([][32]byte, keyCount)
	for i := 0; i < keyCount; i++ {
		copy(out[i][:], tx[offset+i*32:offset+(i+1)*32])
	}
	return out
}

// TestBuildEscrowDeposit_Live exercises the full builder against a live RPC.
// Gated on SOLANA_RPC_URL + RCR_GO_SDK_LIVE_TEST=1.
//
// This test verifies the returned base64 tx decodes cleanly, contains the
// deposit discriminator, the agent pubkey, and a valid structure.
func TestBuildEscrowDeposit_Live(t *testing.T) {
	if os.Getenv("RCR_GO_SDK_LIVE_TEST") != "1" {
		t.Skip("RCR_GO_SDK_LIVE_TEST not set")
	}
	rpcURL := os.Getenv("SOLANA_RPC_URL")
	if rpcURL == "" {
		t.Skip("SOLANA_RPC_URL not set")
	}

	agentKP := escrowTestKeypairB58(t)
	var serviceID [32]byte
	for i := range serviceID {
		serviceID[i] = 0xAB
	}

	// Compute expiry slot from live RPC.
	expiry, err := GetExpirySlotFromTimeout(rpcURL, 300)
	if err != nil {
		t.Fatalf("GetExpirySlotFromTimeout: %v", err)
	}

	result, err := BuildEscrowDeposit(EscrowDepositParams{
		AgentKeypairB58:    agentKP,
		ProviderWalletB58:  "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		EscrowProgramIDB58: "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
		Amount:             1000,
		ServiceID:          serviceID,
		ExpirySlot:         expiry,
	})
	if err != nil {
		t.Fatalf("BuildEscrowDeposit: %v", err)
	}

	decoded, err := base64.StdEncoding.DecodeString(result.DepositTxB64)
	if err != nil {
		t.Fatalf("base64 decode: %v", err)
	}

	// Check signature count byte
	if decoded[0] != 0x01 {
		t.Errorf("signature count = %d, want 1", decoded[0])
	}

	// Check the agent pubkey is present in the tx
	agentPub := escrowTestAgentPubkey(t)
	if !bytes.Contains(decoded, agentPub[:]) {
		t.Error("agent pubkey not present in transaction bytes")
	}

	// Check the deposit discriminator is present
	disc := anchorDiscriminator("deposit")
	if !bytes.Contains(decoded, disc[:]) {
		t.Error("deposit discriminator not present in transaction bytes")
	}

	// Sanity check the agent pubkey base58 matches what's returned
	expectedB58 := base58.Encode(agentPub[:])
	if result.AgentPubkey != expectedB58 {
		t.Errorf("AgentPubkey = %s, want %s", result.AgentPubkey, expectedB58)
	}
}

// TestBuildEscrowDeposit_SignatureVerifies is an end-to-end signing
// correctness test analogous to TestBuildSolanaTransferChecked_SignatureVerifies
// in exact_test.go. It calls BuildEscrowDeposit to sign a real escrow
// deposit transaction, then decodes the wire bytes, extracts the signature
// (bytes [1:65]) and message (bytes [65:]), and verifies the signature using
// ed25519.Verify against the deterministic agent pubkey. A passing result
// rules out whole classes of bugs: wrong message bytes signed, wrong
// signature length, wrong signing key, or any byte-order regression that
// doesn't also break the Python cross-verify.
//
// Gated on SOLANA_RPC_URL + RCR_GO_SDK_LIVE_TEST=1 because
// BuildEscrowDeposit fetches a blockhash from RPC.
func TestBuildEscrowDeposit_SignatureVerifies(t *testing.T) {
	if os.Getenv("RCR_GO_SDK_LIVE_TEST") != "1" {
		t.Skip("RCR_GO_SDK_LIVE_TEST not set")
	}
	if os.Getenv("SOLANA_RPC_URL") == "" {
		t.Skip("SOLANA_RPC_URL not set")
	}

	agentPub := escrowTestAgentPubkey(t)
	var serviceID [32]byte
	for i := range serviceID {
		serviceID[i] = 0xAB
	}
	expiry, err := GetExpirySlotFromTimeout(os.Getenv("SOLANA_RPC_URL"), 300)
	if err != nil {
		t.Fatalf("GetExpirySlotFromTimeout: %v", err)
	}

	result, err := BuildEscrowDeposit(EscrowDepositParams{
		AgentKeypairB58:    escrowTestKeypairB58(t),
		ProviderWalletB58:  "4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp",
		EscrowProgramIDB58: "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
		Amount:             1000,
		ServiceID:          serviceID,
		ExpirySlot:         expiry,
	})
	if err != nil {
		t.Fatalf("BuildEscrowDeposit: %v", err)
	}

	decoded, err := base64.StdEncoding.DecodeString(result.DepositTxB64)
	if err != nil {
		t.Fatalf("base64 decode: %v", err)
	}

	// Wire format: compact_u16(1) || signature(64) || message
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

	pub := ed25519.PublicKey(agentPub[:])
	if !ed25519.Verify(pub, message, signature) {
		t.Error("ed25519.Verify returned false — the escrow deposit signature " +
			"does not verify against the deterministic agent pubkey for the " +
			"decoded message bytes")
	}
}

// TestGetExpirySlotFromTimeout_MissingRPC verifies that GetExpirySlotFromTimeout
// returns an error when called with an empty RPC URL.
func TestGetExpirySlotFromTimeout_MissingRPC(t *testing.T) {
	_, err := GetExpirySlotFromTimeout("", 300)
	if err == nil {
		t.Fatal("expected error for empty RPC URL")
	}
}
