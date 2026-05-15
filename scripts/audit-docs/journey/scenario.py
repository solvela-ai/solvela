"""Step + Scenario dataclasses for the onboarding journey simulator.

A `Scenario` mirrors the commands in a quickstart doc. The runner executes
each `Step` in a controlled environment, captures the result, and compares
against the step's expectations. Divergences become Findings.

The expectation model is intentionally narrow:

  - `expected_exit`: int (typically 0) or None to skip exit check
  - `must_contain`: substrings that MUST appear in stdout+stderr
  - `must_not_contain`: substrings that MUST NOT appear (this is where the
    original-bug detection lives — "this step must not error with
    'SOLANA_RPC_URL required'")
  - `timeout_seconds`: per-step timeout

That's it. No regex matching, no flaky pattern logic. If a check needs more,
the scenario file can post-process the result manually.
"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass(frozen=True)
class Step:
    """One executable step of an onboarding journey."""

    name: str
    cmd: list[str]
    """argv list, e.g. ['solvela', 'chat', 'test', '--yes']."""
    expected_exit: int | None = 0
    """None = don't check exit code; integer = require exact match."""
    must_contain: tuple[str, ...] = ()
    """Substrings that MUST appear in combined stdout+stderr."""
    must_not_contain: tuple[str, ...] = ()
    """Substrings that MUST NOT appear in combined stdout+stderr."""
    timeout_seconds: int = 60
    """Hard limit per step. Default 60s; bump in the step for long commands."""
    why: str = ""
    """Human-readable description for failure messages."""


@dataclass(frozen=True)
class Scenario:
    """A complete onboarding-flow journey for one doc page."""

    name: str
    """Short id used in CLI flags (e.g. 'rust_cli')."""
    doc_relpath: str
    """Path to the doc this scenario mirrors. Used in finding messages."""
    description: str
    """One-line description for --journey help output."""
    env: dict[str, str]
    """Env vars to set BEFORE the first step, mirroring the doc's exports.
    These are the docs' claim about 'what you need to set to make this work'.
    If they're insufficient, the journey catches it."""
    steps: tuple[Step, ...]
    """Steps run in declared order. Failure of one step does NOT abort the
    journey — the runner reports all step failures so the user sees the full
    picture."""
    cli_dependent: bool = True
    """Set False for scenarios that don't need the built solvela binary.
    The runner skips the scenario (with an info finding) if cli_dependent
    is True and no binary is found."""

    extra_runtime_env: dict[str, str] = field(default_factory=dict)
    """Env additions that aren't in the doc but are needed for the journey
    to run at all (e.g. setting HOME to a tempdir so we don't pollute the
    real ~/.solvela/wallet.json). NEVER include vars that would change the
    behavior we're auditing — only infrastructure plumbing."""


@dataclass(frozen=True)
class StepResult:
    """Outcome of running one step."""

    step: Step
    actual_exit: int | None
    """None if the step timed out or couldn't be started."""
    stdout: str
    stderr: str
    duration_seconds: float
    error: str | None = None
    """Non-None for runner-level failures (spawn failure, timeout, etc.).
    Distinct from a step that ran but exited non-zero."""

    @property
    def combined(self) -> str:
        return (self.stdout or "") + "\n" + (self.stderr or "")
