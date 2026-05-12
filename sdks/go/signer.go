package solvela

import "context"

// Signer is a pluggable interface for signing payment transactions.
type Signer interface {
	SignPayment(ctx context.Context, amountAtomic uint64, recipient string, resource Resource, accepted PaymentAccept) (*PaymentPayload, error)
}

// Compile-time assertion that UnimplementedSigner satisfies Signer. If the
// interface drifts (e.g., a method is renamed), the build fails here rather
// than at first runtime call.
var _ Signer = (*UnimplementedSigner)(nil)

// UnimplementedSigner is a placeholder Signer for callers who do not yet
// have a real Solana USDC-SPL signing implementation. Its SignPayment
// method always returns an error. Construct your own Signer implementation
// (or use a future built-in signer) before sending paid requests.
type UnimplementedSigner struct {
	wallet *Wallet
	rpcURL string
}

// NewUnimplementedSigner creates a stub [Signer] from a wallet and optional
// Solana RPC URL. If rpcURL is empty, defaults to mainnet-beta. The returned
// signer's SignPayment always returns an error; see [UnimplementedSigner].
func NewUnimplementedSigner(wallet *Wallet, rpcURL string) *UnimplementedSigner {
	if rpcURL == "" {
		rpcURL = "https://api.mainnet-beta.solana.com"
	}
	return &UnimplementedSigner{wallet: wallet, rpcURL: rpcURL}
}

// SignPayment is not yet implemented in solvela-go.
//
// Use the Python SDK (solvela-python) or TypeScript SDK (solvela-ts) for
// production payment signing, or implement a custom [Signer] using the
// standard library's crypto/ed25519 package and a Solana JSON-RPC client
// of your choice.
func (s *UnimplementedSigner) SignPayment(_ context.Context, _ uint64, _ string, _ Resource, _ PaymentAccept) (*PaymentPayload, error) {
	return nil, &SignerError{Message: "UnimplementedSigner.SignPayment is not yet implemented in solvela-go; use the Python or TS SDK, or implement a custom Signer"}
}

// KeypairSigner is a deprecated alias for [UnimplementedSigner].
//
// Deprecated: renamed to UnimplementedSigner because the original name
// implied a working keypair-based signer; the type is in fact a stub. Use
// [UnimplementedSigner] directly. This alias will be removed in a future
// release.
type KeypairSigner = UnimplementedSigner

// NewKeypairSigner is a deprecated alias for [NewUnimplementedSigner].
//
// Deprecated: see [KeypairSigner].
func NewKeypairSigner(wallet *Wallet, rpcURL string) *UnimplementedSigner {
	return NewUnimplementedSigner(wallet, rpcURL)
}
