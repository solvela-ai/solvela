"""Anthropic SDK wrapper with cost accounting and dry-run support.

The wrapper exists so the rest of the agentic layer can stay model-agnostic
(swap in a mock for tests, swap in Vercel AI Gateway later if desired) and
so cost discipline is in one place. Callers should:

  client = make_client(model="claude-sonnet-4-6", dry_run=False)
  result = client.invoke(system="...", user="...")
  client.total_cost_usd  # cumulative across all invoke() calls

If `dry_run=True`, no API calls are made — invoke() returns a stub response
with empty content and accumulates *estimated* token costs based on length.
That estimate is intentionally pessimistic (over-counts) so a dry-run can't
falsely promise that a real run will fit in the budget.
"""

from __future__ import annotations

import json
import os
import re
import sys
from dataclasses import dataclass
from typing import Protocol


# Pricing per million tokens (input, output). Update if Anthropic changes
# them. Sourced 2026-05-14.
_PRICING: dict[str, tuple[float, float]] = {
    "claude-opus-4-7": (15.0, 75.0),
    "claude-sonnet-4-6": (3.0, 15.0),
    "claude-haiku-4-5": (1.0, 5.0),
}


@dataclass(frozen=True)
class InvokeResult:
    text: str
    input_tokens: int
    output_tokens: int
    cost_usd: float
    stopped_for: str


class _ClientProtocol(Protocol):
    total_cost_usd: float
    total_input_tokens: int
    total_output_tokens: int

    def invoke(self, *, system: str, user: str, max_tokens: int = 2048) -> InvokeResult:
        ...


@dataclass
class CostTracker:
    """Cumulative token + dollar accounting, shared across all client calls
    in one run so the orchestrator can enforce a single budget."""

    max_cost_usd: float
    total_input_tokens: int = 0
    total_output_tokens: int = 0
    total_cost_usd: float = 0.0

    def add(self, input_tokens: int, output_tokens: int, cost_usd: float) -> None:
        self.total_input_tokens += input_tokens
        self.total_output_tokens += output_tokens
        self.total_cost_usd += cost_usd

    def remaining(self) -> float:
        return max(0.0, self.max_cost_usd - self.total_cost_usd)

    def exhausted(self) -> bool:
        return self.total_cost_usd >= self.max_cost_usd


def _pricing_for(model: str) -> tuple[float, float]:
    if model in _PRICING:
        return _PRICING[model]
    # Unknown model → use the most expensive tier so budget can't under-count.
    return _PRICING["claude-opus-4-7"]


def _estimate_cost(model: str, input_tokens: int, output_tokens: int) -> float:
    in_rate, out_rate = _pricing_for(model)
    return (input_tokens * in_rate + output_tokens * out_rate) / 1_000_000


# Pessimistic token-count estimate when we can't tokenize: ~1 token per 3 chars.
# Real models use ~4 chars/token; over-counting by ~25% means dry-runs
# discover budget overruns *before* the real run, not after.
def _estimate_tokens(text: str) -> int:
    return max(1, len(text) // 3)


class RealAnthropicClient:
    """Thin wrapper around the official `anthropic` SDK.

    Lazy-imports the SDK so the rest of audit-docs has no hard dep on it.
    """

    def __init__(
        self,
        model: str,
        tracker: CostTracker,
        api_key: str | None = None,
    ) -> None:
        try:
            import anthropic  # type: ignore[import-not-found]
        except ImportError as e:
            raise RuntimeError(
                "the agentic check needs the `anthropic` SDK. "
                "install with: pip install -r scripts/audit-docs/requirements.txt"
            ) from e

        key = api_key or os.environ.get("ANTHROPIC_API_KEY")
        if not key:
            raise RuntimeError(
                "ANTHROPIC_API_KEY is required for agentic checks. "
                "set it in your env, or use --dry-run to preview without spending."
            )
        self._sdk = anthropic.Anthropic(api_key=key)
        self._model = model
        self._tracker = tracker

    @property
    def total_cost_usd(self) -> float:
        return self._tracker.total_cost_usd

    @property
    def total_input_tokens(self) -> int:
        return self._tracker.total_input_tokens

    @property
    def total_output_tokens(self) -> int:
        return self._tracker.total_output_tokens

    def invoke(self, *, system: str, user: str, max_tokens: int = 2048) -> InvokeResult:
        if self._tracker.exhausted():
            return InvokeResult(
                text="",
                input_tokens=0,
                output_tokens=0,
                cost_usd=0.0,
                stopped_for="budget",
            )
        try:
            resp = self._sdk.messages.create(
                model=self._model,
                max_tokens=max_tokens,
                system=system,
                messages=[{"role": "user", "content": user}],
            )
        except Exception as e:
            return InvokeResult(
                text="",
                input_tokens=0,
                output_tokens=0,
                cost_usd=0.0,
                stopped_for=f"error:{type(e).__name__}: {e}",
            )

        text = _extract_text(resp)
        in_toks = getattr(resp.usage, "input_tokens", 0) if hasattr(resp, "usage") else 0
        out_toks = getattr(resp.usage, "output_tokens", 0) if hasattr(resp, "usage") else 0
        cost = _estimate_cost(self._model, in_toks, out_toks)
        self._tracker.add(in_toks, out_toks, cost)

        stop = getattr(resp, "stop_reason", "end_turn") or "end_turn"
        return InvokeResult(
            text=text,
            input_tokens=in_toks,
            output_tokens=out_toks,
            cost_usd=cost,
            stopped_for=stop,
        )


def _extract_text(resp) -> str:
    parts: list[str] = []
    for block in getattr(resp, "content", []) or []:
        if getattr(block, "type", None) == "text":
            t = getattr(block, "text", "")
            if t:
                parts.append(t)
    return "\n".join(parts)


class DryRunClient:
    """Estimates tokens + cost without calling the API. Returns empty text."""

    def __init__(self, model: str, tracker: CostTracker, log_stream=None) -> None:
        self._model = model
        self._tracker = tracker
        self._log = log_stream or sys.stderr

    @property
    def total_cost_usd(self) -> float:
        return self._tracker.total_cost_usd

    @property
    def total_input_tokens(self) -> int:
        return self._tracker.total_input_tokens

    @property
    def total_output_tokens(self) -> int:
        return self._tracker.total_output_tokens

    def invoke(self, *, system: str, user: str, max_tokens: int = 2048) -> InvokeResult:
        in_toks = _estimate_tokens(system + "\n" + user)
        out_toks = int(max_tokens * 0.3)
        cost = _estimate_cost(self._model, in_toks, out_toks)
        self._tracker.add(in_toks, out_toks, cost)

        first_user_line = user.splitlines()[0][:120] if user else ""
        print(
            f"[dry-run] model={self._model} in≈{in_toks}tk out≈{out_toks}tk "
            f"cost≈${cost:.4f} :: {first_user_line}",
            file=self._log,
        )

        return InvokeResult(
            text="",
            input_tokens=in_toks,
            output_tokens=out_toks,
            cost_usd=cost,
            stopped_for="dry-run",
        )


def make_client(
    model: str,
    max_cost_usd: float,
    dry_run: bool = False,
    api_key: str | None = None,
) -> _ClientProtocol:
    tracker = CostTracker(max_cost_usd=max_cost_usd)
    if dry_run:
        return DryRunClient(model=model, tracker=tracker)
    return RealAnthropicClient(model=model, tracker=tracker, api_key=api_key)


_JSON_BLOCK_RE = re.compile(r"```(?:json)?\s*\n(.*?)\n```", re.DOTALL)


def parse_json_response(text: str) -> dict | list | None:
    """Parse a JSON object/array from a model response.

    The prompt asks for raw JSON, but models sometimes wrap it in ```json
    anyway. Handle both, plus a last-resort substring extraction. Returns
    None if no parse works.
    """
    if not text or not text.strip():
        return None
    stripped = text.strip()
    try:
        return json.loads(stripped)
    except json.JSONDecodeError:
        pass

    m = _JSON_BLOCK_RE.search(text)
    if m:
        try:
            return json.loads(m.group(1))
        except json.JSONDecodeError:
            pass

    for opener, closer in (("{", "}"), ("[", "]")):
        start = text.find(opener)
        end = text.rfind(closer)
        if start != -1 and end > start:
            try:
                return json.loads(text[start : end + 1])
            except json.JSONDecodeError:
                continue
    return None
