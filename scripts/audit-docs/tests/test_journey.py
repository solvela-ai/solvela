"""Tests for the onboarding journey simulator. No real subprocess execution
— every test monkeypatches `subprocess.run` (and the timeout path) so the
runner's behavior can be exercised deterministically.

Focus: the runner's evaluation discipline. If a step is expected to exit 0
and exits 1, we want a Finding. If a step is expected to NOT contain
'SOLANA_RPC_URL required' and it does, we want a Finding. If subprocess
times out, we want a Finding. None of these should crash the runner.
"""

from __future__ import annotations

from pathlib import Path
from unittest.mock import MagicMock

from common import AuditContext, Severity
from journey import orchestrator
from journey.runner import _run_step, evaluate
from journey.scenario import Scenario, Step


def _mk_proc(stdout: str = "", stderr: str = "", returncode: int = 0):
    proc = MagicMock()
    proc.stdout = stdout
    proc.stderr = stderr
    proc.returncode = returncode
    return proc


def _stub_run(monkeypatch, proc):
    import journey.runner as runner_mod

    monkeypatch.setattr(
        runner_mod.subprocess, "run", MagicMock(return_value=proc)
    )


def _stub_run_raises(monkeypatch, exc):
    import journey.runner as runner_mod

    monkeypatch.setattr(
        runner_mod.subprocess, "run", MagicMock(side_effect=exc)
    )


def test_step_passes_when_expectations_match(monkeypatch) -> None:
    _stub_run(monkeypatch, _mk_proc(stdout="hello world", returncode=0))
    step = Step(
        name="t",
        cmd=["echo", "hello"],
        expected_exit=0,
        must_contain=("hello",),
    )
    result = _run_step(step, {"PATH": "/usr/bin"}, cli_path=None)
    ok, reasons = evaluate(result)
    assert ok and reasons == []


def test_wrong_exit_code_fails(monkeypatch) -> None:
    _stub_run(monkeypatch, _mk_proc(returncode=1))
    step = Step(name="t", cmd=["x"], expected_exit=0)
    result = _run_step(step, {"PATH": "/usr/bin"}, cli_path=None)
    ok, reasons = evaluate(result)
    assert not ok
    assert any("expected exit 0, got 1" in r for r in reasons)


def test_missing_required_substring_fails(monkeypatch) -> None:
    _stub_run(monkeypatch, _mk_proc(stdout="hi there", returncode=0))
    step = Step(name="t", cmd=["x"], must_contain=("welcome",))
    result = _run_step(step, {"PATH": "/usr/bin"}, cli_path=None)
    ok, reasons = evaluate(result)
    assert not ok
    assert any("welcome" in r for r in reasons)


def test_forbidden_substring_present_fails(monkeypatch) -> None:
    """Original-bug-detection path: stderr contains 'SOLANA_RPC_URL required'."""
    _stub_run(
        monkeypatch,
        _mk_proc(
            stderr="Error: SOLANA_RPC_URL required for payment signing.",
            returncode=1,
        ),
    )
    step = Step(
        name="chat-smoke-test",
        cmd=["solvela", "chat", "test"],
        expected_exit=None,
        must_not_contain=("SOLANA_RPC_URL required",),
    )
    result = _run_step(step, {"PATH": "/usr/bin"}, cli_path=None)
    ok, reasons = evaluate(result)
    assert not ok
    assert any("SOLANA_RPC_URL required" in r for r in reasons)


def test_chat_step_passes_when_failure_is_payment_not_env(monkeypatch) -> None:
    _stub_run(
        monkeypatch,
        _mk_proc(
            stderr="Error: insufficient USDC balance to cover payment.",
            returncode=1,
        ),
    )
    step = Step(
        name="chat-smoke-test",
        cmd=["solvela", "chat", "test"],
        expected_exit=None,
        must_not_contain=("SOLANA_RPC_URL required",),
    )
    result = _run_step(step, {"PATH": "/usr/bin"}, cli_path=None)
    ok, reasons = evaluate(result)
    assert ok and reasons == []


def test_timeout_recorded_as_error(monkeypatch) -> None:
    import subprocess

    _stub_run_raises(
        monkeypatch,
        subprocess.TimeoutExpired(cmd=["x"], timeout=5),
    )
    step = Step(name="t", cmd=["x"], timeout_seconds=5)
    result = _run_step(step, {"PATH": "/usr/bin"}, cli_path=None)
    ok, reasons = evaluate(result)
    assert not ok
    assert any("timeout" in r for r in reasons)
    assert result.actual_exit is None


def test_spawn_failure_recorded_as_error(monkeypatch) -> None:
    _stub_run_raises(monkeypatch, FileNotFoundError("no such binary"))
    step = Step(name="t", cmd=["does-not-exist"], timeout_seconds=5)
    result = _run_step(step, {"PATH": "/usr/bin"}, cli_path=None)
    ok, reasons = evaluate(result)
    assert not ok
    assert any("spawn failed" in r for r in reasons)


def test_cli_substitution_uses_provided_binary(monkeypatch) -> None:
    captured: dict = {}

    import journey.runner as runner_mod

    def _capture_run(argv, **kwargs):
        captured["argv"] = argv
        return _mk_proc()

    monkeypatch.setattr(runner_mod.subprocess, "run", _capture_run)

    step = Step(name="t", cmd=["solvela", "--version"])
    _run_step(step, {"PATH": "/usr/bin"}, cli_path="/path/to/built/solvela")
    assert captured["argv"][0] == "/path/to/built/solvela"
    assert captured["argv"][1] == "--version"


def test_orchestrator_skips_when_no_cli_binary(tmp_path: Path) -> None:
    ctx = AuditContext(repo_root=tmp_path, cli_binary=None)
    result = orchestrator.run(ctx, ["rust_cli"])
    assert result.scenarios_run == 0
    assert result.scenarios_skipped == 1
    assert len(result.findings) == 1
    assert result.findings[0].severity == Severity.WARNING
    assert "needs a built `solvela` binary" in result.findings[0].message


def test_orchestrator_unknown_scenario_is_error(tmp_path: Path) -> None:
    ctx = AuditContext(repo_root=tmp_path)
    result = orchestrator.run(ctx, ["nonexistent_scenario"])
    assert len(result.findings) == 1
    assert result.findings[0].severity == Severity.ERROR
    assert "unknown scenario" in result.findings[0].message


def test_orchestrator_runs_scenario_with_mocked_binary(
    tmp_path: Path, monkeypatch
) -> None:
    """End-to-end: orchestrator picks up the scenario, runs through the
    runner (with subprocess.run mocked), produces no findings when all
    steps pass."""
    import journey.runner as runner_mod

    monkeypatch.setattr(
        runner_mod.subprocess,
        "run",
        MagicMock(
            return_value=_mk_proc(
                stdout=(
                    "solvela 0.2.0\n"
                    "Commands:\n  wallet  ...\n  chat  ...\n  models  ...\n"
                    "  health  ...\n  PROMPT\nGenerated address: stubbed\n"
                ),
                stderr="",
                returncode=0,
            )
        ),
    )

    fake_binary = tmp_path / "solvela"
    fake_binary.write_text("# stub\n", encoding="utf-8")
    fake_binary.chmod(0o755)

    ctx = AuditContext(repo_root=tmp_path, cli_binary=fake_binary)
    result = orchestrator.run(ctx, ["rust_cli"])
    assert result.scenarios_run == 1
    assert result.failed_steps == 0
    assert result.findings == []


def test_orchestrator_catches_original_bug_pattern(
    tmp_path: Path, monkeypatch
) -> None:
    """When the chat smoke step encounters 'SOLANA_RPC_URL required' in
    stderr (the original bug surface), produce an error finding."""
    import journey.runner as runner_mod

    def _per_step_run(argv, **kwargs):
        if "chat" in argv and "--help" not in argv:
            return _mk_proc(
                stdout="",
                stderr="Error: SOLANA_RPC_URL required for payment signing.",
                returncode=1,
            )
        return _mk_proc(
            stdout=(
                "solvela 0.2.0\nCommands:\n  wallet  ...\n  chat  ...\n  "
                "models  ...\n  health  ...\n  PROMPT\nGenerated address: x\n"
            ),
            stderr="",
            returncode=0,
        )

    monkeypatch.setattr(runner_mod.subprocess, "run", _per_step_run)

    fake_binary = tmp_path / "solvela"
    fake_binary.write_text("# stub\n", encoding="utf-8")
    fake_binary.chmod(0o755)

    ctx = AuditContext(repo_root=tmp_path, cli_binary=fake_binary)
    result = orchestrator.run(ctx, ["rust_cli"])
    assert result.failed_steps >= 1
    chat_findings = [
        f for f in result.findings if "chat-smoke-test" in f.message
    ]
    assert len(chat_findings) == 1
    assert "SOLANA_RPC_URL required" in chat_findings[0].message
    assert chat_findings[0].severity == Severity.ERROR


def test_list_scenarios_includes_rust_cli() -> None:
    names = [n for n, _ in orchestrator.list_scenarios()]
    assert "rust_cli" in names


def test_personal_env_vars_dont_leak(monkeypatch) -> None:
    """The runner only passes PATH/LANG/LC_ALL/TERM from os.environ. A user's
    SOLANA_RPC_URL or similar must NOT leak into the subprocess env."""
    captured: dict = {}

    import journey.runner as runner_mod

    def _capture_run(argv, **kwargs):
        captured["env"] = kwargs.get("env")
        return _mk_proc()

    monkeypatch.setattr(runner_mod.subprocess, "run", _capture_run)
    monkeypatch.setenv("SOLANA_RPC_URL", "https://user.local/rpc")
    monkeypatch.setenv("PATH", "/usr/bin:/bin")

    step = Step(name="t", cmd=["solvela", "--version"])
    _run_step(step, {"PATH": "/usr/bin"}, cli_path=None)

    env = captured["env"]
    assert env is not None
    assert "PATH" in env
    assert "SOLANA_RPC_URL" not in env


def test_scenario_env_is_used(monkeypatch) -> None:
    captured: dict = {}

    import journey.runner as runner_mod

    def _capture_run(argv, **kwargs):
        captured["env"] = kwargs.get("env")
        return _mk_proc()

    monkeypatch.setattr(runner_mod.subprocess, "run", _capture_run)

    scenario = Scenario(
        name="test",
        doc_relpath="x.md",
        description="t",
        env={"SOLANA_RPC_URL": "https://from-doc/rpc"},
        steps=(Step(name="t", cmd=["solvela", "--version"]),),
        cli_dependent=False,
    )
    from journey.runner import run_scenario

    run_scenario(scenario, cli_binary=None)
    assert captured["env"]["SOLANA_RPC_URL"] == "https://from-doc/rpc"
