"""LRU response cache with TTL expiry and dedup window."""

from __future__ import annotations

import hashlib
import threading
import time
from collections import OrderedDict
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from solvela.types import ChatMessage, ChatResponse

_DEFAULT_MAX_ENTRIES = 200
_DEFAULT_TTL = 600.0
_DEFAULT_DEDUP_WINDOW = 30.0


class ResponseCache:
    """Thread-safe LRU cache for chat responses with TTL and dedup window."""

    def __init__(
        self,
        max_entries: int = _DEFAULT_MAX_ENTRIES,
        ttl: float = _DEFAULT_TTL,
        dedup_window: float = _DEFAULT_DEDUP_WINDOW,
    ) -> None:
        self._max_entries = max_entries
        self._ttl = ttl
        self._dedup_window = dedup_window
        self._lock = threading.Lock()
        self._entries: OrderedDict[int, tuple[ChatResponse, float]] = OrderedDict()

    @staticmethod
    def cache_key(model: str, messages: list[ChatMessage]) -> int:
        """Deterministic hash of model + messages. Different models = different keys."""
        h = hashlib.sha256()
        h.update(model.encode())
        for msg in messages:
            h.update(msg.role.value.encode())
            h.update((msg.content or "").encode())
        return int.from_bytes(h.digest()[:8], "big")

    def get(self, key: int) -> ChatResponse | None:
        """Get cached response. Returns None if miss or expired."""
        with self._lock:
            entry = self._entries.get(key)
            if entry is None:
                return None
            resp, inserted = entry
            if time.monotonic() - inserted > self._ttl:
                del self._entries[key]
                return None
            self._entries.move_to_end(key)
            return resp

    def put(self, key: int, response: ChatResponse) -> None:
        """Cache a response. Respects dedup window and max entries."""
        with self._lock:
            if key in self._entries:
                _, inserted = self._entries[key]
                if time.monotonic() - inserted < self._dedup_window:
                    return  # don't overwrite within dedup window
            self._entries[key] = (response, time.monotonic())
            self._entries.move_to_end(key)
            while len(self._entries) > self._max_entries:
                self._entries.popitem(last=False)  # evict LRU
