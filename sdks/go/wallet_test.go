package solvela

import (
	"crypto/ed25519"
	"fmt"
	"strings"
	"testing"
)

func TestCreateWallet(t *testing.T) {
	w, mnemonic, err := CreateWallet()
	if err != nil {
		t.Fatalf("CreateWallet: %v", err)
	}
	if w == nil {
		t.Fatal("wallet should not be nil")
	}
	if mnemonic == "" {
		t.Error("mnemonic placeholder should not be empty")
	}
	if len(w.Address()) == 0 {
		t.Error("address should not be empty")
	}
	if len(w.PublicKey()) != ed25519.PublicKeySize {
		t.Errorf("public key length: got %d, want %d", len(w.PublicKey()), ed25519.PublicKeySize)
	}
	// Private key is no longer exported; verify length via the persistence helper.
	if len(w.ToKeypairBytes()) != ed25519.PrivateKeySize {
		t.Errorf("private key length: got %d, want %d", len(w.ToKeypairBytes()), ed25519.PrivateKeySize)
	}
	// Sign should succeed on a freshly created wallet.
	sig, err := w.Sign([]byte("hello"))
	if err != nil {
		t.Fatalf("Sign: %v", err)
	}
	if len(sig) != ed25519.SignatureSize {
		t.Errorf("sig length: got %d, want %d", len(sig), ed25519.SignatureSize)
	}
}

func TestWalletRoundtripBytes(t *testing.T) {
	w1, _, err := CreateWallet()
	if err != nil {
		t.Fatalf("CreateWallet: %v", err)
	}

	raw := w1.ToKeypairBytes()
	w2, err := WalletFromKeypairBytes(raw)
	if err != nil {
		t.Fatalf("WalletFromKeypairBytes: %v", err)
	}

	if w1.Address() != w2.Address() {
		t.Errorf("address mismatch: %q != %q", w1.Address(), w2.Address())
	}
}

func TestWalletRoundtripB58(t *testing.T) {
	w1, _, err := CreateWallet()
	if err != nil {
		t.Fatalf("CreateWallet: %v", err)
	}

	b58 := w1.ToKeypairB58()
	w2, err := WalletFromKeypairB58(b58)
	if err != nil {
		t.Fatalf("WalletFromKeypairB58: %v", err)
	}

	if w1.Address() != w2.Address() {
		t.Errorf("address mismatch: %q != %q", w1.Address(), w2.Address())
	}
}

func TestWalletFromKeypairBytesInvalidLength(t *testing.T) {
	_, err := WalletFromKeypairBytes([]byte("too short"))
	if err == nil {
		t.Fatal("expected error for invalid length")
	}
	var walletErr *WalletError
	if !isWalletError(err, &walletErr) {
		t.Errorf("expected WalletError, got %T", err)
	}
}

func TestWalletFromKeypairB58Invalid(t *testing.T) {
	_, err := WalletFromKeypairB58("not-valid-base58!!!")
	if err == nil {
		t.Fatal("expected error for invalid base58")
	}
}

func TestWalletFromEnv(t *testing.T) {
	w1, _, err := CreateWallet()
	if err != nil {
		t.Fatalf("CreateWallet: %v", err)
	}

	t.Setenv("TEST_WALLET_KEY", w1.ToKeypairB58())

	w2, err := WalletFromEnv("TEST_WALLET_KEY")
	if err != nil {
		t.Fatalf("WalletFromEnv: %v", err)
	}

	if w1.Address() != w2.Address() {
		t.Errorf("address mismatch: %q != %q", w1.Address(), w2.Address())
	}
}

func TestWalletFromEnvMissing(t *testing.T) {
	_, err := WalletFromEnv("NONEXISTENT_VAR_12345")
	if err == nil {
		t.Fatal("expected error for missing env var")
	}
}

func TestWalletStringRedacts(t *testing.T) {
	w, _, err := CreateWallet()
	if err != nil {
		t.Fatalf("CreateWallet: %v", err)
	}

	s := w.String()
	if !strings.Contains(s, "REDACTED") {
		t.Errorf("String() should contain REDACTED: got %q", s)
	}
	if !strings.Contains(s, w.Address()) {
		t.Errorf("String() should contain address: got %q", s)
	}
	// Ensure the private key is not in the string
	b58 := w.ToKeypairB58()
	if strings.Contains(s, b58) {
		t.Error("String() should not contain the full private key")
	}
}

// TestWalletFormatRedactsAllVerbs guards against the %#v secret leak: fmt's
// default %#v reflects over struct fields and would dump the raw
// ed25519.PrivateKey bytes. Wallet implements fmt.Formatter (plus Stringer /
// GoStringer) so EVERY verb — %v, %+v, %#v, %s — on both a Wallet value and a
// *Wallet pointer routes through the redacted form. The key is constructed from
// an all-0x2a seed so the raw secret bytes render as "0x2a" / "42" sequences
// under the default formatters; their absence proves redaction.
func TestWalletFormatRedactsAllVerbs(t *testing.T) {
	// Deterministic key: every private-key byte is 0x2a (== 42 decimal).
	seed := make([]byte, ed25519.SeedSize)
	for i := range seed {
		seed[i] = 0x2a
	}
	priv := ed25519.NewKeyFromSeed(seed)
	w, err := WalletFromKeypairBytes(priv)
	if err != nil {
		t.Fatalf("WalletFromKeypairBytes: %v", err)
	}

	// The first 32 bytes of an ed25519.PrivateKey are the seed (all 0x2a here),
	// so a leaked raw key would surface these byte renderings.
	leakMarkers := []string{"0x2a", "0x2A", ", 42,", "{42,", " 42,"}

	verbs := []string{"%v", "%+v", "%#v", "%s"}
	subjects := map[string]any{
		"value":   *w,
		"pointer": w,
	}
	for name, subj := range subjects {
		for _, verb := range verbs {
			out := fmt.Sprintf(verb, subj)
			if !strings.Contains(out, "REDACTED") {
				t.Errorf("%s %s: expected REDACTED, got %q", name, verb, out)
			}
			if !strings.Contains(out, w.Address()) {
				t.Errorf("%s %s: expected address %q, got %q", name, verb, w.Address(), out)
			}
			for _, marker := range leakMarkers {
				if strings.Contains(out, marker) {
					t.Errorf("%s %s leaked raw key bytes (marker %q): %q", name, verb, marker, out)
				}
			}
			// Also ensure the serialized secret never appears verbatim.
			if strings.Contains(out, w.ToKeypairB58()) {
				t.Errorf("%s %s leaked base58 secret: %q", name, verb, out)
			}
		}
	}
}

func isWalletError(err error, target **WalletError) bool {
	we, ok := err.(*WalletError)
	if ok && target != nil {
		*target = we
	}
	return ok
}
