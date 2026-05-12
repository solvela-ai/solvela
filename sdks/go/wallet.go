package solvela

import (
	"crypto/ed25519"
	"crypto/rand"
	"fmt"
	"os"

	"github.com/mr-tron/base58"
)

// Wallet holds an ed25519 keypair for Solana operations.
type Wallet struct {
	privateKey ed25519.PrivateKey
}

// CreateWallet generates a new random wallet.
// Returns the wallet and a placeholder mnemonic string.
func CreateWallet() (*Wallet, string, error) {
	_, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		return nil, "", &WalletError{Message: fmt.Sprintf("failed to generate keypair: %v", err)}
	}
	return &Wallet{privateKey: priv}, "(mnemonic not supported — store keypair bytes)", nil
}

// WalletFromKeypairBytes creates a wallet from raw 64-byte ed25519 keypair bytes.
func WalletFromKeypairBytes(raw []byte) (*Wallet, error) {
	if len(raw) != ed25519.PrivateKeySize {
		return nil, &WalletError{
			Message: fmt.Sprintf("invalid keypair length: expected %d, got %d", ed25519.PrivateKeySize, len(raw)),
		}
	}
	return &Wallet{privateKey: ed25519.PrivateKey(raw)}, nil
}

// WalletFromKeypairB58 creates a wallet from a base58-encoded keypair.
func WalletFromKeypairB58(b58 string) (*Wallet, error) {
	raw, err := base58.Decode(b58)
	if err != nil {
		return nil, &WalletError{Message: fmt.Sprintf("invalid base58: %v", err)}
	}
	return WalletFromKeypairBytes(raw)
}

// WalletFromEnv creates a wallet from a base58-encoded keypair stored in an environment variable.
func WalletFromEnv(varName string) (*Wallet, error) {
	val := os.Getenv(varName)
	if val == "" {
		return nil, &WalletError{Message: fmt.Sprintf("environment variable %s not set", varName)}
	}
	return WalletFromKeypairB58(val)
}

// Address returns the base58-encoded public key (Solana address).
func (w *Wallet) Address() string {
	return base58.Encode(w.PublicKey())
}

// PublicKey returns the ed25519 public key.
func (w *Wallet) PublicKey() ed25519.PublicKey {
	return w.privateKey.Public().(ed25519.PublicKey)
}

// privateKeyBytes returns the raw ed25519 private key. It is unexported so the
// secret material does not leak across the package boundary; callers that need
// to sign should use [Wallet.Sign]. Persistence helpers within this package
// (e.g. [Wallet.ToKeypairBytes]) use this internally.
//
// SECURITY: callers within the package MUST NOT log, print, or otherwise
// expose the returned bytes.
func (w *Wallet) privateKeyBytes() ed25519.PrivateKey {
	return w.privateKey
}

// Sign signs the given message with the wallet's ed25519 private key and
// returns the signature. Callers should prefer this over reaching for raw
// key bytes — it keeps the secret inside the package.
func (w *Wallet) Sign(msg []byte) ([]byte, error) {
	if w == nil || len(w.privateKey) != ed25519.PrivateKeySize {
		return nil, &WalletError{Message: "wallet is not initialized"}
	}
	return ed25519.Sign(w.privateKey, msg), nil
}

// ToKeypairBytes returns the raw 64-byte keypair.
//
// SECURITY: returns the secret key bytes. Use only for persistence (writing
// to a keystore or env var). Never log or print the result.
func (w *Wallet) ToKeypairBytes() []byte {
	return []byte(w.privateKey)
}

// ToKeypairB58 returns the base58-encoded keypair.
//
// SECURITY: returns the secret key in serialized form. Use only for
// persistence; never log or print the result.
func (w *Wallet) ToKeypairB58() string {
	return base58.Encode(w.privateKey)
}

// String returns a debug-safe representation that redacts the secret key.
func (w *Wallet) String() string {
	return fmt.Sprintf("Wallet(address=%s, secret=REDACTED)", w.Address())
}
