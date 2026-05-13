"""Session tracking with three-strike escalation."""

from __future__ import annotations

import hashlib
import threading
import time
from collections import deque
from dataclasses import dataclass
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from solvela.types import ChatMessage

_MAX_RECENT_HASHES = 10
_THREE_STRIKE_THRESHOLD = 3


@dataclass
class SessionInfo:
    """Public session state returned to callers."""

    model: str
    escalated: bool


class SessionStore:
    """Thread-safe session store with TTL expiry and three-strike escalation."""

    def __init__(self, ttl: float = 1800.0) -> None:
        self._ttl = ttl
        self._lock = threading.Lock()
        self._sessions: dict[str, _SessionEntry] = {}

    def get_or_create(self, session_id: str, default_model: str) -> SessionInfo:
        """Get existing session or create new one with default model."""
        with self._lock:
            entry = self._sessions.get(session_id)
            if entry is not None and (time.monotonic() - entry.created) < self._ttl:
                return SessionInfo(model=entry.model, escalated=entry.escalated)
            self._sessions[session_id] = _SessionEntry(
                model=default_model, created=time.monotonic()
            )
            return SessionInfo(model=default_model, escalated=False)

    def record_request(self, session_id: str, request_hash: int) -> None:
        """Record a request hash. If same hash appears 3+ times in recent window, escalate."""
        with self._lock:
            entry = self._sessions.get(session_id)
            if entry is None:
                return
            entry.request_count += 1
            if len(entry.recent_hashes) >= _MAX_RECENT_HASHES:
                entry.recent_hashes.popleft()
            entry.recent_hashes.append(request_hash)
            if not entry.escalated:
                counts: dict[int, int] = {}
                for h in entry.recent_hashes:
                    counts[h] = counts.get(h, 0) + 1
                    if counts[h] >= _THREE_STRIKE_THRESHOLD:
                        entry.escalated = True
                        break

    def cleanup_expired(self) -> None:
        """Remove all expired sessions."""
        with self._lock:
            now = time.monotonic()
            expired = [k for k, v in self._sessions.items() if now - v.created >= self._ttl]
            for k in expired:
                del self._sessions[k]

    @staticmethod
    def derive_session_id(messages: list[ChatMessage]) -> str:
        """Derive a deterministic session ID from the full message sequence.

        Hashes ``(role, content)`` for every message with explicit field
        separators, so two conversations that happen to share an opening
        message (e.g. "Hello") but diverge afterwards land in distinct
        sessions. This prevents three-strike escalation and request-hash
        tracking from bleeding across unrelated conversations.
        """
        h = hashlib.sha256()
        for msg in messages:
            h.update(msg.role.encode())
            h.update(b"\x00")
            h.update((msg.content or "").encode())
            h.update(b"\x01")
        return h.hexdigest()[:16]


class _SessionEntry:
    """Internal mutable session state."""

    def __init__(self, model: str, created: float) -> None:
        self.model = model
        self.created = created
        self.request_count = 0
        self.recent_hashes: deque[int] = deque()
        self.escalated = False
