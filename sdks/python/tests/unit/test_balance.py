"""Unit tests for BalanceMonitor — polling, callbacks, stop idempotency."""

from __future__ import annotations

import asyncio
import logging

import pytest

from solvela.balance import BalanceMonitor
from solvela.client import SolvelaClient
from solvela.errors import ClientError


@pytest.mark.asyncio
async def test_balance_monitor_polls() -> None:
    """Verify fetch is called multiple times."""
    call_count = 0

    async def fetch() -> float:
        nonlocal call_count
        call_count += 1
        return 100.0

    monitor = BalanceMonitor(fetch_balance=fetch, poll_interval=0.01)
    monitor.start()
    await asyncio.sleep(0.05)
    monitor.stop()
    assert call_count >= 2


@pytest.mark.asyncio
async def test_balance_monitor_updates_state() -> None:
    """last_known_balance() reflects the polled value."""

    async def fetch() -> float:
        return 42.5

    monitor = BalanceMonitor(fetch_balance=fetch, poll_interval=0.01)
    assert monitor.last_known_balance() is None
    monitor.start()
    await asyncio.sleep(0.05)
    monitor.stop()
    assert monitor.last_known_balance() == 42.5


@pytest.mark.asyncio
async def test_low_balance_callback_fires_on_transition() -> None:
    """Callback fires once when crossing threshold, not every tick."""
    fired: list[float] = []
    call_count = 0

    async def fetch() -> float:
        nonlocal call_count
        call_count += 1
        return 5.0  # always below threshold

    def on_low(balance: float) -> None:
        fired.append(balance)

    monitor = BalanceMonitor(
        fetch_balance=fetch,
        poll_interval=0.01,
        low_balance_threshold=10.0,
        on_low_balance=on_low,
    )
    monitor.start()
    await asyncio.sleep(0.05)
    monitor.stop()
    # Should fire exactly once (transition from not-low to low)
    assert len(fired) == 1
    assert fired[0] == 5.0
    # But polling happened multiple times
    assert call_count >= 2


@pytest.mark.asyncio
async def test_stop_is_idempotent() -> None:
    """Calling stop() twice doesn't raise."""

    async def fetch() -> float:
        return 100.0

    monitor = BalanceMonitor(fetch_balance=fetch, poll_interval=0.01)
    monitor.start()
    await asyncio.sleep(0.02)
    monitor.stop()
    monitor.stop()  # second call should not raise


@pytest.mark.asyncio
async def test_poll_error_emits_warning_and_recovers(
    caplog: pytest.LogCaptureFixture,
) -> None:
    """A failing poll must log at WARNING level and the loop must continue.

    Mirrors the warn-on-poll-error contract in the TS SDK. The loop keeps
    polling on the next tick; the failed poll itself clears
    ``last_known_balance()`` to ``None`` (an explicit "unknown" signal — see
    ``test_client_error_resets_state_to_none`` for that contract).
    """
    call_count = 0

    async def fetch() -> float:
        nonlocal call_count
        call_count += 1
        if call_count == 1:
            raise RuntimeError("network blip")
        return 42.0

    caplog.set_level(logging.WARNING, logger="solvela.balance")
    monitor = BalanceMonitor(fetch_balance=fetch, poll_interval=0.01)
    monitor.start()
    await asyncio.sleep(0.05)
    monitor.stop()

    assert monitor.last_known_balance() == 42.0  # recovered after the failed poll
    warnings = [r for r in caplog.records if r.levelno == logging.WARNING]
    assert any("Balance fetch failed" in r.getMessage() for r in warnings), (
        f"expected a WARNING containing 'Balance fetch failed'; got {warnings!r}"
    )


@pytest.mark.asyncio
async def test_client_error_resets_state_to_none(
    caplog: pytest.LogCaptureFixture,
) -> None:
    """ClientError on fetch must mark the balance unknown, not keep stale 0.0.

    Without this, BalanceMonitor's `_balance` stays at the previous successful
    value, and any caller using `last_known_balance()` to gate behavior sees a
    stale balance for the entire RPC outage window.
    """
    call_count = 0

    async def fetch() -> float:
        nonlocal call_count
        call_count += 1
        if call_count == 1:
            return 0.0  # legitimate zero balance recorded first
        raise ClientError("RPC outage")

    caplog.set_level(logging.WARNING, logger="solvela.balance")
    monitor = BalanceMonitor(fetch_balance=fetch, poll_interval=0.01)
    monitor.start()
    await asyncio.sleep(0.05)
    monitor.stop()

    # First poll set it to 0.0; subsequent ClientError must clear it to None.
    assert monitor.last_known_balance() is None
    assert any("RPC error" in r.getMessage() for r in caplog.records)


@pytest.mark.asyncio
async def test_on_balance_change_fires_on_every_transition() -> None:
    """Callback fires for every state mutation: success, ClientError, generic.

    Pre-this-PR, BalanceMonitor stored its own `_balance` but never told the
    SolvelaClient about transitions. Wiring `on_balance_change` to the client's
    setter is the supported path for keeping the chat balance guard in sync.
    """
    transitions: list[float | None] = []
    call_count = 0

    async def fetch() -> float:
        nonlocal call_count
        call_count += 1
        if call_count == 1:
            return 5.0
        if call_count == 2:
            raise ClientError("RPC outage")
        # call_count >= 3
        return 7.5

    monitor = BalanceMonitor(
        fetch_balance=fetch,
        poll_interval=0.01,
        on_balance_change=transitions.append,
    )
    monitor.start()
    await asyncio.sleep(0.06)
    monitor.stop()

    # Should have at least: 5.0 (first poll) -> None (ClientError) -> 7.5 (recover)
    assert 5.0 in transitions
    assert None in transitions
    assert 7.5 in transitions
    # Order matters: 5.0 must come before None must come before 7.5
    i_five = transitions.index(5.0)
    i_none = transitions.index(None)
    i_seven = transitions.index(7.5)
    assert i_five < i_none < i_seven


@pytest.mark.asyncio
async def test_on_balance_change_callback_failure_does_not_kill_loop(
    caplog: pytest.LogCaptureFixture,
) -> None:
    """A buggy listener must not crash the polling loop."""
    call_count = 0

    async def fetch() -> float:
        nonlocal call_count
        call_count += 1
        return float(call_count)

    def bad_listener(_: float | None) -> None:
        raise RuntimeError("listener bug")

    caplog.set_level(logging.WARNING, logger="solvela.balance")
    monitor = BalanceMonitor(
        fetch_balance=fetch,
        poll_interval=0.01,
        on_balance_change=bad_listener,
    )
    monitor.start()
    await asyncio.sleep(0.05)
    monitor.stop()

    assert call_count >= 2  # loop kept running despite listener raising
    assert any("on_balance_change" in r.getMessage() for r in caplog.records)


@pytest.mark.asyncio
async def test_balance_state_setter_accepts_none() -> None:
    """`SolvelaClient.balance_state_setter()` round-trips both float and None.

    Wiring BalanceMonitor's on_balance_change to this setter must clear the
    chat guard's `_last_balance` cache when the monitor reports unknown.
    """
    client = SolvelaClient()
    setter = client.balance_state_setter()

    setter(50.0)
    assert client.last_known_balance() == 50.0

    setter(None)
    assert client.last_known_balance() is None


@pytest.mark.asyncio
async def test_full_wiring_unlocks_free_fallback_after_outage() -> None:
    """End-to-end: monitor → setter → client._last_balance.

    Pin the failure mode: a successful 0.0 poll arms the free-fallback guard;
    a subsequent ClientError must clear `_last_balance` to None so the guard
    no longer routes new chats to free-fallback.
    """
    call_count = 0

    async def fetch() -> float:
        nonlocal call_count
        call_count += 1
        if call_count == 1:
            return 0.0
        raise ClientError("RPC outage")

    client = SolvelaClient()
    monitor = BalanceMonitor(
        fetch_balance=fetch,
        poll_interval=0.01,
        on_balance_change=client.balance_state_setter(),
    )
    monitor.start()
    await asyncio.sleep(0.05)
    monitor.stop()

    # The 0.0 from the first poll has already been replaced by None on the
    # second poll's ClientError — chat guard sees `_last_balance is None`
    # and falls through, no free-fallback swap.
    assert client.last_known_balance() is None


@pytest.mark.asyncio
async def test_callback_does_not_fire_when_above_threshold() -> None:
    """No callback when balance stays above threshold."""
    fired: list[float] = []

    async def fetch() -> float:
        return 100.0

    def on_low(balance: float) -> None:
        fired.append(balance)

    monitor = BalanceMonitor(
        fetch_balance=fetch,
        poll_interval=0.01,
        low_balance_threshold=10.0,
        on_low_balance=on_low,
    )
    monitor.start()
    await asyncio.sleep(0.05)
    monitor.stop()
    assert len(fired) == 0
