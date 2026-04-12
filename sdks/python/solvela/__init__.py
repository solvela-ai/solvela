"""RustyClawRouter Python SDK — AI agent payments with USDC on Solana."""

from .client import AsyncLLMClient, BudgetExceededError, LLMClient, PaymentError
from .config import DEFAULT_API_URL
from .types import ChatMessage, ChatResponse, CostBreakdown, Role
from .wallet import Wallet

__version__ = "0.1.0"
__all__ = [
    "LLMClient",
    "AsyncLLMClient",
    "PaymentError",
    "BudgetExceededError",
    "ChatMessage",
    "ChatResponse",
    "CostBreakdown",
    "Role",
    "Wallet",
    "DEFAULT_API_URL",
]
