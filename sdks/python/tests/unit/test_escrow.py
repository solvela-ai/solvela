"""Unit tests for the escrow deposit-transaction builder.

The golden-vector parity test is the contract: the Python deposit builder MUST
reproduce ``crates/escrow-tx/src/deposit.rs``'s ``GOLDEN_VECTOR_B64`` byte-for-
byte for its fixed input. If it diverges, the on-chain layout disagreement is a
fund-misdirection bug — fix the Python builder, NEVER edit the expected golden
value (the Rust/on-chain value is ground truth).
"""

from __future__ import annotations

import base64

import pytest
from solders.keypair import Keypair  # type: ignore[import-untyped]
from solders.pubkey import Pubkey  # type: ignore[import-untyped]

from solvela.escrow import (
    DepositError,
    DepositParams,
    anchor_discriminator,
    build_deposit_tx,
    derive_ata_address,
    derive_escrow_pda,
)

# ---------------------------------------------------------------------------
# Golden vector — byte-exact match against crates/escrow-tx/src/deposit.rs.
# ---------------------------------------------------------------------------

# Copied verbatim from `GOLDEN_VECTOR_B64` in crates/escrow-tx/src/deposit.rs.
# DO NOT hand-edit. If this changes, the deposit-tx layout changed.
GOLDEN_VECTOR_B64 = "Ad449R7ht12UKokgbDUYh29Ey/wDEwPioT/QtJOPKwSO4RHIaFRqS0DH+bPpEo2vCK78jbRLpP/6FV/5TY+qLw8BAAYKGX9rI+FshTLGq8g4+s1ep4m+DHaykgM0A5v6iz02jWFyvMLyv5o/k5XBGVxL9QMaEsQP6v01aK7nXazb0N/wMBS12eVticTdq8DzgM47jC/J8D1uNnFG6ZNqwpSU6GOelSnAbR4dY/56biCP6roeTxLQ8Q4TL7a2ArnBn2RW9HJ+jAiHYL/eHd3PMsF/IJuCQu5SqvEx+s2I0OosbQsG8sb6evO+2606PWXzaqvJdDGxu+TC0vbg5HymAgNFL11hBt324ddloZPZy+FGzut5rBy0he1fWzeROoz1hX7/AKmMlyWPTiSJ8bs9ECkUjg2DC1oTmdr/EIQEjnvY2+n4WQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAgo61GtoIM8r3akxjdsa2N5CEHZ8QBYuAbfwPDOL78eGrq6urq6urq6urq6urq6urq6urq6urq6urq6urq6urqwEJCQAEBQECAwYHCDjyI8aJUuHytkEKAAAAAAAABwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcuRQ8AAAAAAA=="  # noqa: E501

PROVIDER = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM"
USDC_MINT = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
ESCROW_PROGRAM = "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU"


def _golden_keypair_b58() -> str:
    """64-byte keypair from seed [42] * 32 (matches the Rust golden fixture)."""
    kp = Keypair.from_seed(bytes([42] * 32))
    return str(kp)  # base58 64-byte keypair string


def _golden_params() -> DepositParams:
    return DepositParams(
        agent_keypair_b58=_golden_keypair_b58(),
        provider_wallet_b58=PROVIDER,
        usdc_mint_b58=USDC_MINT,
        escrow_program_id_b58=ESCROW_PROGRAM,
        amount=2625,
        service_id=bytes([7] * 32),
        expiry_slot=1_000_750,
        recent_blockhash=bytes([0xAB] * 32),
    )


class TestGoldenVector:
    def test_deposit_tx_matches_rust_golden_vector(self) -> None:
        """Byte-for-byte parity with the canonical Rust builder.

        This is the sole correctness bar for the wire format.
        """
        out = build_deposit_tx(_golden_params())
        assert out == GOLDEN_VECTOR_B64, (
            "Python deposit-tx does not match the Rust golden vector. "
            "The on-chain layout is ground truth — fix the Python builder, "
            "do NOT edit the expected value."
        )

    def test_golden_vector_decodes_to_signed_tx(self) -> None:
        raw = base64.b64decode(GOLDEN_VECTOR_B64)
        # compact-u16(1) || sig(64) || message
        assert raw[0] == 0x01
        assert len(raw) > 1 + 64


# ---------------------------------------------------------------------------
# Derivation parity (PDA seeds + vault ATA).
# ---------------------------------------------------------------------------


class TestDerivation:
    def test_escrow_pda_external_ground_truth(self) -> None:
        # Mirrors crates/escrow-tx/src/pda.rs ground-truth test:
        # agent=[1]*32, service_id=[2]*32 -> BEAUsvsWvV4o6y7XkC1bkyTq4FtQnKErcV3dzTFPT5hX, bump 255.
        program = Pubkey.from_string(ESCROW_PROGRAM)
        pda, bump = derive_escrow_pda(bytes([1] * 32), bytes([2] * 32), program)
        assert str(pda) == "BEAUsvsWvV4o6y7XkC1bkyTq4FtQnKErcV3dzTFPT5hX"
        assert bump == 255

    def test_pda_seeds_are_escrow_agent_service_id(self) -> None:
        program = Pubkey.from_string(ESCROW_PROGRAM)
        agent = bytes([3] * 32)
        service_id = bytes([9] * 32)
        pda, _ = derive_escrow_pda(agent, service_id, program)
        # Recompute with solders directly using the documented seed order.
        expected, _ = Pubkey.find_program_address([b"escrow", agent, service_id], program)
        assert pda == expected

    def test_vault_ata_known_value(self) -> None:
        # Mirrors crates/escrow-tx/src/pda.rs test_derive_ata_for_known_wallet.
        wallet = Pubkey.from_string("4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp")
        mint = Pubkey.from_string(USDC_MINT)
        ata = derive_ata_address(wallet, mint)
        assert str(ata) == "CYHVCkLwiEjMBdRiz5MsrrCbVL2YTZuv57TjV3ggxoSN"

    def test_anchor_discriminator_deterministic(self) -> None:
        d1 = anchor_discriminator("deposit")
        d2 = anchor_discriminator("deposit")
        assert d1 == d2
        assert len(d1) == 8
        assert anchor_discriminator("deposit") != anchor_discriminator("claim")


# ---------------------------------------------------------------------------
# Rejection / fail-closed behavior.
# ---------------------------------------------------------------------------


class TestRejections:
    def test_zero_amount_rejected(self) -> None:
        params = _golden_params()
        params.amount = 0
        with pytest.raises(DepositError, match="zero"):
            build_deposit_tx(params)

    def test_invalid_keypair_rejected(self) -> None:
        params = _golden_params()
        params.agent_keypair_b58 = "notavalidkeypair!!!"
        with pytest.raises(DepositError):
            build_deposit_tx(params)

    def test_invalid_provider_rejected(self) -> None:
        params = _golden_params()
        params.provider_wallet_b58 = "not-base58!!!"
        with pytest.raises(DepositError):
            build_deposit_tx(params)

    def test_bad_service_id_length_rejected(self) -> None:
        params = _golden_params()
        params.service_id = bytes([7] * 31)
        with pytest.raises(DepositError):
            build_deposit_tx(params)

    def test_bad_blockhash_length_rejected(self) -> None:
        params = _golden_params()
        params.recent_blockhash = bytes([0xAB] * 31)
        with pytest.raises(DepositError):
            build_deposit_tx(params)
