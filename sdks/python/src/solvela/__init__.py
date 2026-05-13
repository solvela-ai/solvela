"""Solvela Python SDK — Solana-native AI agent payment client."""

from solvela.balance import BalanceMonitor
from solvela.client import SolvelaClient
from solvela.config import ClientBuilder, ClientConfig
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
from solvela.signer import KeypairSigner, Signer
from solvela.types import (
    AtomicUsdc,
    ChatChunk,
    ChatMessage,
    ChatRequest,
    ChatResponse,
    FinishReason,
    ModelInfo,
    PaymentAccept,
    PaymentPayload,
    PaymentRequired,
    Role,
    Scheme,
    ToolType,
)
from solvela.wallet import Wallet

__all__ = [
    "BalanceMonitor",
    "SolvelaClient",
    "ClientBuilder",
    "ClientConfig",
    "AmountExceedsMaxError",
    "ClientError",
    "GatewayError",
    "InsufficientBalanceError",
    "PaymentRejectedError",
    "PaymentRequiredError",
    "RecipientMismatchError",
    "Scheme",
    "SignerError",
    "TimeoutError",
    "WalletError",
    "KeypairSigner",
    "Signer",
    "Wallet",
    # Wire types — exposed so callers can annotate their own code without
    # reaching into ``solvela.types``.
    "AtomicUsdc",
    "ChatChunk",
    "ChatMessage",
    "ChatRequest",
    "ChatResponse",
    "FinishReason",
    "ModelInfo",
    "PaymentAccept",
    "PaymentPayload",
    "PaymentRequired",
    "Role",
    "ToolType",
]
