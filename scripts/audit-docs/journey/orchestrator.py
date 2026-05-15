"""Orchestrator for the journey check.

Loads a named scenario, runs it through the controlled-env subprocess
runner, and emits Phase-1-compatible `Finding` objects for any step that
diverges from its declared expectations.

Each scenario gets a fresh tempdir as HOME so wallet files (and any other
per-user CLI state) don't leak between runs. The tempdir is cleaned up
after the scenario completes.
"""

from __future__ import annotations

import dataclasses
import tempfile
from dataclasses import dataclass

from common import AuditContext, Finding, Severity, eprint

from journey.runner import evaluate, run_scenario
from journey.scenario import Scenario, StepResult
from journey.scenarios import rust_cli


CHECK_NAME = "journey"


SCENARIOS: dict[str, Scenario] = {
    rust_cli.SCENARIO.name: rust_cli.SCENARIO,
}


@dataclass
class JourneyResult:
    findings: list[Finding]
    scenarios_run: int
    scenarios_skipped: int
    total_steps: int
    failed_steps: int


def list_scenarios() -> list[tuple[str, str]]:
    """Return [(name, description), ...] for --journey help output."""
    return sorted((s.name, s.description) for s in SCENARIOS.values())


def run(ctx: AuditContext, scenario_names: list[str]) -> JourneyResult:
    """Run each named scenario in sequence. Each scenario gets a fresh
    tempdir HOME so per-user CLI state is isolated."""
    findings: list[Finding] = []
    scenarios_run = 0
    scenarios_skipped = 0
    total_steps = 0
    failed_steps = 0

    for name in scenario_names:
        scenario = SCENARIOS.get(name)
        if scenario is None:
            findings.append(
                Finding(
                    check=CHECK_NAME,
                    severity=Severity.ERROR,
                    message=(
                        f"unknown scenario `{name}`; "
                        f"valid: {sorted(SCENARIOS.keys())}"
                    ),
                )
            )
            continue

        if scenario.cli_dependent and ctx.cli_binary is None:
            findings.append(
                Finding(
                    check=CHECK_NAME,
                    severity=Severity.WARNING,
                    message=(
                        f"scenario `{name}` needs a built `solvela` binary "
                        "but none was found under target/release or "
                        "target/debug — skipping. Build with "
                        "`cargo build --release -p solvela-cli`."
                    ),
                )
            )
            scenarios_skipped += 1
            continue

        with tempfile.TemporaryDirectory(prefix=f"audit-journey-{name}-") as tmp:
            scenario_with_home = _with_tempdir_home(scenario, tmp)
            results = run_scenario(scenario_with_home, ctx.cli_binary)
            scenarios_run += 1
            total_steps += len(results)
            findings.extend(_results_to_findings(scenario, results))
            failed_steps += sum(1 for r in results if not evaluate(r)[0])

    return JourneyResult(
        findings=findings,
        scenarios_run=scenarios_run,
        scenarios_skipped=scenarios_skipped,
        total_steps=total_steps,
        failed_steps=failed_steps,
    )


def _with_tempdir_home(scenario: Scenario, tmp: str) -> Scenario:
    extra = dict(scenario.extra_runtime_env)
    extra["HOME"] = tmp
    return dataclasses.replace(scenario, extra_runtime_env=extra)


def _results_to_findings(
    scenario: Scenario, results: list[StepResult]
) -> list[Finding]:
    out: list[Finding] = []
    for r in results:
        ok, reasons = evaluate(r)
        if ok:
            continue
        msg_parts = [
            f"[{scenario.name}] step `{r.step.name}` failed: ",
            "; ".join(reasons),
        ]
        if r.step.why:
            msg_parts.append(f" — why this step exists: {r.step.why}")
        out.append(
            Finding(
                check=CHECK_NAME,
                severity=Severity.ERROR,
                message="".join(msg_parts),
                file=scenario.doc_relpath,
                suggestion=_short_suggestion(r),
            )
        )
    return out


def _short_suggestion(result: StepResult) -> str:
    out = (result.stdout or "").strip()
    err = (result.stderr or "").strip()
    tail = err if err else out
    if not tail:
        return "no stdout/stderr captured"
    if len(tail) > 240:
        tail = "…" + tail[-240:]
    return tail.replace("\n", " | ")


def print_summary(result: JourneyResult) -> None:
    eprint(
        f"journey: ran {result.scenarios_run} scenario(s), "
        f"skipped {result.scenarios_skipped}, "
        f"{result.failed_steps}/{result.total_steps} step(s) failed"
    )
