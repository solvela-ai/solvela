"""Live contract tests — require a running Solvela gateway."""

from __future__ import annotations

import pytest

from solvela.errors import PaymentRequiredError
from solvela.types import ChatMessage, ChatRequest, Role


@pytest.mark.asyncio
async def test_live_chat_402_without_wallet(live_client):
    """Without a wallet/signer, a chat request should return 402."""
    req = ChatRequest(
        model="gpt-4o",
        messages=[ChatMessage(role=Role.USER, content="Hello")],
    )
    with pytest.raises(PaymentRequiredError):
        await live_client.chat(req)


@pytest.mark.asyncio
async def test_live_models_list(live_client):
    """The gateway should return at least one model."""
    models = await live_client.models()
    assert len(models) > 0
