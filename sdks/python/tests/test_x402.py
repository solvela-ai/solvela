"""Tests for x402 protocol helpers including escrow scheme support."""

import base64
import json

import pytest

from solvela.types import CostBreakdown, PaymentAccept, PaymentRequired
from solvela.x402 import (
    build_escrow_payment_payload,
    decode_payment_header,
    encode_payment_header,
)

ESCROW_PROGRAM_ID = "Fg6PaFpoGXkYsidMpWTK6W2BeZ7FEfcYkg476zPFsLnS"


class TestPaymentAcceptWithEscrowProgramId:
    def test_escrow_program_id_field_present(self):
        accept = PaymentAccept(
            scheme="escrow",
            network="solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
            amount="2625",
            asset="EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            pay_to="RecipientWalletPubkey",
            max_timeout_seconds=300,
            escrow_program_id=ESCROW_PROGRAM_ID,
        )
        assert accept.escrow_program_id == ESCROW_PROGRAM_ID

    def test_escrow_program_id_defaults_to_none(self):
        accept = PaymentAccept(
            scheme="exact",
            network="solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
            amount="2625",
            asset="EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            pay_to="RecipientWalletPubkey",
            max_timeout_seconds=300,
        )
        assert accept.escrow_program_id is None

    def test_escrow_accept_roundtrip(self):
        data = {
            "scheme": "escrow",
            "network": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
            "amount": "2625",
            "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            "pay_to": "RecipientWalletPubkey",
            "max_timeout_seconds": 300,
            "escrow_program_id": ESCROW_PROGRAM_ID,
        }
        accept = PaymentAccept.model_validate(data)
        assert accept.escrow_program_id == ESCROW_PROGRAM_ID
        dumped = accept.model_dump()
        assert dumped["escrow_program_id"] == ESCROW_PROGRAM_ID

    def test_payment_required_with_escrow_accept(self):
        pr = PaymentRequired(
            x402_version=2,
            accepts=[
                PaymentAccept(
                    scheme="escrow",
                    network="solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
                    amount="2625",
                    asset="EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                    pay_to="RecipientWalletPubkey",
                    max_timeout_seconds=300,
                    escrow_program_id=ESCROW_PROGRAM_ID,
                )
            ],
            cost_breakdown=CostBreakdown(
                provider_cost="0.002500",
                platform_fee="0.000125",
                total="0.002625",
                currency="USDC",
                fee_percent=5,
            ),
            error="Payment required",
        )
        assert pr.accepts[0].escrow_program_id == ESCROW_PROGRAM_ID


class TestEncodePaymentHeaderEscrowScheme:
    def _make_escrow_accept(self) -> PaymentAccept:
        return PaymentAccept(
            scheme="escrow",
            network="solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
            amount="2625",
            asset="EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            pay_to="RecipientWalletPubkey",
            max_timeout_seconds=300,
            escrow_program_id=ESCROW_PROGRAM_ID,
        )

    def test_escrow_scheme_produces_valid_header(self):
        accept = self._make_escrow_accept()
        header = encode_payment_header(
            accept,
            "http://localhost:8402/v1/chat/completions",
            private_key=None,
        )
        assert isinstance(header, str)
        assert len(header) > 0

    def test_escrow_scheme_header_is_base64(self):
        accept = self._make_escrow_accept()
        header = encode_payment_header(
            accept,
            "http://localhost:8402/v1/chat/completions",
            private_key=None,
        )
        # Should decode without error
        decoded = base64.b64decode(header)
        data = json.loads(decoded)
        assert isinstance(data, dict)

    def test_escrow_payload_contains_required_fields(self):
        accept = self._make_escrow_accept()
        header = encode_payment_header(
            accept,
            "http://localhost:8402/v1/chat/completions",
            private_key=None,
            request_body=b'{"model":"gpt-4o","messages":[]}',
        )
        decoded = json.loads(base64.b64decode(header))
        payload = decoded["payload"]
        assert "deposit_tx" in payload
        assert "service_id" in payload
        assert "agent_pubkey" in payload

    def test_escrow_stub_values_no_private_key(self):
        accept = self._make_escrow_accept()
        header = encode_payment_header(
            accept,
            "http://localhost:8402/v1/chat/completions",
            private_key=None,
        )
        decoded = json.loads(base64.b64decode(header))
        payload = decoded["payload"]
        assert payload["deposit_tx"] == "STUB_ESCROW_DEPOSIT_TX"
        assert payload["agent_pubkey"] == "STUB_AGENT_PUBKEY"

    def test_escrow_service_id_is_base64_string(self):
        accept = self._make_escrow_accept()
        header = encode_payment_header(
            accept,
            "http://localhost:8402/v1/chat/completions",
            private_key=None,
            request_body=b"test body",
        )
        decoded = json.loads(base64.b64decode(header))
        service_id_b64 = decoded["payload"]["service_id"]
        # Must be a valid base64 string decoding to 32 bytes
        assert isinstance(service_id_b64, str)
        decoded_bytes = base64.b64decode(service_id_b64)
        assert len(decoded_bytes) == 32

    def test_escrow_service_id_is_random_per_call(self):
        accept = self._make_escrow_accept()
        body = b'{"model":"gpt-4o"}'
        header1 = encode_payment_header(
            accept, "http://localhost:8402/v1/chat/completions",
            private_key=None, request_body=body,
        )
        header2 = encode_payment_header(
            accept, "http://localhost:8402/v1/chat/completions",
            private_key=None, request_body=body,
        )
        sid1 = json.loads(base64.b64decode(header1))["payload"]["service_id"]
        sid2 = json.loads(base64.b64decode(header2))["payload"]["service_id"]
        assert sid1 != sid2  # os.urandom(8) ensures uniqueness

    def test_escrow_header_contains_x402_version(self):
        accept = self._make_escrow_accept()
        header = encode_payment_header(
            accept,
            "http://localhost:8402/v1/chat/completions",
            private_key=None,
        )
        decoded = json.loads(base64.b64decode(header))
        assert "x402_version" in decoded
        assert isinstance(decoded["x402_version"], int)

    def test_escrow_header_contains_resource(self):
        accept = self._make_escrow_accept()
        url = "http://localhost:8402/v1/chat/completions"
        header = encode_payment_header(accept, url, private_key=None)
        decoded = json.loads(base64.b64decode(header))
        assert decoded["resource"]["url"] == url
        assert decoded["resource"]["method"] == "POST"

    def test_escrow_header_contains_accepted(self):
        accept = self._make_escrow_accept()
        header = encode_payment_header(
            accept,
            "http://localhost:8402/v1/chat/completions",
            private_key=None,
        )
        decoded = json.loads(base64.b64decode(header))
        assert decoded["accepted"]["scheme"] == "escrow"
        assert decoded["accepted"]["escrow_program_id"] == ESCROW_PROGRAM_ID

    def test_exact_scheme_unchanged_with_request_body_param(self):
        """Existing exact scheme must still work when request_body is passed."""
        accept = PaymentAccept(
            scheme="exact",
            network="solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
            amount="2625",
            asset="EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            pay_to="RecipientWalletPubkey",
            max_timeout_seconds=300,
        )
        header = encode_payment_header(
            accept,
            "http://localhost:8402/v1/chat/completions",
            private_key=None,
            request_body=b'{"model":"gpt-4o"}',
        )
        decoded = json.loads(base64.b64decode(header))
        assert decoded["payload"]["transaction"] == "STUB_BASE64_TX"

    def test_exact_scheme_backwards_compatible_no_request_body(self):
        """encode_payment_header with no request_body still works (exact scheme)."""
        accept = PaymentAccept(
            scheme="exact",
            network="solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
            amount="2625",
            asset="EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            pay_to="RecipientWalletPubkey",
            max_timeout_seconds=300,
        )
        header = encode_payment_header(
            accept,
            "http://localhost:8402/v1/chat/completions",
        )
        decoded = json.loads(base64.b64decode(header))
        assert decoded["payload"]["transaction"] == "STUB_BASE64_TX"


class TestBuildEscrowPaymentPayload:
    def test_basic_payload(self):
        service_id = bytes(range(32))
        payload = build_escrow_payment_payload(
            deposit_tx="STUB_TX",
            service_id=service_id,
            agent_pubkey="AgentPubkeyBase58",
        )
        assert payload["deposit_tx"] == "STUB_TX"
        assert payload["agent_pubkey"] == "AgentPubkeyBase58"
        assert payload["service_id"] == base64.b64encode(service_id).decode()

    def test_service_id_base64_decodes_to_32_bytes(self):
        service_id = b"\xde\xad\xbe\xef" * 8
        payload = build_escrow_payment_payload(
            deposit_tx="TX",
            service_id=service_id,
            agent_pubkey="PK",
        )
        assert len(base64.b64decode(payload["service_id"])) == 32
