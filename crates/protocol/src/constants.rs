/// x402 protocol version.
pub const X402_VERSION: u8 = 2;

/// USDC-SPL mint address on Solana mainnet.
pub const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

/// Solana mainnet network identifier for x402.
pub const SOLANA_NETWORK: &str = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp";

/// Solvela escrow program ID deployed on Solana mainnet.
///
/// Pinned alongside [`USDC_MINT`] and [`SOLANA_NETWORK`] so clients can enforce
/// the expected escrow program by default and reject a malicious gateway that
/// advertises a different (attacker-controlled) escrow program.
pub const MAINNET_ESCROW_PROGRAM_ID: &str = "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU";

/// Maximum timeout for payment authorization (5 minutes).
pub const MAX_TIMEOUT_SECONDS: u64 = 300;

/// The platform fee multiplier (1.05 = provider cost + 5%).
pub const PLATFORM_FEE_MULTIPLIER: f64 = 1.05;

/// Platform fee percentage.
pub const PLATFORM_FEE_PERCENT: u8 = 5;
