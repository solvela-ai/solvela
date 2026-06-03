"""Escrow deposit-transaction builder for the Python SDK.

Byte-exact port of the canonical Rust builder ``crates/escrow-tx/src/deposit.rs``
(+ ``pda.rs``). The output MUST reproduce ``GOLDEN_VECTOR_B64`` for the fixed
golden-vector input; the gateway verifier (``crates/x402/src/escrow/``)
independently re-derives the same layout, so any divergence here is a
fund-misdirection bug, not a style nit.

Wire layout (single source of truth — see the Rust file and the ``solvela-x402``
skill):

* Escrow PDA seeds: ``[b"escrow", agent_pubkey(32), service_id(32)]``.
* Vault ATA: ``ATA(escrow_pda, usdc_mint)``.
* Instruction data (56 bytes): ``anchor_discriminator("deposit")[8]`` ``||``
  ``amount: u64 LE`` ``||`` ``service_id: [u8;32]`` ``||`` ``expiry_slot: u64 LE``.
* Account list sorted by writability; program key appended last (index 9).
* Instruction account-index order: ``[0, 4, 5, 1, 2, 3, 6, 7, 8]``.
* Legacy message header ``[1, 0, 6]``; length prefixes are single-byte
  compact-u16 (valid only for lengths <= 127 — reject larger rather than
  truncating).
* Wire tx: ``compact-u16(1) || ed25519_signature(64) || message`` then base64
  (standard alphabet).

We deliberately do NOT use ``solders.message.Message`` /
``solders.transaction.Transaction`` to assemble the message: those compute their
own account ordering and would not match the canonical writability-sorted layout
byte-for-byte. We use ``solders`` only for the primitives whose output is
canonical and already pinned as ground truth: ``Pubkey.find_program_address``
(PDA + ATA derivation) and ``Keypair`` ed25519 signing.

Library: solders >= 0.21, < 0.30 (declared in pyproject.toml; tested against
0.27.1). Anchor discriminator and account ordering are computed locally with the
stdlib ``hashlib``.
"""

from __future__ import annotations

import base64
import hashlib
from dataclasses import dataclass

from solders.keypair import Keypair
from solders.pubkey import Pubkey

from solvela.errors import SignerError

# ---------------------------------------------------------------------------
# Well-known program constants (match crates/escrow-tx/src/pda.rs).
# ---------------------------------------------------------------------------

ATA_PROGRAM_ID = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"
TOKEN_PROGRAM_ID = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
SYSTEM_PROGRAM_ID = "11111111111111111111111111111111"

# Single-byte compact-u16 limit. Lengths above this would need a continuation
# byte; emitting a bare u8 there would corrupt the encoding, so we reject.
_COMPACT_U16_SINGLE_BYTE_MAX = 127

# Maximum value representable as a little-endian u64 (the on-chain amount field).
_U64_MAX = 0xFFFF_FFFF_FFFF_FFFF


def _is_strict_int(value: object) -> bool:
    """True only for a real ``int`` — rejects ``float`` and ``bool``.

    Money-path guard: ``bool`` is an ``int`` subclass, and a ``float`` amount
    would be silently truncated by ``int(...)`` / ``.to_bytes`` and mis-charge
    the agent. Both must be refused at the boundary.
    """
    return isinstance(value, int) and not isinstance(value, bool)


class DepositError(SignerError):
    """Raised when an escrow deposit transaction cannot be built.

    Subclasses ``SignerError`` so it stays inside the SDK's typed error
    hierarchy (callers using ``except SignerError`` / ``except ClientError``
    catch it).
    """


# ---------------------------------------------------------------------------
# Derivation helpers (solders provides the canonical primitives).
# ---------------------------------------------------------------------------


def anchor_discriminator(name: str) -> bytes:
    """Compute the Anchor instruction discriminator: ``sha256("global:<name>")[..8]``."""
    return hashlib.sha256(f"global:{name}".encode()).digest()[:8]


def derive_escrow_pda(
    agent_pubkey: bytes, service_id: bytes, program_id: Pubkey
) -> tuple[Pubkey, int]:
    """Derive the escrow PDA from seeds ``[b"escrow", agent, service_id]``.

    Returns ``(pda, bump)``. Delegates to ``solders`` /
    ``solana-program``'s ``find_program_address``, which is the canonical
    on-chain algorithm (pinned as ground truth in the Rust test suite).
    """
    pda, bump = Pubkey.find_program_address([b"escrow", agent_pubkey, service_id], program_id)
    return pda, bump


def derive_ata_address(wallet: Pubkey, mint: Pubkey) -> Pubkey:
    """Derive the Associated Token Account address for a wallet and mint."""
    ata_program = Pubkey.from_string(ATA_PROGRAM_ID)
    token_program = Pubkey.from_string(TOKEN_PROGRAM_ID)
    derived, _bump = Pubkey.find_program_address(
        [bytes(wallet), bytes(token_program), bytes(mint)],
        ata_program,
    )
    return derived


# ---------------------------------------------------------------------------
# Parameters
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class DepositParams:
    """Inputs for :func:`build_deposit_tx`.

    The ``service_id``, ``expiry_slot``, and ``recent_blockhash`` are injected
    so the builder is a pure, deterministic function (golden-vector testable).
    Production code (``KeypairSigner.sign_payment``) generates the CSPRNG
    ``service_id``, computes ``expiry_slot`` from the current slot, and fetches
    the blockhash, then calls this builder.

    Frozen + validated at construction: a money-path struct must not be mutated
    after the byte-layout-relevant fields are checked. To produce a variant
    (e.g. in tests), use ``dataclasses.replace(params, ...)`` which re-runs
    validation, never attribute assignment.

    .. warning::
        ``agent_keypair_b58`` is secret key material that is **NOT wiped from
        Python heap memory**. Unlike the Rust builder (which holds the key in a
        ``Zeroizing<String>`` zeroed on drop), CPython cannot reliably overwrite
        an immutable ``str`` — copies may linger in the interned-string table,
        f-string buffers, and GC arenas until reclaimed. Callers MUST keep a
        ``DepositParams`` short-lived (build the tx, then drop it) and MUST NEVER
        store it in a cache, module-global, or long-lived attribute. No ctypes
        scrub is attempted here — it would give false assurance.
    """

    # Base58-encoded 64-byte ed25519 keypair (32-byte secret || 32-byte pubkey).
    agent_keypair_b58: str
    provider_wallet_b58: str
    usdc_mint_b58: str
    escrow_program_id_b58: str
    # Atomic USDC units (6 decimals). MUST be a positive int (no float/bool).
    amount: int
    # 32-byte service identifier that seeds the escrow PDA.
    service_id: bytes
    # Slot at which the escrow deposit expires.
    expiry_slot: int
    # Recent blockhash (32 bytes) from getLatestBlockhash.
    recent_blockhash: bytes

    def __post_init__(self) -> None:
        # Reject a float/bool amount BEFORE any to_bytes / arithmetic use:
        # ``int(2.9)`` would silently truncate the on-chain transfer amount,
        # and ``True`` is an ``int`` subclass that would deposit 1 atomic unit.
        if not _is_strict_int(self.amount):
            raise DepositError("deposit amount must be an integer number of atomic USDC units")
        if self.amount <= 0:
            raise DepositError("deposit amount must be greater than zero")
        if self.amount > _U64_MAX:
            raise DepositError("deposit amount exceeds u64 range")
        if len(self.service_id) != 32:
            raise DepositError(f"service_id must be 32 bytes, got {len(self.service_id)}")
        if len(self.recent_blockhash) != 32:
            raise DepositError(
                f"recent_blockhash must be 32 bytes, got {len(self.recent_blockhash)}"
            )

    def __repr__(self) -> str:
        # Never leak the keypair into logs / tracebacks.
        return (
            "DepositParams("
            "agent_keypair_b58=REDACTED, "
            f"provider_wallet_b58={self.provider_wallet_b58!r}, "
            f"usdc_mint_b58={self.usdc_mint_b58!r}, "
            f"escrow_program_id_b58={self.escrow_program_id_b58!r}, "
            f"amount={self.amount}, expiry_slot={self.expiry_slot})"
        )


# ---------------------------------------------------------------------------
# Legacy message serializer (manual — must match the Rust byte layout exactly).
# ---------------------------------------------------------------------------


def _build_legacy_message(
    header: bytes,
    accounts: list[Pubkey],
    program_id: Pubkey,
    recent_blockhash: bytes,
    program_id_index: int,
    ix_account_indices: bytes,
    ix_data: bytes,
) -> bytes:
    """Serialize a Solana legacy transaction message.

    Layout: header(3) || acct_count(u8) || N*32 keys (+ program key) ||
    blockhash(32) || ix_count(u8)=1 || program_id_index(u8) || ix_acct_count(u8)
    || ix_acct_indices || data_len(u8) || data.

    Length prefixes are emitted as a single ``u8`` — the correct compact-u16
    encoding only for lengths <= 127. Larger values are rejected (not
    truncated), mirroring the Rust builder.
    """
    total_accounts = len(accounts) + 1  # +1 for the appended program key
    if total_accounts > _COMPACT_U16_SINGLE_BYTE_MAX:
        raise DepositError(
            f"account count {total_accounts} exceeds the 127-byte compact-u16 single-byte limit"
        )
    if len(ix_account_indices) > _COMPACT_U16_SINGLE_BYTE_MAX:
        raise DepositError(
            f"instruction account count {len(ix_account_indices)} exceeds the "
            "127-byte compact-u16 single-byte limit"
        )
    if len(ix_data) > _COMPACT_U16_SINGLE_BYTE_MAX:
        raise DepositError(
            f"instruction data length {len(ix_data)} exceeds the 127-byte "
            "compact-u16 single-byte limit"
        )

    msg = bytearray()
    msg += header
    msg.append(total_accounts)
    for acc in accounts:
        msg += bytes(acc)
    msg += bytes(program_id)  # program key is the last account
    msg += recent_blockhash
    msg.append(1)  # instruction count
    msg.append(program_id_index)
    msg.append(len(ix_account_indices))
    msg += ix_account_indices
    msg.append(len(ix_data))
    msg += ix_data
    return bytes(msg)


# ---------------------------------------------------------------------------
# Public builder
# ---------------------------------------------------------------------------


def build_deposit_tx(params: DepositParams) -> str:
    """Build a signed escrow ``deposit`` transaction, base64-encoded.

    Returns a base64 (standard alphabet) wire-format Solana legacy transaction
    ready for the gateway / ``sendTransaction``.

    .. warning::
        ``params.agent_keypair_b58`` is secret key material that is **NOT wiped
        from Python heap memory** after this call (unlike the Rust builder's
        ``Zeroizing<String>``). CPython cannot reliably overwrite an immutable
        ``str``. Keep the ``DepositParams`` short-lived and never cache it; see
        the :class:`DepositParams` docstring.

    Raises:
        DepositError: zero amount, malformed keypair/address, bad service_id or
            blockhash length, or message-too-long.
    """
    # Step 1: fail-closed money-path guards on the amount. ``DepositParams``
    # already validates these at construction, but we re-check here (defense in
    # depth) BEFORE any ``.to_bytes`` use so a hand-constructed params object
    # that bypassed __post_init__ can never silently truncate a float amount.
    if not _is_strict_int(params.amount):
        raise DepositError("deposit amount must be an integer number of atomic USDC units")
    if params.amount <= 0:
        raise DepositError("deposit amount must be greater than zero")
    if params.amount > _U64_MAX:
        raise DepositError("deposit amount exceeds u64 range")

    if len(params.service_id) != 32:
        raise DepositError(f"service_id must be 32 bytes, got {len(params.service_id)}")
    if len(params.recent_blockhash) != 32:
        raise DepositError(f"recent_blockhash must be 32 bytes, got {len(params.recent_blockhash)}")

    # Step 2: load the keypair and recover the agent pubkey. The raw solders
    # exception is NOT interpolated and the chain is dropped: a malformed
    # keypair string is (or contains) secret key material, and some solders
    # error variants can echo the offending input. Surface a generic message
    # only — never the raw input or the underlying exception text.
    try:
        keypair = Keypair.from_base58_string(params.agent_keypair_b58)
    except Exception:  # noqa: BLE001 — normalize to typed error, no secret leak
        raise DepositError("invalid agent keypair: bad base58 or wrong length") from None
    agent_pubkey = keypair.pubkey()
    agent_pubkey_bytes = bytes(agent_pubkey)

    # Step 3: parse addresses.
    try:
        provider = Pubkey.from_string(params.provider_wallet_b58)
        usdc_mint = Pubkey.from_string(params.usdc_mint_b58)
        escrow_program = Pubkey.from_string(params.escrow_program_id_b58)
        token_program = Pubkey.from_string(TOKEN_PROGRAM_ID)
        ata_program = Pubkey.from_string(ATA_PROGRAM_ID)
        system_program = Pubkey.from_string(SYSTEM_PROGRAM_ID)
    except Exception as exc:  # noqa: BLE001 — normalize to typed error
        raise DepositError(f"invalid address: {exc}") from exc

    # Step 4: derive escrow PDA and ATAs.
    escrow_pda, _bump = derive_escrow_pda(agent_pubkey_bytes, params.service_id, escrow_program)
    agent_ata = derive_ata_address(agent_pubkey, usdc_mint)
    vault_ata = derive_ata_address(escrow_pda, usdc_mint)

    # Step 5: account list sorted by writability (writable signer, writable
    # non-signers, readonly non-signers). The program key is appended last
    # (index 9) inside the message serializer.
    accounts = [
        agent_pubkey,  # 0: signer, writable
        escrow_pda,  # 1: writable, non-signer
        agent_ata,  # 2: writable, non-signer
        vault_ata,  # 3: writable, non-signer
        provider,  # 4: readonly
        usdc_mint,  # 5: readonly
        token_program,  # 6: readonly
        ata_program,  # 7: readonly
        system_program,  # 8: readonly
    ]

    # Step 6: instruction data = discriminator || amount(LE u64) || service_id
    # || expiry_slot(LE u64) = 8 + 8 + 32 + 8 = 56 bytes.
    ix_data = bytearray()
    ix_data += anchor_discriminator("deposit")
    ix_data += params.amount.to_bytes(8, "little")
    ix_data += params.service_id
    ix_data += params.expiry_slot.to_bytes(8, "little")
    if len(ix_data) != 56:  # invariant guard, mirrors the Rust assert
        raise DepositError(f"deposit ix_data must be 56 bytes, got {len(ix_data)}")

    # Anchor program account order remapped into the writability-sorted layout.
    ix_account_indices = bytes([0, 4, 5, 1, 2, 3, 6, 7, 8])

    # Step 7: legacy message. Header [1, 0, 6]; program_id_index = 9 (last).
    message = _build_legacy_message(
        header=bytes([1, 0, 6]),
        accounts=accounts,
        program_id=escrow_program,
        recent_blockhash=params.recent_blockhash,
        program_id_index=9,
        ix_account_indices=ix_account_indices,
        ix_data=bytes(ix_data),
    )

    # Step 8: sign the raw message bytes with the agent keypair (ed25519).
    signature = keypair.sign_message(message)

    # Step 9: assemble wire tx: compact-u16(1) || signature(64) || message.
    tx_bytes = bytearray()
    tx_bytes.append(0x01)  # compact-u16: 1 signature
    tx_bytes += bytes(signature)
    tx_bytes += message

    return base64.b64encode(bytes(tx_bytes)).decode()
