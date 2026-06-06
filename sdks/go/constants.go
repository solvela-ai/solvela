package solvela

const (
	// X402Version is the protocol version of the x402 payment scheme this SDK speaks.
	X402Version = 2

	// USDCMint is the SPL-token mint address of USDC on Solana mainnet-beta.
	USDCMint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"

	// SolanaNetwork is the x402 network identifier this SDK targets by default
	// (Solana mainnet-beta, encoded as "solana:<genesis-hash-prefix>").
	SolanaNetwork = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"

	// MainnetEscrowProgramID is the deployed Solvela escrow program on Solana
	// mainnet-beta. [DefaultConfig] pins [ClientConfig].ExpectedEscrowProgram to
	// this value so an escrow deposit is never signed for an unexpected program
	// (mirrors the hardcoded [USDCMint] / [SolanaNetwork]). Matches the Rust
	// SDK's MAINNET_ESCROW_PROGRAM_ID.
	MainnetEscrowProgramID = "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU"

	// MaxTimeoutSeconds is the upper bound (in seconds) enforced on
	// [ClientConfig].Timeout when constructing a client.
	MaxTimeoutSeconds = 300

	// PlatformFeePercent is the percentage of each payment that Solvela
	// retains as a platform fee; applied gateway-side, surfaced here for
	// callers that want to display fee-inclusive pricing.
	PlatformFeePercent = 5
)
