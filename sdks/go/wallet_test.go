package solvela

import (
	"crypto/ed25519"
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

func isWalletError(err error, target **WalletError) bool {
	we, ok := err.(*WalletError)
	if ok && target != nil {
		*target = we
	}
	return ok
}
