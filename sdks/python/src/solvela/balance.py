"""Solvela balance monitor — background USDC-SPL balance poller."""

from __future__ import annotations

import asyncio
import logging
from typing import TYPE_CHECKING

from solvela.errors import ClientError

if TYPE_CHECKING:
    from collections.abc import Awaitable, Callable

logger = logging.getLogger(__name__)


class BalanceMonitor:
    """Polls USDC-SPL balance on a schedule, fires callbacks on transitions.

    ``on_balance_change`` fires after every balance update, including the
    transition to ``None`` when an RPC fetch fails. Pair with
    ``SolvelaClient.balance_state_setter()`` to keep the chat balance guard in
    sync — without that wiring, a stale ``0.0`` cached in
    ``SolvelaClient._last_balance`` would lock the caller into the
    free-fallback model for the entire RPC outage window even though this
    monitor's ``last_known_balance()`` correctly reports unknown.
    """

    def __init__(
        self,
        fetch_balance: Callable[[], Awaitable[float]],
        poll_interval: float = 30.0,
        low_balance_threshold: float | None = None,
        on_low_balance: Callable[[float], None] | None = None,
        on_balance_change: Callable[[float | None], None] | None = None,
    ) -> None:
        self._fetch_balance = fetch_balance
        self._poll_interval = poll_interval
        self._threshold = low_balance_threshold
        self._on_low_balance = on_low_balance
        self._on_balance_change = on_balance_change
        self._balance: float | None = None
        self._was_low = False
        self._task: asyncio.Task[None] | None = None
        self._stopped = False

    def start(self) -> None:
        """Start the polling loop as a background task."""
        self._stopped = False
        self._task = asyncio.create_task(self._run())

    def stop(self) -> None:
        """Stop the polling loop. Idempotent."""
        self._stopped = True
        if self._task is not None and not self._task.done():
            self._task.cancel()
            self._task = None

    def last_known_balance(self) -> float | None:
        """Get the last polled balance, or None if never polled."""
        return self._balance

    def _set_balance(self, value: float | None) -> None:
        """Update internal balance and notify the on_balance_change subscriber.

        Centralized so every mutation site — success, ClientError, generic
        Exception — fires the callback with the current state. Callbacks are
        guarded so a buggy listener can't kill the polling loop.
        """
        self._balance = value
        if self._on_balance_change is not None:
            try:
                self._on_balance_change(value)
            except Exception:
                logger.warning(
                    "on_balance_change callback raised; continuing poll loop",
                    exc_info=True,
                )

    async def _run(self) -> None:
        while not self._stopped:
            try:
                balance = await self._fetch_balance()
                self._set_balance(balance)

                # Transition-debounced low balance callback
                if self._threshold is not None and self._on_low_balance is not None:
                    is_low = balance < self._threshold
                    if is_low and not self._was_low:
                        self._on_low_balance(balance)
                    self._was_low = is_low
            except asyncio.CancelledError:
                break
            except ClientError:
                # The new C3 fix in client._query_balance distinguishes ATA-absent
                # ("real zero") from RPC failure (raises ClientError). On failure
                # we must NOT keep the previous balance — a stale 0.0 would lock
                # callers into the free-fallback model for the entire RPC outage
                # window. None signals "unknown", letting the balance guard skip.
                self._set_balance(None)
                logger.warning("Balance fetch failed (RPC error)", exc_info=True)
            except Exception:
                # Surface poll errors at WARNING level. A silent failure (the
                # previous DEBUG default) would leave last_known_balance() stuck
                # at None and silently disable any free-fallback-model guard
                # downstream. Reset _balance to None so a stale value doesn't
                # lock callers into the free-fallback model for the entire
                # outage window — None signals "unknown" to the balance guard.
                self._set_balance(None)
                logger.warning("Balance fetch failed", exc_info=True)

            try:
                await asyncio.sleep(self._poll_interval)
            except asyncio.CancelledError:
                break
