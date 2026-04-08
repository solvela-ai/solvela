"""Pydantic v2 models for the RustyClawRouter SDK."""

from enum import Enum
from typing import List, Optional

from pydantic import BaseModel


class Role(str, Enum):
    SYSTEM = "system"
    USER = "user"
    ASSISTANT = "assistant"
    TOOL = "tool"


class ChatMessage(BaseModel):
    role: Role
    content: str
    name: Optional[str] = None


class ChatRequest(BaseModel):
    model: str
    messages: List[ChatMessage]
    max_tokens: Optional[int] = None
    temperature: Optional[float] = None
    top_p: Optional[float] = None
    stream: bool = False


class Usage(BaseModel):
    prompt_tokens: int
    completion_tokens: int
    total_tokens: int


class ChatChoice(BaseModel):
    index: int
    message: ChatMessage
    finish_reason: Optional[str] = None


class ChatResponse(BaseModel):
    id: str
    object: str
    created: int
    model: str
    choices: List[ChatChoice]
    usage: Optional[Usage] = None


class CostBreakdown(BaseModel):
    provider_cost: str
    platform_fee: str
    total: str
    currency: str
    fee_percent: int


class PaymentAccept(BaseModel):
    scheme: str
    network: str
    amount: str
    asset: str
    pay_to: str
    max_timeout_seconds: int
    escrow_program_id: Optional[str] = None


class PaymentRequired(BaseModel):
    x402_version: int
    accepts: List[PaymentAccept]
    cost_breakdown: CostBreakdown
    error: str


class SpendInfo(BaseModel):
    total_requests: int = 0
    total_cost_usdc: float = 0.0
    daily_cost_usdc: float = 0.0
