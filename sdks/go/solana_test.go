package solvela

import (
	"bytes"
	"crypto/ed25519"
	"encoding/hex"
	"strings"
	"testing"

	"github.com/mr-tron/base58"
)

// IMPORTANT — Anchor discriminator pitfalls
// -----------------------------------------
// Anchor generates TWO discriminators per instruction:
//   - sha256("global:<name>")[:8]       → the INSTRUCTION entrypoint discriminator
//   - sha256("event:<EventName>")[:8]   → the event emission discriminator
// We need the INSTRUCTION discriminator here. If you see a value that looks
// like "e445a52e51cb9a1d" (a known-wrong value from an earlier draft of this
// plan, fixed in a later review) or "3ecdf2aff4a98834" (which is actually
// sha256("event:Deposit")[:8], NOT the instruction discriminator) — those
// are NOT the instruction discriminator for "deposit". Recompute via:
//   python3 -c 'import hashlib; print(hashlib.sha256(b"global:deposit").digest()[:8].hex())'
// Expected: f223c68952e1f2b6

// TestAnchorDiscriminatorDeposit locks the "deposit" discriminator to an
// externally-computed value. Do NOT derive this from anchorDiscriminator()
// itself — it was computed via:
//   python -c 'import hashlib; print(hashlib.sha256(b"global:deposit").digest()[:8].hex())'
// Expected: f223c68952e1f2b6
func TestAnchorDiscriminatorDeposit(t *testing.T) {
	const expectedHex = "f223c68952e1f2b6"
	got := anchorDiscriminator("deposit")
	gotHex := hex.EncodeToString(got[:])
	if gotHex != expectedHex {
		t.Fatalf("anchorDiscriminator(\"deposit\") = %s, want %s", gotHex, expectedHex)
	}
}

// TestAnchorDiscriminatorClaim locks the "claim" discriminator, also
// externally verified.
func TestAnchorDiscriminatorClaim(t *testing.T) {
	// Computed via: python -c 'import hashlib; print(hashlib.sha256(b"global:claim").digest()[:8].hex())'
	const expectedHex = "3ec6d6c1d59f6cd2"
	got := anchorDiscriminator("claim")
	gotHex := hex.EncodeToString(got[:])
	if gotHex != expectedHex {
		t.Fatalf("anchorDiscriminator(\"claim\") = %s, want %s", gotHex, expectedHex)
	}
}

// TestAnchorDiscriminatorRefund locks the "refund" discriminator.
func TestAnchorDiscriminatorRefund(t *testing.T) {
	// Computed via: python -c 'import hashlib; print(hashlib.sha256(b"global:refund").digest()[:8].hex())'
	const expectedHex = "0260b7fb3fd02e2e"
	got := anchorDiscriminator("refund")
	gotHex := hex.EncodeToString(got[:])
	if gotHex != expectedHex {
		t.Fatalf("anchorDiscriminator(\"refund\") = %s, want %s", gotHex, expectedHex)
	}
}

// TestEscrowPDARegression is pinned to the `solders`-computed canonical PDA
// from the Invariants table. Agent=[1;32], ServiceID=[2;32],
// Program=9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU.
// Expected PDA: BEAUsvsWvV4o6y7XkC1bkyTq4FtQnKErcV3dzTFPT5hX, bump: 255.
func TestEscrowPDARegression(t *testing.T) {
	programID, err := base58DecodePubkey("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU")
	if err != nil {
		t.Fatalf("decode program id: %v", err)
	}
	var agent [32]byte
	var serviceID [32]byte
	for i := range agent {
		agent[i] = 1
	}
	for i := range serviceID {
		serviceID[i] = 2
	}

	pda, bump, err := findProgramAddress(
		[][]byte{[]byte("escrow"), agent[:], serviceID[:]},
		programID,
	)
	if err != nil {
		t.Fatalf("findProgramAddress: %v", err)
	}

	expected, err := base58DecodePubkey("BEAUsvsWvV4o6y7XkC1bkyTq4FtQnKErcV3dzTFPT5hX")
	if err != nil {
		t.Fatalf("decode expected PDA: %v", err)
	}
	if pda != expected {
		t.Errorf("PDA = %s, want BEAUsvsWvV4o6y7XkC1bkyTq4FtQnKErcV3dzTFPT5hX",
			base58EncodePubkey(pda))
	}
	if bump != 255 {
		t.Errorf("bump = %d, want 255", bump)
	}
}

// TestATARegression is pinned to a Helius-verified mainnet ATA.
// Wallet: 4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp
// Mint:   EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v (USDC)
// ATA:    CYHVCkLwiEjMBdRiz5MsrrCbVL2YTZuv57TjV3ggxoSN
func TestATARegression(t *testing.T) {
	wallet, err := base58DecodePubkey("4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp")
	if err != nil {
		t.Fatalf("decode wallet: %v", err)
	}
	mint, err := base58DecodePubkey("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v")
	if err != nil {
		t.Fatalf("decode mint: %v", err)
	}

	ata, err := deriveATA(wallet, mint)
	if err != nil {
		t.Fatalf("deriveATA: %v", err)
	}

	expected, err := base58DecodePubkey("CYHVCkLwiEjMBdRiz5MsrrCbVL2YTZuv57TjV3ggxoSN")
	if err != nil {
		t.Fatalf("decode expected ATA: %v", err)
	}
	if ata != expected {
		t.Errorf("ATA = %s, want CYHVCkLwiEjMBdRiz5MsrrCbVL2YTZuv57TjV3ggxoSN",
			base58EncodePubkey(ata))
	}
}

// TestATAProgramIDNotTypo guards against the historical typo bug where
// ATA_PROGRAM_ID ended in "e1bxs" instead of "A8knL".
func TestATAProgramIDNotTypo(t *testing.T) {
	decoded, err := base58DecodePubkey(AssociatedTokenProgramIDB58)
	if err != nil {
		t.Fatalf("decode ATA program id: %v", err)
	}
	reencoded := base58EncodePubkey(decoded)
	suffix := reencoded[len(reencoded)-5:]
	if suffix != "A8knL" {
		t.Errorf("ATA program id suffix = %s, want A8knL (typo bug regression)", suffix)
	}
}

// TestSystemProgramIDAllZeros verifies the System program id decodes to
// 32 zero bytes (Solana canonical).
func TestSystemProgramIDAllZeros(t *testing.T) {
	decoded, err := base58DecodePubkey(SystemProgramIDB58)
	if err != nil {
		t.Fatalf("decode system program id: %v", err)
	}
	var zero [32]byte
	if decoded != zero {
		t.Errorf("System program id = %x, want all zeros", decoded)
	}
}

// TestIsOnEd25519Curve_WalletOnCurve verifies that a known wallet address
// is on the curve.
func TestIsOnEd25519Curve_WalletOnCurve(t *testing.T) {
	wallet, err := base58DecodePubkey("4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp")
	if err != nil {
		t.Fatalf("decode wallet: %v", err)
	}
	if !isOnEd25519Curve(wallet) {
		t.Error("expected a valid wallet address to be on the ed25519 curve")
	}
}

// TestIsOnEd25519Curve_PDAOffCurve verifies that a derived PDA is off the curve.
func TestIsOnEd25519Curve_PDAOffCurve(t *testing.T) {
	programID, _ := base58DecodePubkey("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU")
	var agent, serviceID [32]byte
	for i := range agent {
		agent[i] = 1
		serviceID[i] = 2
	}
	pda, _, err := findProgramAddress(
		[][]byte{[]byte("escrow"), agent[:], serviceID[:]},
		programID,
	)
	if err != nil {
		t.Fatalf("findProgramAddress: %v", err)
	}
	if isOnEd25519Curve(pda) {
		t.Error("expected PDA to be off the ed25519 curve")
	}
}

// TestCompactU16Encoding verifies the short_vec encoding used in Solana messages.
func TestCompactU16Encoding(t *testing.T) {
	cases := []struct {
		in  uint16
		out string
	}{
		{0, "00"},
		{1, "01"},
		{127, "7f"},
		{128, "8001"},
		{129, "8101"},
		{16383, "ff7f"},
		{16384, "808001"},
	}
	for _, c := range cases {
		got := hex.EncodeToString(encodeCompactU16(c.in))
		if got != c.out {
			t.Errorf("encodeCompactU16(%d) = %s, want %s", c.in, got, c.out)
		}
	}
}

// TestBase58RoundTrip verifies encode/decode symmetry.
func TestBase58RoundTrip(t *testing.T) {
	var key [32]byte
	for i := range key {
		key[i] = byte(i)
	}
	encoded := base58EncodePubkey(key)
	decoded, err := base58DecodePubkey(encoded)
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	if decoded != key {
		t.Errorf("round-trip mismatch: got %x, want %x", decoded, key)
	}
}

// TestBase58InvalidLength rejects non-32-byte inputs.
func TestBase58InvalidLength(t *testing.T) {
	_, err := base58DecodePubkey("1111")
	if err == nil {
		t.Error("expected error for short base58 input")
	}
}

// TestDecodeAndValidateKeypair_Valid accepts a genuine base58 keypair.
func TestDecodeAndValidateKeypair_Valid(t *testing.T) {
	var seed [32]byte
	for i := range seed {
		seed[i] = 7
	}
	priv := ed25519.NewKeyFromSeed(seed[:])
	b58 := base58.Encode(priv)

	got, pub, err := decodeAndValidateKeypair(b58)
	if err != nil {
		t.Fatalf("decodeAndValidateKeypair: %v", err)
	}
	if !bytes.Equal(got, priv) {
		t.Error("returned private key differs from input")
	}
	expectedPub := priv.Public().(ed25519.PublicKey)
	if !bytes.Equal(pub[:], expectedPub) {
		t.Error("returned agent pubkey differs from seed-derived pubkey")
	}
}

// TestDecodeAndValidateKeypair_SwappedPubkey is the critical integrity test.
func TestDecodeAndValidateKeypair_SwappedPubkey(t *testing.T) {
	var seedA, seedB [32]byte
	for i := range seedA {
		seedA[i] = 1
	}
	for i := range seedB {
		seedB[i] = 2
	}
	privA := ed25519.NewKeyFromSeed(seedA[:])
	privB := ed25519.NewKeyFromSeed(seedB[:])
	pubB := privB.Public().(ed25519.PublicKey)

	bad := make([]byte, 64)
	copy(bad[:32], privA[:32])
	copy(bad[32:], pubB)
	badB58 := base58.Encode(bad)

	_, _, err := decodeAndValidateKeypair(badB58)
	if err == nil {
		t.Fatal("expected error for mismatched stored pubkey, got nil")
	}
	if _, ok := err.(*SigningError); !ok {
		t.Errorf("expected *SigningError, got %T", err)
	}
}

// TestDecodeAndValidateKeypair_ShortLength rejects non-64-byte inputs.
func TestDecodeAndValidateKeypair_ShortLength(t *testing.T) {
	short := base58.Encode(make([]byte, 32))
	_, _, err := decodeAndValidateKeypair(short)
	if err == nil {
		t.Fatal("expected error for 32-byte input")
	}
}

// TestDecodeAndValidateKeypair_InvalidBase58 rejects disallowed characters.
func TestDecodeAndValidateKeypair_InvalidBase58(t *testing.T) {
	_, _, err := decodeAndValidateKeypair("0OIl")
	if err == nil {
		t.Fatal("expected error for invalid base58 input containing disallowed chars")
	}
	if _, ok := err.(*SigningError); !ok {
		t.Errorf("expected *SigningError, got %T", err)
	}
}

// TestDecodeAndValidateKeypair_EmptyString rejects empty input.
func TestDecodeAndValidateKeypair_EmptyString(t *testing.T) {
	_, _, err := decodeAndValidateKeypair("")
	if err == nil {
		t.Fatal("expected error for empty string input")
	}
	se, ok := err.(*SigningError)
	if !ok {
		t.Fatalf("expected *SigningError, got %T", err)
	}
	if !strings.Contains(se.Message, "64 bytes") {
		t.Errorf("expected error to mention \"64 bytes\", got %q", se.Message)
	}
}
