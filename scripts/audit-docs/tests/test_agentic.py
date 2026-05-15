"""Tests for the agentic page-review check. No real API calls — every test
injects a stub client whose `invoke()` returns canned text.

Focus: the orchestrator's validation discipline. If an agent hallucinates a
file, fabricates a line number, returns malformed JSON, or exhausts the
budget, the orchestrator must catch it without crashing.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

from common import AuditContext, Severity
from agentic import page_review
from agentic.client import (
    CostTracker,
    DryRunClient,
    InvokeResult,
    _estimate_cost,
    _estimate_tokens,
    parse_json_response,
)


@dataclass
class _StubClient:
    """Returns a queued list of canned responses, one per invoke()."""

    responses: list[InvokeResult]
    _idx: int = 0
    total_cost_usd: float = 0.0
    total_input_tokens: int = 0
    total_output_tokens: int = 0

    def invoke(self, *, system: str, user: str, max_tokens: int = 2048) -> InvokeResult:
        if self._idx >= len(self.responses):
            return InvokeResult(
                text="[]",
                input_tokens=0,
                output_tokens=0,
                cost_usd=0.0,
                stopped_for="end_turn",
            )
        r = self.responses[self._idx]
        self._idx += 1
        self.total_cost_usd += r.cost_usd
        self.total_input_tokens += r.input_tokens
        self.total_output_tokens += r.output_tokens
        return r


def _build_repo(tmp_path: Path, doc_content: str = "# Hello\n\nbody line two.\n") -> AuditContext:
    (tmp_path / "Cargo.toml").write_text(
        "[workspace.package]\nversion = \"0.2.0\"\n", encoding="utf-8"
    )
    (tmp_path / "crates" / "protocol" / "src").mkdir(parents=True)
    (tmp_path / "crates" / "protocol" / "src" / "constants.rs").write_text(
        "pub const PLATFORM_FEE_PERCENT: u8 = 5;\n", encoding="utf-8"
    )
    (tmp_path / "crates" / "cli" / "src").mkdir(parents=True)
    (tmp_path / "crates" / "cli" / "src" / "main.rs").write_text(
        "// stub\n", encoding="utf-8"
    )
    (tmp_path / "config").mkdir()
    (tmp_path / "config" / "models.toml").write_text("[models.x]\n", encoding="utf-8")
    (tmp_path / "sdks" / "typescript").mkdir(parents=True)
    (tmp_path / "sdks" / "typescript" / "package.json").write_text(
        '{"version":"0.2.2"}\n', encoding="utf-8"
    )
    readme = tmp_path / "README.md"
    readme.write_text(doc_content, encoding="utf-8")
    return AuditContext(repo_root=tmp_path)


def _result(text: str) -> InvokeResult:
    return InvokeResult(
        text=text,
        input_tokens=100,
        output_tokens=50,
        cost_usd=0.001,
        stopped_for="end_turn",
    )


def test_agent_finding_is_returned(tmp_path: Path) -> None:
    ctx = _build_repo(tmp_path, doc_content="# H\n\nclaim 1\nclaim 2\n")
    response = json.dumps([
        {
            "severity": "error",
            "message": "doc claims X but source says Y",
            "file": "README.md",
            "line": 3,
            "evidence_file": "crates/protocol/src/constants.rs",
            "evidence_line": 1,
            "suggestion": "fix the claim",
        }
    ])
    client = _StubClient(responses=[_result(response)])
    result = page_review.run(ctx, client=client)
    assert len(result.findings) == 1
    f = result.findings[0]
    assert f.severity == Severity.ERROR
    assert "X but source says Y" in f.message
    assert "evidence: crates/protocol/src/constants.rs:1" in f.message
    assert f.file == "README.md"
    assert f.line == 3
    assert f.suggestion == "fix the claim"


def test_finding_with_fake_file_is_dropped(tmp_path: Path) -> None:
    """Hallucination guard: model claims a finding in a file we didn't send."""
    ctx = _build_repo(tmp_path)
    response = json.dumps([
        {
            "severity": "error",
            "message": "claim about a nonexistent file",
            "file": "imaginary/path.md",
            "line": 1,
            "evidence_file": None,
            "evidence_line": None,
            "suggestion": None,
        }
    ])
    client = _StubClient(responses=[_result(response)])
    result = page_review.run(ctx, client=client)
    assert result.findings == []


def test_finding_with_out_of_range_line_is_dropped(tmp_path: Path) -> None:
    ctx = _build_repo(tmp_path, doc_content="single line\n")
    response = json.dumps([
        {
            "severity": "warning",
            "message": "claim at line 999 of a 1-line doc",
            "file": "README.md",
            "line": 999,
            "evidence_file": None,
            "evidence_line": None,
            "suggestion": None,
        }
    ])
    client = _StubClient(responses=[_result(response)])
    result = page_review.run(ctx, client=client)
    assert result.findings == []


def test_finding_citing_unbundled_sot_is_demoted_to_info(tmp_path: Path) -> None:
    """The model cited a SOT file the orchestrator didn't include. The
    orchestrator demotes severity to info and strips the bogus evidence
    instead of crashing or silently accepting it."""
    ctx = _build_repo(tmp_path)
    response = json.dumps([
        {
            "severity": "error",
            "message": "claim with bogus evidence",
            "file": "README.md",
            "line": 1,
            "evidence_file": "not/a/real/sot.rs",
            "evidence_line": 42,
            "suggestion": None,
        }
    ])
    client = _StubClient(responses=[_result(response)])
    result = page_review.run(ctx, client=client)
    assert len(result.findings) == 1
    assert result.findings[0].severity == Severity.INFO
    assert "evidence:" not in result.findings[0].message


def test_malformed_json_yields_warning_not_crash(tmp_path: Path) -> None:
    ctx = _build_repo(tmp_path)
    client = _StubClient(responses=[_result("this is not JSON at all")])
    result = page_review.run(ctx, client=client)
    assert len(result.findings) == 1
    assert result.findings[0].severity == Severity.WARNING
    assert "not valid JSON" in result.findings[0].message


def test_fenced_json_is_parsed(tmp_path: Path) -> None:
    ctx = _build_repo(tmp_path, doc_content="# H\n\nline.\n")
    fenced = "```json\n" + json.dumps([
        {
            "severity": "info",
            "message": "fenced finding",
            "file": "README.md",
            "line": 1,
            "evidence_file": None,
            "evidence_line": None,
            "suggestion": None,
        }
    ]) + "\n```"
    client = _StubClient(responses=[_result(fenced)])
    result = page_review.run(ctx, client=client)
    assert len(result.findings) == 1
    assert result.findings[0].severity == Severity.INFO


def test_budget_exhaustion_stops_loop(tmp_path: Path) -> None:
    ctx = _build_repo(tmp_path)
    client = _StubClient(responses=[
        InvokeResult(text="", input_tokens=0, output_tokens=0, cost_usd=0.0, stopped_for="budget"),
    ])
    result = page_review.run(ctx, client=client)
    assert result.findings == []
    assert result.pages_skipped_for_budget == 1
    assert result.pages_audited == 0


def test_agent_error_recorded_as_warning(tmp_path: Path) -> None:
    ctx = _build_repo(tmp_path)
    client = _StubClient(responses=[
        InvokeResult(
            text="", input_tokens=0, output_tokens=0, cost_usd=0.0,
            stopped_for="error:APIConnectionError: boom",
        ),
    ])
    result = page_review.run(ctx, client=client)
    assert len(result.findings) == 1
    assert result.findings[0].severity == Severity.WARNING
    assert "agent call failed" in result.findings[0].message


def test_empty_corpus_returns_warning(tmp_path: Path) -> None:
    ctx = AuditContext(repo_root=tmp_path)
    client = _StubClient(responses=[])
    result = page_review.run(ctx, client=client, doc_filter=["does-not-exist.md"])
    assert len(result.findings) == 1
    assert "no docs matched" in result.findings[0].message


def test_parse_json_response_handles_bare_array() -> None:
    assert parse_json_response("[1, 2, 3]") == [1, 2, 3]


def test_parse_json_response_handles_fenced() -> None:
    assert parse_json_response('```json\n{"a": 1}\n```') == {"a": 1}


def test_parse_json_response_returns_none_on_garbage() -> None:
    assert parse_json_response("totally not json") is None


def test_parse_json_response_handles_empty() -> None:
    assert parse_json_response("") is None
    assert parse_json_response("   ") is None


def test_estimate_cost_uses_known_pricing() -> None:
    # 1M input + 1M output on Sonnet 4.6 = $3 + $15
    assert _estimate_cost("claude-sonnet-4-6", 1_000_000, 1_000_000) == 18.0
    # Unknown model defaults to Opus pricing (conservative).
    assert _estimate_cost("claude-future-9000", 1_000_000, 0) == 15.0


def test_estimate_tokens_floor() -> None:
    assert _estimate_tokens("") >= 1
    assert _estimate_tokens("x") >= 1


def test_dry_run_client_accumulates_cost() -> None:
    tracker = CostTracker(max_cost_usd=10.0)
    client = DryRunClient(model="claude-sonnet-4-6", tracker=tracker)
    r1 = client.invoke(system="sys", user="user prompt one", max_tokens=500)
    r2 = client.invoke(system="sys", user="user prompt two", max_tokens=500)
    assert r1.stopped_for == "dry-run"
    assert r2.stopped_for == "dry-run"
    assert client.total_cost_usd > 0
    assert tracker.total_cost_usd == client.total_cost_usd
