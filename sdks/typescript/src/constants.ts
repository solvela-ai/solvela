export const X402_VERSION = 2;
export const USDC_MINT = 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v';
export const SOLANA_NETWORK = 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp';
/**
 * Deployed Solvela escrow program on Solana mainnet-beta. `DEFAULT_CONFIG` pins
 * `expectedEscrowProgram` to this so an escrow deposit is never signed for an
 * unexpected program (mirrors the hardcoded {@link USDC_MINT} /
 * {@link SOLANA_NETWORK}). Matches the Rust SDK's `MAINNET_ESCROW_PROGRAM_ID`.
 */
export const MAINNET_ESCROW_PROGRAM_ID = '9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU';
export const MAX_TIMEOUT_SECONDS = 300;
export const PLATFORM_FEE_PERCENT = 5;
