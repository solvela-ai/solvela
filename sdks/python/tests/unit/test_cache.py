"""Tests for ResponseCache — LRU with TTL and dedup window."""

from __future__ import annotations

import time

from solvela.cache import ResponseCache
from solvela.types import ChatChoice, ChatMessage, ChatResponse, Role, Usage


def _make_response(content: str = "Hello") -> ChatResponse:
    return ChatResponse(
        id="chatcmpl-test",
        object="chat.completion",
        created=1700000000,
        model="gpt-4",
        choices=[
            ChatChoice(
                index=0,
                message=ChatMessage(role=Role.ASSISTANT, content=content),
                finish_reason="stop",
            )
        ],
        usage=Usage(prompt_tokens=10, completion_tokens=5, total_tokens=15),
    )


def _msgs(*contents: str) -> list[ChatMessage]:
    return [ChatMessage(role=Role.USER, content=c) for c in contents]


class TestCacheMiss:
    def test_cache_miss(self) -> None:
        cache = ResponseCache()
        assert cache.get(12345) is None


class TestCacheHit:
    def test_cache_hit(self) -> None:
        cache = ResponseCache()
        resp = _make_response()
        key = ResponseCache.cache_key("gpt-4", _msgs("Hello"))
        cache.put(key, resp)
        assert cache.get(key) is resp


class TestTTLExpiry:
    def test_ttl_expiry(self) -> None:
        cache = ResponseCache(ttl=0.05)
        resp = _make_response()
        key = ResponseCache.cache_key("gpt-4", _msgs("Hello"))
        cache.put(key, resp)
        assert cache.get(key) is resp
        time.sleep(0.06)
        assert cache.get(key) is None


class TestLRUEviction:
    def test_lru_eviction(self) -> None:
        cache = ResponseCache(max_entries=3)
        keys = []
        for i in range(4):
            key = ResponseCache.cache_key("gpt-4", _msgs(f"msg-{i}"))
            keys.append(key)
            cache.put(key, _make_response(f"resp-{i}"))

        # Oldest (keys[0]) should be evicted
        assert cache.get(keys[0]) is None
        # Others should still be present
        assert cache.get(keys[1]) is not None
        assert cache.get(keys[2]) is not None
        assert cache.get(keys[3]) is not None


class TestDedupWindow:
    def test_dedup_window(self) -> None:
        cache = ResponseCache(dedup_window=1.0)
        key = ResponseCache.cache_key("gpt-4", _msgs("Hello"))
        first = _make_response("first")
        second = _make_response("second")
        cache.put(key, first)
        cache.put(key, second)
        result = cache.get(key)
        assert result is not None
        assert result.choices[0].message.content == "first"

    def test_dedup_window_expired(self) -> None:
        cache = ResponseCache(dedup_window=0.05)
        key = ResponseCache.cache_key("gpt-4", _msgs("Hello"))
        first = _make_response("first")
        second = _make_response("second")
        cache.put(key, first)
        time.sleep(0.06)
        cache.put(key, second)
        result = cache.get(key)
        assert result is not None
        assert result.choices[0].message.content == "second"


class TestCacheKey:
    def test_cache_key_deterministic(self) -> None:
        msgs = _msgs("Hello", "World")
        k1 = ResponseCache.cache_key("gpt-4", msgs)
        k2 = ResponseCache.cache_key("gpt-4", msgs)
        assert k1 == k2

    def test_cache_key_differs_for_different_models(self) -> None:
        msgs = _msgs("Hello")
        k1 = ResponseCache.cache_key("gpt-4", msgs)
        k2 = ResponseCache.cache_key("claude-3", msgs)
        assert k1 != k2
