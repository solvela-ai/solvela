"""OpenAI-compatible wrapper for SolvelaClient."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from solvela.types import ChatMessage, ChatRequest, Role

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

    from solvela.types import ChatChunk, ChatResponse


class OpenAICompat:
    """OpenAI-compatible wrapper for SolvelaClient.

    Usage:
        openai = OpenAICompat(client)
        resp = await openai.chat.completions.create(model="gpt-4o", messages=[...])
    """

    def __init__(self, client: Any) -> None:
        self.chat = _ChatNamespace(client)


class _ChatNamespace:
    def __init__(self, client: Any) -> None:
        self.completions = _CompletionsNamespace(client)


class _CompletionsNamespace:
    def __init__(self, client: Any) -> None:
        self._client = client

    async def create(
        self,
        model: str,
        messages: list[dict[str, str]],
        stream: bool = False,
        **kwargs: Any,
    ) -> ChatResponse | AsyncIterator[ChatChunk]:
        """Create a chat completion (OpenAI-compatible interface)."""
        parsed = [
            ChatMessage(
                role=Role(m["role"]),
                content=m.get("content", ""),
                name=m.get("name"),
            )
            for m in messages
        ]
        request = ChatRequest(model=model, messages=parsed, **kwargs)
        if stream:
            return self._client.chat_stream(request)
        return await self._client.chat(request)
