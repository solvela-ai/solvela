"""Tests for the SDK parity sweep. No real API calls — each test injects a
stub client whose `invoke()` returns canned text. Mirrors test_agentic.py.

Focus: the orchestrator's validation discipline holds for this check's
input shape (per-op bundles vs. per-page bundles). Same hallucination
guards as page_review.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

from common import AuditContext, Severity
from agentic import sdk_parity
from agentic.canonical_ops import (
    CANONICAL_OPS,
    OpSpec,
)
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


def _build_minimal_op_repo(tmp_path: Path) -> AuditContext:
    """Create a tmpdir with the SDK files canonical_ops references for
    init_wallet, so gather_op_context() returns a non-empty bundle."""
    (tmp_path / "sdks" / "typescript" / "src").mkdir(parents=True)
    (tmp_path / "sdks" / "python" / "src" / "solvela").mkdir(parents=True)
    (tmp_path / "sdks" / "go").mkdir(parents=True)
    (tmp_path / "crates" / "cli" / "src" / "commands").mkdir(parents=True)

    (tmp_path / "sdks" / "typescript" / "src" / "wallet.ts").write_text(
        "export function initWallet() { /* generate keypair */ }\n",
        encoding="utf-8",
    )
    (tmp_path / "sdks" / "typescript" / "src" / "signer.ts").write_text(
        "// signer\n", encoding="utf-8"
    )
    (tmp_path / "sdks" / "python" / "src" / "solvela" / "wallet.py").write_text(
        "def init_wallet():\n    pass\n", encoding="utf-8"
    )
    (tmp_path / "sdks" / "python" / "src" / "solvela" / "signer.py").write_text(
        "# signer\n", encoding="utf-8"
    )
    (tmp_path / "sdks" / "go" / "wallet.go").write_text(
        "package solvela\nfunc InitWallet() {}\n", encoding="utf-8"
    )
    (tmp_path / "sdks" / "go" / "signer.go").write_text(
        "// signer\n", encoding="utf-8"
    )
    (tmp_path / "crates" / "cli" / "src" / "commands" / "wallet.rs").write_text(
        "pub fn init() {}\n", encoding="utf-8"
    )

    return AuditContext(repo_root=tmp_path)


def _result(text: str) -> InvokeResult:
    return InvokeResult(
        text=text,
        input_tokens=200,
        output_tokens=80,
        cost_usd=0.002,
        stopped_for="end_turn",
    )


def test_wire_divergence_finding_is_returned(tmp_path: Path) -> None:
    ctx = _build_minimal_op_repo(tmp_path)
    response = json.dumps([
        {
            "severity": "error",
            "message": "TS uses camelCase but protocol uses snake_case",
            "file": "sdks/typescript/src/wallet.ts",
            "line": 1,
            "evidence_file": "crates/cli/src/commands/wallet.rs",
            "evidence_line": 1,
            "suggestion": "rename to snake_case to match protocol",
        }
    ])
    client = _StubClient(responses=[_result(response), _result("[]"), _result("[]")])
    result = sdk_parity.run(ctx, client=client, op_filter=["init_wallet"])
    assert len(result.findings) == 1
    f = result.findings[0]
    assert f.severity == Severity.ERROR
    assert "[init_wallet]" in f.message
    assert "camelCase" in f.message
    assert "evidence: crates/cli/src/commands/wallet.rs:1" in f.message
    assert f.file == "sdks/typescript/src/wallet.ts"


def test_finding_outside_bundle_is_dropped(tmp_path: Path) -> None:
    ctx = _build_minimal_op_repo(tmp_path)
    response = json.dumps([
        {
            "severity": "error",
            "message": "imaginary",
            "file": "sdks/imaginary/src/wallet.ts",
            "line": 1,
            "evidence_file": None,
            "evidence_line": None,
            "suggestion": None,
        }
    ])
    client = _StubClient(responses=[_result(response), _result("[]"), _result("[]")])
    result = sdk_parity.run(ctx, client=client, op_filter=["init_wallet"])
    assert result.findings == []


def test_finding_with_unbundled_evidence_demoted_to_info(tmp_path: Path) -> None:
    ctx = _build_minimal_op_repo(tmp_path)
    response = json.dumps([
        {
            "severity": "error",
            "message": "claim with bogus evidence",
            "file": "sdks/typescript/src/wallet.ts",
            "line": 1,
            "evidence_file": "not/a/file.rs",
            "evidence_line": 1,
            "suggestion": None,
        }
    ])
    client = _StubClient(responses=[_result(response), _result("[]"), _result("[]")])
    result = sdk_parity.run(ctx, client=client, op_filter=["init_wallet"])
    assert len(result.findings) == 1
    assert result.findings[0].severity == Severity.INFO
    assert "evidence:" not in result.findings[0].message


def test_malformed_json_yields_warning(tmp_path: Path) -> None:
    ctx = _build_minimal_op_repo(tmp_path)
    client = _StubClient(responses=[
        _result("not JSON"),
        _result("[]"),
        _result("[]"),
    ])
    result = sdk_parity.run(ctx, client=client, op_filter=["init_wallet"])
    assert len(result.findings) == 1
    assert result.findings[0].severity == Severity.WARNING
    assert "not valid JSON" in result.findings[0].message


def test_budget_exhaustion_stops(tmp_path: Path) -> None:
    """When the client returns stopped_for='budget', the orchestrator
    increments ops_skipped_for_budget and produces no findings for that op.
    Scoped to a single op so the other ops' 'no SDK source files' warnings
    don't pollute the assertion."""
    ctx = _build_minimal_op_repo(tmp_path)
    client = _StubClient(responses=[
        InvokeResult(text="", input_tokens=0, output_tokens=0, cost_usd=0.0, stopped_for="budget"),
    ])
    result = sdk_parity.run(ctx, client=client, op_filter=["init_wallet"])
    assert result.findings == []
    assert result.ops_audited == 0
    assert result.ops_skipped_for_budget == 1


def test_op_filter_narrows_to_subset(tmp_path: Path) -> None:
    ctx = _build_minimal_op_repo(tmp_path)
    client = _StubClient(responses=[_result("[]")])
    result = sdk_parity.run(ctx, client=client, op_filter=["init_wallet"])
    assert result.ops_audited == 1


def test_unknown_op_filter_returns_warning(tmp_path: Path) -> None:
    ctx = _build_minimal_op_repo(tmp_path)
    client = _StubClient(responses=[])
    result = sdk_parity.run(ctx, client=client, op_filter=["nonexistent"])
    assert len(result.findings) == 1
    assert "no canonical ops matched" in result.findings[0].message


def test_empty_sdk_sources_skips_op(tmp_path: Path) -> None:
    ctx = AuditContext(repo_root=tmp_path)
    client = _StubClient(responses=[])
    result = sdk_parity.run(ctx, client=client, op_filter=["init_wallet"])
    matching = [f for f in result.findings if "no SDK source files" in f.message]
    assert len(matching) == 1
    assert matching[0].severity == Severity.WARNING


def test_canonical_ops_registry_is_well_formed() -> None:
    """The registry itself shouldn't have shape bugs."""
    assert len(CANONICAL_OPS) >= 1
    seen_names: set[str] = set()
    for op in CANONICAL_OPS:
        assert isinstance(op, OpSpec)
        assert op.name
        assert op.name not in seen_names, f"duplicate op id: {op.name}"
        seen_names.add(op.name)
        assert op.description
        assert op.sdk_files
        for sdk_name, files in op.sdk_files.items():
            assert isinstance(sdk_name, str) and sdk_name
            assert isinstance(files, tuple)
            assert all(isinstance(f, str) for f in files)
