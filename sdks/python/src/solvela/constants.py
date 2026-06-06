"""Solvela protocol constants."""

from __future__ import annotations

X402_VERSION: int = 2
USDC_MINT: str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
SOLANA_NETWORK: str = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"
# Deployed Solvela escrow program on Solana mainnet-beta. ``ClientConfig`` pins
# ``expected_escrow_program_id`` to this by default so an escrow deposit is never
# signed for an unexpected program (mirrors the hardcoded ``USDC_MINT`` /
# ``SOLANA_NETWORK``). Matches the Rust SDK's ``MAINNET_ESCROW_PROGRAM_ID``.
MAINNET_ESCROW_PROGRAM_ID: str = "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU"
MAX_TIMEOUT_SECONDS: int = 300
PLATFORM_FEE_PERCENT: int = 5
