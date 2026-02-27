package rcr

import "os"

// Wallet holds a Solana keypair for signing x402 payment transactions.
type Wallet struct {
	privateKey string
	address    string
}

// NewWallet creates a Wallet from a base58-encoded private key.
// If privateKey is empty, it falls back to the SOLANA_WALLET_KEY env var.
func NewWallet(privateKey string) *Wallet {
	if privateKey == "" {
		privateKey = os.Getenv("SOLANA_WALLET_KEY")
	}
	return &Wallet{privateKey: privateKey}
}

// HasKey reports whether the wallet has a private key configured.
func (w *Wallet) HasKey() bool {
	return w.privateKey != ""
}

// Address returns the wallet's public address.
func (w *Wallet) Address() string {
	return w.address
}
