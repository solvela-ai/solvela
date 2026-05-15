"""Orchestrator for the per-page agentic review.

Walks the curated corpus, dispatches one agent per page, validates each
returned finding against citation discipline, and aggregates the results
as Phase-1-compatible `Finding` objects so they flow through the same
output pipeline as the mechanical checks.
"""

from __future__ import annotations

from dataclasses import dataclass

from common import AuditContext, Finding, Severity, eprint

from agentic.client import _ClientProtocol, parse_json_response
from agentic.prompts import SYSTEM_PROMPT, format_user_message
from agentic.surfaces import Doc, SourceContext, gather_context, select_corpus


CHECK_NAME = "agentic.page_review"

# Output cap per agent call. The system prompt asks for JSON only, so 2048
# is enough headroom for ~10-15 findings per page even with verbose models.
_MAX_OUTPUT_TOKENS = 2048


@dataclass
class PageReviewResult:
    findings: list[Finding]
    pages_audited: int
    pages_skipped_for_budget: int
    total_cost_usd: float
    total_input_tokens: int
    total_output_tokens: int


def run(
    ctx: AuditContext,
    client: _ClientProtocol,
    doc_filter: list[str] | None = None,
) -> PageReviewResult:
    """Run page review across the corpus. The caller controls cost via the
    client (budget cap lives in CostTracker)."""

    corpus = select_corpus(ctx.repo_root, doc_filter=doc_filter)
    context = gather_context(ctx.repo_root)

    if not corpus:
        return PageReviewResult(
            findings=[
                Finding(
                    check=CHECK_NAME,
                    severity=Severity.WARNING,
                    message=(
                        "no docs matched the curated corpus patterns. "
                        "check _USER_FACING_PATTERNS in agentic/surfaces.py "
                        "or your --doc filter"
                    ),
                )
            ],
            pages_audited=0,
            pages_skipped_for_budget=0,
            total_cost_usd=0.0,
            total_input_tokens=0,
            total_output_tokens=0,
        )

    findings: list[Finding] = []
    pages_audited = 0
    pages_skipped = 0

    for doc in corpus:
        user_msg = format_user_message(doc, context)
        result = client.invoke(
            system=SYSTEM_PROMPT,
            user=user_msg,
            max_tokens=_MAX_OUTPUT_TOKENS,
        )

        if result.stopped_for == "budget":
            pages_skipped += 1
            continue
        if result.stopped_for.startswith("error:"):
            findings.append(
                Finding(
                    check=CHECK_NAME,
                    severity=Severity.WARNING,
                    message=f"agent call failed: {result.stopped_for}",
                    file=doc.relpath,
                )
            )
            pages_audited += 1
            continue

        pages_audited += 1
        if result.stopped_for == "dry-run":
            continue

        findings.extend(_parse_findings(doc.relpath, result.text, context, doc))

    return PageReviewResult(
        findings=findings,
        pages_audited=pages_audited,
        pages_skipped_for_budget=pages_skipped,
        total_cost_usd=client.total_cost_usd,
        total_input_tokens=client.total_input_tokens,
        total_output_tokens=client.total_output_tokens,
    )


_REQUIRED_KEYS = {"severity", "message", "file", "line"}
_VALID_SEVERITIES = {"error", "warning", "info"}


def _parse_findings(
    doc_relpath: str,
    text: str,
    context: list[SourceContext],
    doc: Doc,
) -> list[Finding]:
    parsed = parse_json_response(text)
    if parsed is None:
        return [
            Finding(
                check=CHECK_NAME,
                severity=Severity.WARNING,
                message=(
                    "agent response was not valid JSON — skipping findings "
                    "for this page (a sign the system prompt is being ignored "
                    "or the model is degrading)"
                ),
                file=doc_relpath,
            )
        ]
    if not isinstance(parsed, list):
        return [
            Finding(
                check=CHECK_NAME,
                severity=Severity.WARNING,
                message="agent returned JSON but not an array",
                file=doc_relpath,
            )
        ]

    out: list[Finding] = []
    doc_lines = doc.text.count("\n") + 1
    sot_lookup = {s.relpath: s for s in context}

    for raw in parsed:
        if not isinstance(raw, dict):
            continue
        if not _REQUIRED_KEYS.issubset(raw.keys()):
            continue
        sev_raw = raw.get("severity")
        if sev_raw not in _VALID_SEVERITIES:
            continue
        file_claim = raw.get("file")
        line_claim = raw.get("line")
        if not isinstance(file_claim, str) or not isinstance(line_claim, int):
            continue

        # Citation discipline: the file claimed must be the doc we sent OR
        # one of the bundled SOT files. Anything else is a hallucination.
        if file_claim != doc_relpath and file_claim not in sot_lookup:
            continue
        if file_claim == doc_relpath and not (1 <= line_claim <= doc_lines):
            continue

        evidence_file = raw.get("evidence_file")
        evidence_line = raw.get("evidence_line")
        if evidence_file is not None and not isinstance(evidence_file, str):
            evidence_file = None
            evidence_line = None
        if evidence_file and evidence_file not in sot_lookup:
            # The agent cited a source it wasn't given. Demote to info.
            sev_raw = "info"
            evidence_file = None
            evidence_line = None

        msg_raw = raw.get("message")
        if not isinstance(msg_raw, str) or not msg_raw.strip():
            continue
        msg = msg_raw.strip()
        if evidence_file:
            cite = (
                f"{evidence_file}:{evidence_line}"
                if isinstance(evidence_line, int)
                else evidence_file
            )
            msg = f"{msg} (evidence: {cite})"

        sug_raw = raw.get("suggestion")
        suggestion = (
            sug_raw if isinstance(sug_raw, str) and sug_raw.strip() else None
        )

        out.append(
            Finding(
                check=CHECK_NAME,
                severity=Severity(sev_raw),
                message=msg,
                file=file_claim,
                line=line_claim,
                suggestion=suggestion,
            )
        )
    return out


def print_summary(result: PageReviewResult) -> None:
    """Log a one-shot summary to stderr after a run."""
    eprint(
        f"agentic.page_review: audited {result.pages_audited} page(s), "
        f"skipped {result.pages_skipped_for_budget} for budget, "
        f"in≈{result.total_input_tokens}tk out≈{result.total_output_tokens}tk "
        f"cost≈${result.total_cost_usd:.4f}"
    )
