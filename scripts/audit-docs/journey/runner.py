"""Subprocess executor for journey steps.

Runs each Step in a child process with a controlled environment:
  - The scenario's declared `env` (mirroring the doc's exports).
  - Plus `extra_runtime_env` (infra plumbing — e.g. a tempdir HOME).
  - Plus a minimal PATH so the binary is findable.

The user's personal env vars are NOT propagated. This is intentional: we
want to reproduce what a brand-new user following the docs would see.
"""

from __future__ import annotations

import os
import subprocess
import time
from pathlib import Path

from journey.scenario import Scenario, Step, StepResult


# Vars passed through from the calling environment because subprocess.run
# can't sensibly function without them. Keep this minimal — anything here
# is something we're explicitly NOT auditing.
_PASSTHROUGH_BASE_KEYS: tuple[str, ...] = (
    "PATH",
    "LANG",
    "LC_ALL",
    "TERM",
)


def run_scenario(scenario: Scenario, cli_binary: Path | None) -> list[StepResult]:
    """Execute every step in order. Always runs all steps — a failure does
    not short-circuit, because the operator needs to see the full picture."""
    base_env = _build_base_env(scenario)
    cli_path = str(cli_binary) if cli_binary else None

    results: list[StepResult] = []
    for step in scenario.steps:
        results.append(_run_step(step, base_env, cli_path))
    return results


def _build_base_env(scenario: Scenario) -> dict[str, str]:
    base: dict[str, str] = {}
    for key in _PASSTHROUGH_BASE_KEYS:
        if key in os.environ:
            base[key] = os.environ[key]
    base.update(scenario.env)
    base.update(scenario.extra_runtime_env)
    return base


def _run_step(
    step: Step, base_env: dict[str, str], cli_path: str | None
) -> StepResult:
    argv = list(step.cmd)
    if argv and argv[0] == "solvela" and cli_path is not None:
        argv[0] = cli_path

    start = time.monotonic()
    try:
        proc = subprocess.run(
            argv,
            env=base_env,
            capture_output=True,
            text=True,
            timeout=step.timeout_seconds,
            check=False,
        )
    except subprocess.TimeoutExpired as e:
        return StepResult(
            step=step,
            actual_exit=None,
            stdout=(
                e.stdout.decode("utf-8", errors="ignore")
                if isinstance(e.stdout, bytes)
                else (e.stdout or "")
            ),
            stderr=(
                e.stderr.decode("utf-8", errors="ignore")
                if isinstance(e.stderr, bytes)
                else (e.stderr or "")
            ),
            duration_seconds=time.monotonic() - start,
            error=f"timeout after {step.timeout_seconds}s",
        )
    except (FileNotFoundError, OSError) as e:
        return StepResult(
            step=step,
            actual_exit=None,
            stdout="",
            stderr="",
            duration_seconds=time.monotonic() - start,
            error=f"spawn failed: {e}",
        )

    return StepResult(
        step=step,
        actual_exit=proc.returncode,
        stdout=proc.stdout or "",
        stderr=proc.stderr or "",
        duration_seconds=time.monotonic() - start,
        error=None,
    )


def evaluate(result: StepResult) -> tuple[bool, list[str]]:
    """Compare a step result against its expectations. Returns
    (ok, [reasons_for_failure])."""
    if result.error is not None:
        return False, [f"runner error: {result.error}"]

    reasons: list[str] = []
    step = result.step

    if step.expected_exit is not None and result.actual_exit != step.expected_exit:
        reasons.append(
            f"expected exit {step.expected_exit}, got {result.actual_exit}"
        )

    combined = result.combined
    for needle in step.must_contain:
        if needle not in combined:
            reasons.append(f"output missing required substring: {needle!r}")
    for needle in step.must_not_contain:
        if needle in combined:
            reasons.append(f"output contains forbidden substring: {needle!r}")

    return (len(reasons) == 0, reasons)
