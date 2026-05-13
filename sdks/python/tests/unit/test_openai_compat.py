"""Unit tests for OpenAI compatibility wrapper."""

from __future__ import annotations

from unittest.mock import AsyncMock, MagicMock

import pytest

from solvela.openai_compat import OpenAICompat
from solvela.types import ChatMessage, ChatRequest, ChatResponse, Role


@pytest.mark.asyncio
async def test_openai_compat_create() -> None:
    """Mock client, verify chat called with ChatRequest."""
    mock_response = MagicMock(spec=ChatResponse)
    mock_client = MagicMock()
    mock_client.chat = AsyncMock(return_value=mock_response)

    openai = OpenAICompat(mock_client)
    result = await openai.chat.completions.create(
        model="gpt-4o",
        messages=[{"role": "user", "content": "Hello"}],
    )

    assert result is mock_response
    mock_client.chat.assert_called_once()
    call_args = mock_client.chat.call_args[0][0]
    assert isinstance(call_args, ChatRequest)
    assert call_args.model == "gpt-4o"
    assert len(call_args.messages) == 1
    assert call_args.messages[0].role == Role.USER


@pytest.mark.asyncio
async def test_openai_compat_accepts_dict_messages() -> None:
    """Verify dicts are converted to ChatMessage objects."""
    mock_client = MagicMock()
    mock_client.chat = AsyncMock(return_value=MagicMock(spec=ChatResponse))

    openai = OpenAICompat(mock_client)
    await openai.chat.completions.create(
        model="gpt-4o",
        messages=[
            {"role": "system", "content": "You are helpful."},
            {"role": "user", "content": "Hi"},
        ],
    )

    call_args = mock_client.chat.call_args[0][0]
    assert len(call_args.messages) == 2
    assert all(isinstance(m, ChatMessage) for m in call_args.messages)
    assert call_args.messages[0].role == Role.SYSTEM
    assert call_args.messages[1].content == "Hi"


@pytest.mark.asyncio
async def test_openai_compat_stream() -> None:
    """Verify stream=True calls chat_stream."""
    mock_client = MagicMock()
    sentinel = object()
    mock_client.chat_stream = MagicMock(return_value=sentinel)

    openai = OpenAICompat(mock_client)
    result = await openai.chat.completions.create(
        model="gpt-4o",
        messages=[{"role": "user", "content": "Hello"}],
        stream=True,
    )

    assert result is sentinel
    mock_client.chat_stream.assert_called_once()
