"""Tests for SDK configuration constants."""

from rustyclawrouter.config import (
    DEFAULT_API_URL,
    DEFAULT_DEVNET_URL,
    SOLANA_DEVNET_RPC,
    SOLANA_MAINNET_RPC,
    SOLANA_NETWORK,
    USDC_MINT,
    X402_VERSION,
)


class TestConfigConstants:
    def test_default_api_url(self):
        assert DEFAULT_API_URL == "https://api.solvela.ai"

    def test_default_devnet_url(self):
        assert DEFAULT_DEVNET_URL == "http://localhost:8402"

    def test_solana_mainnet_rpc(self):
        assert SOLANA_MAINNET_RPC == "https://api.mainnet-beta.solana.com"

    def test_solana_devnet_rpc(self):
        assert SOLANA_DEVNET_RPC == "https://api.devnet.solana.com"

    def test_usdc_mint(self):
        # Real USDC-SPL mint address on Solana mainnet
        assert USDC_MINT == "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
        assert len(USDC_MINT) > 30  # Base58 pubkey length

    def test_solana_network(self):
        assert SOLANA_NETWORK == "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"
        assert SOLANA_NETWORK.startswith("solana:")

    def test_x402_version(self):
        assert X402_VERSION == 2
        assert isinstance(X402_VERSION, int)
