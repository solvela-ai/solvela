"""Tests for solvela.errors."""

from __future__ import annotations

from solvela.errors import (
    AmountExceedsMaxError,
    ClientError,
    GatewayError,
    InsufficientBalanceError,
    PaymentRejectedError,
    PaymentRequiredError,
    RecipientMismatchError,
    SignerError,
    TimeoutError,
    WalletError,
)
from solvela.types import (
    AtomicUsdc,
    CostBreakdown,
    PaymentAccept,
    PaymentRequired,
    Resource,
)


class TestErrorHierarchy:
    def test_wallet_error_is_client_error(self) -> None:
        assert issubclass(WalletError, ClientError)

    def test_signer_error_is_client_error(self) -> None:
        assert issubclass(SignerError, ClientError)

    def test_insufficient_balance_is_client_error(self) -> None:
        assert issubclass(InsufficientBalanceError, ClientError)

    def test_gateway_error_is_client_error(self) -> None:
        assert issubclass(GatewayError, ClientError)

    def test_payment_required_error_is_client_error(self) -> None:
        assert issubclass(PaymentRequiredError, ClientError)

    def test_payment_rejected_error_is_client_error(self) -> None:
        assert issubclass(PaymentRejectedError, ClientError)

    def test_recipient_mismatch_is_client_error(self) -> None:
        assert issubclass(RecipientMismatchError, ClientError)

    def test_amount_exceeds_max_is_client_error(self) -> None:
        assert issubclass(AmountExceedsMaxError, ClientError)

    def test_timeout_error_is_client_error(self) -> None:
        assert issubclass(TimeoutError, ClientError)


class TestInsufficientBalanceError:
    def test_attributes(self) -> None:
        err = InsufficientBalanceError(have=AtomicUsdc(1000), need=AtomicUsdc(5000))
        assert err.have == 1000
        assert err.need == 5000

    def test_message(self) -> None:
        err = InsufficientBalanceError(have=AtomicUsdc(1000), need=AtomicUsdc(5000))
        assert "1000" in str(err)
        assert "5000" in str(err)


class TestGatewayError:
    def test_attributes(self) -> None:
        err = GatewayError(status=500, message="Internal server error")
        assert err.status == 500
        assert err.message == "Internal server error"

    def test_message(self) -> None:
        err = GatewayError(status=500, message="Internal server error")
        assert "500" in str(err)
        assert "Internal server error" in str(err)


class TestPaymentRequiredError:
    def test_attributes(self) -> None:
        pr = PaymentRequired(
            x402_version=2,
            resource=Resource(url="https://example.com", method="POST"),
            accepts=[
                PaymentAccept(
                    scheme="exact",
                    network="solana:mainnet",
                    amount="50000",
                    asset="USDC",
                    pay_to="wallet123",
                    max_timeout_seconds=300,
                )
            ],
            cost_breakdown=CostBreakdown(
                provider_cost="45000",
                platform_fee="5000",
                total="50000",
                currency="USDC",
                fee_percent=5,
            ),
            error="Payment required",
        )
        err = PaymentRequiredError(payment_required=pr)
        assert err.payment_required is pr
        assert "50000" in str(err)


class TestPaymentRejectedError:
    def test_attributes(self) -> None:
        err = PaymentRejectedError(reason="Invalid signature")
        assert err.reason == "Invalid signature"
        assert "Invalid signature" in str(err)


class TestRecipientMismatchError:
    def test_attributes(self) -> None:
        err = RecipientMismatchError(expected="wallet_a", actual="wallet_b")
        assert err.expected == "wallet_a"
        assert err.actual == "wallet_b"
        assert "wallet_a" in str(err)
        assert "wallet_b" in str(err)


class TestAmountExceedsMaxError:
    def test_attributes(self) -> None:
        err = AmountExceedsMaxError(amount=AtomicUsdc(100000), max_amount=AtomicUsdc(50000))
        assert err.amount == 100000
        assert err.max_amount == 50000
        assert "100000" in str(err)
        assert "50000" in str(err)


class TestTimeoutError:
    def test_attributes(self) -> None:
        err = TimeoutError(timeout_secs=30.5)
        assert err.timeout_secs == 30.5
        assert "30.5" in str(err)


class TestWalletAndSignerErrors:
    def test_wallet_error(self) -> None:
        err = WalletError("Failed to load wallet")
        assert "Failed to load wallet" in str(err)

    def test_signer_error(self) -> None:
        err = SignerError("Signing failed")
        assert "Signing failed" in str(err)
