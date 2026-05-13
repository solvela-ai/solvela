"""Unit tests for Transport — URL/header construction."""

from __future__ import annotations

from solvela.transport import Transport


class TestBuildUrl:
    def test_build_url(self) -> None:
        t = Transport("https://gw.example.com")
        assert t._build_url("/v1/chat/completions") == "https://gw.example.com/v1/chat/completions"

    def test_build_url_strips_trailing_slash(self) -> None:
        t = Transport("https://gw.example.com/")
        assert t._build_url("/v1/models") == "https://gw.example.com/v1/models"


class TestBuildHeaders:
    def test_build_headers_without_payment(self) -> None:
        t = Transport("https://gw.example.com")
        headers = t._build_headers()
        assert headers == {"Content-Type": "application/json"}
        assert "Payment-Signature" not in headers

    def test_build_headers_with_payment(self) -> None:
        t = Transport("https://gw.example.com")
        headers = t._build_headers(payment_signature="sig123")
        assert headers["Payment-Signature"] == "sig123"
        assert headers["Content-Type"] == "application/json"

    def test_build_headers_with_extra(self) -> None:
        t = Transport("https://gw.example.com")
        headers = t._build_headers(extra_headers={"X-Custom": "val"})
        assert headers["X-Custom"] == "val"
        assert headers["Content-Type"] == "application/json"

    def test_build_headers_with_payment_and_extra(self) -> None:
        t = Transport("https://gw.example.com")
        headers = t._build_headers(
            payment_signature="sig",
            extra_headers={"X-Foo": "bar"},
        )
        assert headers["Payment-Signature"] == "sig"
        assert headers["X-Foo"] == "bar"
