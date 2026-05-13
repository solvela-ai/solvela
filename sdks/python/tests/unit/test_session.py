"""Tests for SessionStore — session tracking with three-strike escalation."""

from __future__ import annotations

import time

from solvela.session import SessionStore
from solvela.types import ChatMessage, Role


def _msgs(*contents: str) -> list[ChatMessage]:
    return [ChatMessage(role=Role.USER, content=c) for c in contents]


class TestNewSession:
    def test_new_session_returns_default_model(self) -> None:
        store = SessionStore()
        info = store.get_or_create("sess-1", "gpt-4")
        assert info.model == "gpt-4"
        assert info.escalated is False


class TestExistingSession:
    def test_existing_session_returns_stored_model(self) -> None:
        store = SessionStore()
        store.get_or_create("sess-1", "gpt-4")
        # Second call with different default still returns the original model
        info = store.get_or_create("sess-1", "claude-3")
        assert info.model == "gpt-4"


class TestExpiredSession:
    def test_expired_session_creates_new(self) -> None:
        store = SessionStore(ttl=0.05)
        store.get_or_create("sess-1", "gpt-4")
        time.sleep(0.06)
        info = store.get_or_create("sess-1", "claude-3")
        assert info.model == "claude-3"
        assert info.escalated is False


class TestThreeStrike:
    def test_three_strike_sets_escalated(self) -> None:
        store = SessionStore()
        store.get_or_create("sess-1", "gpt-4")
        for _ in range(3):
            store.record_request("sess-1", 42)
        info = store.get_or_create("sess-1", "gpt-4")
        assert info.escalated is True

    def test_less_than_three_does_not_escalate(self) -> None:
        store = SessionStore()
        store.get_or_create("sess-1", "gpt-4")
        store.record_request("sess-1", 42)
        store.record_request("sess-1", 42)
        info = store.get_or_create("sess-1", "gpt-4")
        assert info.escalated is False


class TestDeriveSessionId:
    def test_derive_session_id_deterministic(self) -> None:
        msgs = _msgs("Hello, world!")
        id1 = SessionStore.derive_session_id(msgs)
        id2 = SessionStore.derive_session_id(msgs)
        assert id1 == id2

    def test_derive_session_id_differs_for_different_content(self) -> None:
        id1 = SessionStore.derive_session_id(_msgs("Hello"))
        id2 = SessionStore.derive_session_id(_msgs("Goodbye"))
        assert id1 != id2

    def test_same_first_message_different_continuation_distinct_ids(self) -> None:
        """Two conversations sharing only an opening message must hash distinctly.

        The previous implementation hashed messages[0].content alone, so unrelated
        agents both starting with "Hello" collided into the same session and bled
        escalation/request-hash state across each other.
        """
        id1 = SessionStore.derive_session_id(_msgs("Hello", "How's the weather?"))
        id2 = SessionStore.derive_session_id(_msgs("Hello", "What's the time?"))
        assert id1 != id2

    def test_role_changes_session_id(self) -> None:
        """Same content under a different role must hash distinctly."""
        from solvela.types import ChatMessage, Role

        as_user = [ChatMessage(role=Role.USER, content="Hello")]
        as_assistant = [ChatMessage(role=Role.ASSISTANT, content="Hello")]
        assert SessionStore.derive_session_id(as_user) != SessionStore.derive_session_id(
            as_assistant
        )

    def test_empty_messages_is_stable(self) -> None:
        """Empty input must still produce a deterministic ID without raising."""
        assert SessionStore.derive_session_id([]) == SessionStore.derive_session_id([])


class TestCleanupExpired:
    def test_cleanup_expired(self) -> None:
        store = SessionStore(ttl=0.05)
        # Create 3 sessions
        store.get_or_create("sess-1", "gpt-4")
        store.get_or_create("sess-2", "gpt-4")
        time.sleep(0.06)
        # Create a third that's still fresh
        store.get_or_create("sess-3", "claude-3")
        store.cleanup_expired()
        # sess-1 and sess-2 expired, sess-3 survives
        info1 = store.get_or_create("sess-1", "new-model")
        assert info1.model == "new-model"  # recreated
        info3 = store.get_or_create("sess-3", "new-model")
        assert info3.model == "claude-3"  # survived cleanup
