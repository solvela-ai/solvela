"""Tests for the PR-diff agentic check. No real API calls, no real git.

The orchestrator shells out to `git diff <range>` to capture the diff; tests
monkeypatch `_capture_diff` to return canned text. Same hallucination guards
as the other agentic checks plus the new sticky-comment formatting.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

import pytest

from common import AuditContext, Severity
from agentic import pr_diff
from agentic.client import InvokeResult


@dataclass
class _StubClient:
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


def _result(text: str) -> InvokeResult:
    return InvokeResult(
        text=text,
        input_tokens=300,
        output_tokens=120,
        cost_usd=0.003,
        stopped_for="end_turn",
    )


def _make_repo_with_corpus(tmp_path: Path) -> AuditContext:
    readme = tmp_path / "README.md"
    readme.write_text(
        "# Solvela\n\nAI agent payment gateway.\n",
        encoding="utf-8",
    )
    return AuditContext(repo_root=tmp_path, doc_files=[readme])


@pytest.fixture
def stub_diff(monkeypatch):
    def _set(text: str, truncated: bool = False, err: str | None = None):
        monkeypatch.setattr(
            pr_diff,
            "_capture_diff",
            lambda repo_root, diff_range: (text, truncated, err),
        )

    return _set


def test_suggestion_in_corpus_is_returned(tmp_path: Path, stub_diff) -> None:
    ctx = _make_repo_with_corpus(tmp_path)
    stub_diff("diff --git a/x b/x\n+ added\n")
    response = json.dumps([
        {
            "doc": "README.md",
            "why": "chat command signature changed",
            "what_to_check": "verify the example, check default flags",
            "confidence": "high",
        }
    ])
    client = _StubClient(responses=[_result(response)])
    result = pr_diff.run(ctx, client=client, diff_range="HEAD~1..HEAD")
    assert len(result.findings) == 1
    assert result.findings[0].severity == Severity.INFO
    assert "README.md" in result.findings[0].message
    assert "[confidence: high]" in result.findings[0].message
    assert pr_diff.STICKY_SENTINEL in result.comment_markdown
    assert "README.md" in result.comment_markdown
    assert "chat command signature changed" in result.comment_markdown


def test_suggestion_outside_corpus_is_dropped(tmp_path: Path, stub_diff) -> None:
    ctx = _make_repo_with_corpus(tmp_path)
    stub_diff("diff --git a/x b/x\n+ added\n")
    response = json.dumps([
        {
            "doc": "imaginary/path.md",
            "why": "made up",
            "what_to_check": "ignore",
            "confidence": "high",
        }
    ])
    client = _StubClient(responses=[_result(response)])
    result = pr_diff.run(ctx, client=client, diff_range="HEAD~1..HEAD")
    assert result.findings == []
    # The clean-comment fallback must still include the sentinel.
    assert pr_diff.STICKY_SENTINEL in result.comment_markdown
    assert "No doc updates suggested" in result.comment_markdown


def test_invalid_confidence_defaults_to_medium(tmp_path: Path, stub_diff) -> None:
    ctx = _make_repo_with_corpus(tmp_path)
    stub_diff("diff content\n")
    response = json.dumps([
        {
            "doc": "README.md",
            "why": "something",
            "what_to_check": "check",
            "confidence": "wildly-unsupported",
        }
    ])
    client = _StubClient(responses=[_result(response)])
    result = pr_diff.run(ctx, client=client, diff_range="HEAD~1..HEAD")
    assert len(result.findings) == 1
    assert "[confidence: medium]" in result.findings[0].message


def test_empty_diff_returns_info_finding(tmp_path: Path, stub_diff) -> None:
    ctx = _make_repo_with_corpus(tmp_path)
    stub_diff("")
    client = _StubClient(responses=[])
    result = pr_diff.run(ctx, client=client, diff_range="HEAD~1..HEAD")
    assert len(result.findings) == 1
    assert result.findings[0].severity == Severity.INFO
    assert "empty" in result.findings[0].message
    assert "No doc updates suggested" in result.comment_markdown
    assert result.total_input_tokens == 0  # no client call


def test_diff_capture_error_emits_warning(tmp_path: Path, stub_diff) -> None:
    ctx = _make_repo_with_corpus(tmp_path)
    stub_diff("", err="bad ref")
    client = _StubClient(responses=[])
    result = pr_diff.run(ctx, client=client, diff_range="bogus..HEAD")
    assert len(result.findings) == 1
    assert result.findings[0].severity == Severity.WARNING
    assert "could not capture diff" in result.findings[0].message
    assert "bad ref" in result.comment_markdown


def test_truncated_diff_is_flagged_in_comment(tmp_path: Path, stub_diff) -> None:
    ctx = _make_repo_with_corpus(tmp_path)
    stub_diff("a" * 100, truncated=True)
    response = json.dumps([
        {
            "doc": "README.md",
            "why": "see above",
            "what_to_check": "check",
            "confidence": "low",
        }
    ])
    client = _StubClient(responses=[_result(response)])
    result = pr_diff.run(ctx, client=client, diff_range="HEAD~99..HEAD")
    assert result.truncated is True
    assert "(truncated)" in result.comment_markdown


def test_malformed_response_returns_empty_suggestions(
    tmp_path: Path, stub_diff
) -> None:
    ctx = _make_repo_with_corpus(tmp_path)
    stub_diff("diff content\n")
    client = _StubClient(responses=[_result("this is not JSON")])
    result = pr_diff.run(ctx, client=client, diff_range="HEAD~1..HEAD")
    assert result.findings == []
    assert pr_diff.STICKY_SENTINEL in result.comment_markdown
    assert "No doc updates suggested" in result.comment_markdown


def test_agent_error_yields_warning(tmp_path: Path, stub_diff) -> None:
    ctx = _make_repo_with_corpus(tmp_path)
    stub_diff("diff content\n")
    client = _StubClient(responses=[
        InvokeResult(
            text="",
            input_tokens=0,
            output_tokens=0,
            cost_usd=0.0,
            stopped_for="error:APIConnectionError: boom",
        ),
    ])
    result = pr_diff.run(ctx, client=client, diff_range="HEAD~1..HEAD")
    assert len(result.findings) == 1
    assert result.findings[0].severity == Severity.WARNING
    assert "agent call failed" in result.findings[0].message
    assert "boom" in result.comment_markdown


def test_budget_exhaustion_yields_warning(tmp_path: Path, stub_diff) -> None:
    ctx = _make_repo_with_corpus(tmp_path)
    stub_diff("diff content\n")
    client = _StubClient(responses=[
        InvokeResult(
            text="",
            input_tokens=0,
            output_tokens=0,
            cost_usd=0.0,
            stopped_for="budget",
        ),
    ])
    result = pr_diff.run(ctx, client=client, diff_range="HEAD~1..HEAD")
    assert len(result.findings) == 1
    assert result.findings[0].severity == Severity.WARNING
    assert "budget exhausted" in result.findings[0].message
    assert "budget exhausted" in result.comment_markdown


def test_sticky_sentinel_always_present(tmp_path: Path, stub_diff) -> None:
    """Whatever happens — suggestions, no suggestions, errors, dry-run — the
    comment must start with the sentinel so the workflow can find+replace it."""
    ctx = _make_repo_with_corpus(tmp_path)

    stub_diff("diff content\n")
    response = json.dumps([
        {
            "doc": "README.md",
            "why": "x",
            "what_to_check": "y",
            "confidence": "low",
        }
    ])
    client = _StubClient(responses=[_result(response)])
    r1 = pr_diff.run(ctx, client=client, diff_range="HEAD~1..HEAD")
    assert r1.comment_markdown.startswith(pr_diff.STICKY_SENTINEL)

    client = _StubClient(responses=[_result("[]")])
    r2 = pr_diff.run(ctx, client=client, diff_range="HEAD~1..HEAD")
    assert r2.comment_markdown.startswith(pr_diff.STICKY_SENTINEL)

    stub_diff("", err="boom")
    client = _StubClient(responses=[])
    r3 = pr_diff.run(ctx, client=client, diff_range="HEAD~1..HEAD")
    assert r3.comment_markdown.startswith(pr_diff.STICKY_SENTINEL)
