"""Tests for ClientConfig and ClientBuilder."""

from __future__ import annotations

import pytest

from solvela.config import DEFAULT_MAX_PAYMENT_AMOUNT, ClientBuilder, ClientConfig
from solvela.errors import ClientError


class TestClientConfig:
    def test_default_config(self) -> None:
        cfg = ClientConfig()
        # Default points at the production HTTPS endpoint — never plain http://.
        assert cfg.gateway_url == "https://api.solvela.ai"
        assert cfg.rpc_url == "https://api.mainnet-beta.solana.com"
        assert cfg.prefer_escrow is False
        assert cfg.timeout == 180.0
        assert cfg.expected_recipient is None
        # Conservative default — 10 USDC cap unless caller opts into more.
        assert cfg.max_payment_amount == DEFAULT_MAX_PAYMENT_AMOUNT
        assert cfg.enable_cache is False
        assert cfg.enable_sessions is False
        assert cfg.session_ttl == 1800.0
        assert cfg.enable_quality_check is False
        assert cfg.max_quality_retries == 1
        assert cfg.free_fallback_model is None

    def test_default_max_payment_amount_value(self) -> None:
        # 10 USDC in atomic units (USDC has 6 decimals).
        assert DEFAULT_MAX_PAYMENT_AMOUNT == 10_000_000

    @pytest.mark.parametrize(
        "url",
        [
            "http://localhost:8402",
            "http://127.0.0.1:8402",
            "http://[::1]:8402",
            "https://api.solvela.ai",
            "https://gw.example.com",
        ],
    )
    def test_accepts_safe_gateway_urls(self, url: str) -> None:
        ClientConfig(gateway_url=url)  # must not raise

    @pytest.mark.parametrize(
        "url",
        [
            "http://api.solvela.ai",
            "http://gw.example.com",
            "http://10.0.0.1",
        ],
    )
    def test_rejects_plain_http_for_remote_hosts(self, url: str) -> None:
        with pytest.raises(ClientError, match="https://"):
            ClientConfig(gateway_url=url)

    @pytest.mark.parametrize(
        "url",
        [
            "http://localhost:8899",
            "http://127.0.0.1:8899",
            "https://api.mainnet-beta.solana.com",
            "https://rpc.example.com",
        ],
    )
    def test_accepts_safe_rpc_urls(self, url: str) -> None:
        ClientConfig(rpc_url=url)  # must not raise

    @pytest.mark.parametrize(
        "url",
        [
            "http://api.mainnet-beta.solana.com",
            "http://rpc.example.com",
            "http://10.0.0.5",
        ],
    )
    def test_rejects_plain_http_rpc_for_remote_hosts(self, url: str) -> None:
        # Plaintext blockhash fetch lets an on-path attacker tamper with the
        # signed transaction. rpc_url must enforce the same rule as gateway_url.
        with pytest.raises(ClientError, match="rpc_url"):
            ClientConfig(rpc_url=url)


class TestClientBuilder:
    def test_builder_fluent(self) -> None:
        cfg = (
            ClientBuilder()
            .gateway_url("https://gw.example.com")
            .rpc_url("https://rpc.example.com")
            .prefer_escrow(True)
            .timeout(60.0)
            .expected_recipient("RecipABC")
            .max_payment_amount(500_000)
            .enable_cache(True)
            .enable_sessions(True)
            .session_ttl(3600.0)
            .enable_quality_check(True)
            .max_quality_retries(3)
            .free_fallback_model("gpt-4o-mini")
            .build()
        )
        assert cfg.gateway_url == "https://gw.example.com"
        assert cfg.rpc_url == "https://rpc.example.com"
        assert cfg.prefer_escrow is True
        assert cfg.timeout == 60.0
        assert cfg.expected_recipient == "RecipABC"
        assert cfg.max_payment_amount == 500_000
        assert cfg.enable_cache is True
        assert cfg.enable_sessions is True
        assert cfg.session_ttl == 3600.0
        assert cfg.enable_quality_check is True
        assert cfg.max_quality_retries == 3
        assert cfg.free_fallback_model == "gpt-4o-mini"

    def test_builder_default_matches_config_default(self) -> None:
        assert ClientBuilder().build() == ClientConfig()

    def test_builder_rejects_plain_http_for_remote_hosts(self) -> None:
        with pytest.raises(ClientError, match="https://"):
            ClientBuilder().gateway_url("http://gw.example.com")

    def test_builder_rejects_plain_http_rpc_for_remote_hosts(self) -> None:
        with pytest.raises(ClientError, match="rpc_url"):
            ClientBuilder().rpc_url("http://rpc.example.com")
