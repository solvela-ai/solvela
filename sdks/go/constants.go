package solvela

const (
	// X402Version is the protocol version of the x402 payment scheme this SDK speaks.
	X402Version = 2

	// USDCMint is the SPL-token mint address of USDC on Solana mainnet-beta.
	USDCMint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"

	// SolanaNetwork is the x402 network identifier this SDK targets by default
	// (Solana mainnet-beta, encoded as "solana:<genesis-hash-prefix>").
	SolanaNetwork = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"

	// MaxTimeoutSeconds is the upper bound (in seconds) enforced on
	// [ClientConfig].Timeout when constructing a client.
	MaxTimeoutSeconds = 300

	// PlatformFeePercent is the percentage of each payment that Solvela
	// retains as a platform fee; applied gateway-side, surfaced here for
	// callers that want to display fee-inclusive pricing.
	PlatformFeePercent = 5
)
