package solvela

import (
	"os"

	"github.com/mr-tron/base58"
)

// Wallet holds a Solana keypair for signing x402 payment transactions.
type Wallet struct {
	privateKey string
	address    string
}

// NewWallet creates a Wallet from a base58-encoded private key.
// If privateKey is empty, it falls back to the SOLANA_WALLET_KEY env var.
//
// The address field is populated only when the provided key decodes to a valid
// 64-byte ed25519 keypair ([seed(32) || pubkey(32)]). If the key is absent or
// invalid the wallet is created without a key (HasKey returns false).
func NewWallet(privateKey string) *Wallet {
	if privateKey == "" {
		privateKey = os.Getenv("SOLANA_WALLET_KEY")
	}

	w := &Wallet{privateKey: privateKey}

	// Populate the address field only when the key is a valid 64-byte
	// ed25519 keypair ([seed(32) || pubkey(32)]). An invalid or short key
	// is still stored so HasKey() returns true, but address stays empty.
	if kp, err := base58.Decode(privateKey); err == nil && len(kp) == 64 {
		w.address = base58.Encode(kp[32:64])
	}

	return w
}

// HasKey reports whether the wallet has a private key configured.
func (w *Wallet) HasKey() bool {
	return w.privateKey != ""
}

// Address returns the wallet's public address.
func (w *Wallet) Address() string {
	return w.address
}

// signExactPayment builds and signs a USDC-SPL TransferChecked transaction
// using this wallet's private key. The returned string is a base64-encoded
// wire transaction ready for submission.
//
// Returns a *SigningError if the wallet has no key or if signing fails.
func (w *Wallet) signExactPayment(payTo string, amount uint64) (string, error) {
	if !w.HasKey() {
		return "", &SigningError{Message: "wallet has no private key configured"}
	}
	return buildSolanaTransferChecked(payTo, amount, w.privateKey)
}

// signEscrowDeposit builds and signs an Anchor escrow deposit transaction
// using this wallet's private key. The params.AgentKeypairB58 field is
// populated automatically from the wallet — callers must leave it empty.
//
// Returns a *SigningError if the wallet has no key or if signing fails.
func (w *Wallet) signEscrowDeposit(params EscrowDepositParams) (BuildEscrowDepositResult, error) {
	if !w.HasKey() {
		return BuildEscrowDepositResult{}, &SigningError{Message: "wallet has no private key configured"}
	}
	params.AgentKeypairB58 = w.privateKey
	return BuildEscrowDeposit(params)
}
