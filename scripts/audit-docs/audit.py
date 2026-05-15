#!/usr/bin/env python3
"""audit-docs — Solvela documentation drift detector.

Run from repo root:

    python scripts/audit-docs/audit.py            # all mechanical checks, pretty output
    python scripts/audit-docs/audit.py --format json
    python scripts/audit-docs/audit.py --check cli_examples
    python scripts/audit-docs/audit.py --check-external --strict

Agentic Phase 2 checks (LLM-driven, opt-in, cost-bearing):

    python scripts/audit-docs/audit.py --agentic page_review --dry-run
    python scripts/audit-docs/audit.py --agentic page_review --max-cost-usd 5
    python scripts/audit-docs/audit.py --agentic page_review --doc dashboard/content/docs/quickstart.mdx

Exit codes:
  0  no errors
  1  at least one error finding (or warning when --strict)
  2  invocation error (missing dep, broken fixture)
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


def _bootstrap_import_path() -> Path:
    here = Path(__file__).resolve().parent
    sys.path.insert(0, str(here))
    return here


_SCRIPT_DIR = _bootstrap_import_path()

from common import (  # noqa: E402
    AuditContext,
    Finding,
    Severity,
    eprint,
    find_cli_binary,
    find_doc_files,
    find_rust_sources,
    find_sdk_sources,
    format_findings_json,
    format_findings_text,
)
from checks import (  # noqa: E402
    cli_examples,
    env_vars,
    links,
    numeric_claims,
    subcommand_coverage,
)


CHECKS = {
    "cli_examples": cli_examples,
    "env_vars": env_vars,
    "subcommand_coverage": subcommand_coverage,
    "numeric_claims": numeric_claims,
    "links": links,
}


def _detect_repo_root() -> Path:
    p = _SCRIPT_DIR
    for _ in range(8):
        cargo = p / "Cargo.toml"
        if cargo.is_file():
            try:
                if "[workspace]" in cargo.read_text(encoding="utf-8"):
                    return p
            except OSError:
                pass
        p = p.parent
    raise RuntimeError(
        "could not locate solvela repo root (no Cargo.toml with [workspace] above this script)"
    )


def _run_agentic(args, ctx: AuditContext) -> list[Finding]:
    """Lazy-import the agentic layer so the mechanical-only path stays free
    of the anthropic SDK dependency."""
    from agentic import page_review  # noqa: WPS433
    from agentic.client import make_client  # noqa: WPS433

    try:
        client = make_client(
            model=args.model,
            max_cost_usd=args.max_cost_usd,
            dry_run=args.dry_run,
        )
    except RuntimeError as e:
        eprint(f"audit-docs: agentic setup failed: {e}")
        return [
            Finding(
                check="agentic",
                severity=Severity.ERROR,
                message=str(e),
            )
        ]

    findings: list[Finding] = []
    for name in args.agentic:
        if name == "page_review":
            result = page_review.run(ctx, client=client, doc_filter=args.doc)
            findings.extend(result.findings)
            page_review.print_summary(result)
        elif name == "sdk_parity":
            from agentic import sdk_parity  # noqa: WPS433

            result = sdk_parity.run(ctx, client=client, op_filter=args.op)
            findings.extend(result.findings)
            sdk_parity.print_summary(result)
        elif name == "pr_diff":
            from agentic import pr_diff  # noqa: WPS433

            result = pr_diff.run(ctx, client=client, diff_range=args.diff)
            findings.extend(result.findings)
            pr_diff.print_summary(result)
            if args.comment_out:
                try:
                    with open(args.comment_out, "w", encoding="utf-8") as fh:
                        fh.write(result.comment_markdown)
                except OSError as e:
                    eprint(
                        f"audit-docs: could not write --comment-out file: {e}"
                    )
        else:
            eprint(f"audit-docs: unknown agentic check: {name}")
    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="audit-docs",
        description="Mechanical docs drift detector for Solvela.",
    )
    parser.add_argument(
        "--check",
        action="append",
        choices=sorted(CHECKS.keys()),
        help="Run only the named check(s). May be repeated. Default: all.",
    )
    parser.add_argument(
        "--format",
        choices=["text", "json"],
        default="text",
        help="Output format (default: text).",
    )
    parser.add_argument(
        "--strict",
        action="store_true",
        help="Treat warnings as errors for the exit code.",
    )
    parser.add_argument(
        "--check-external",
        action="store_true",
        help="Also HEAD-check external URLs in the links check (slow, network-dependent).",
    )

    # Agentic (Phase 2) flags. Default off; --agentic opts in.
    parser.add_argument(
        "--agentic",
        action="append",
        choices=["page_review", "sdk_parity", "pr_diff"],
        help="Run an LLM-driven agentic check. May be repeated. Default: none.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Agentic only: estimate token use without calling the API.",
    )
    parser.add_argument(
        "--model",
        default="claude-sonnet-4-6",
        help="Agentic only: model id (default: claude-sonnet-4-6).",
    )
    parser.add_argument(
        "--max-cost-usd",
        type=float,
        default=5.0,
        help="Agentic only: hard ceiling on total spend per run (default: 5.0).",
    )
    parser.add_argument(
        "--doc",
        action="append",
        help=(
            "Agentic page_review only: narrow to one or more doc relpaths "
            "(repeatable). Useful for iterating on a single page."
        ),
    )
    parser.add_argument(
        "--op",
        action="append",
        help=(
            "Agentic sdk_parity only: narrow to one or more canonical op "
            "ids (repeatable). See agentic/canonical_ops.py for valid names."
        ),
    )
    parser.add_argument(
        "--diff",
        default="origin/main..HEAD",
        help=(
            "Agentic pr_diff only: git diff range to audit "
            "(default: origin/main..HEAD)."
        ),
    )
    parser.add_argument(
        "--comment-out",
        help=(
            "Agentic pr_diff only: also write the PR-comment markdown to "
            "this path. Used by the CI workflow."
        ),
    )

    args = parser.parse_args(argv)

    try:
        repo_root = _detect_repo_root()
    except RuntimeError as e:
        eprint(f"audit-docs: {e}")
        return 2

    ctx = AuditContext(
        repo_root=repo_root,
        cli_binary=find_cli_binary(repo_root),
        doc_files=find_doc_files(repo_root),
        source_files=find_rust_sources(repo_root),
        sdk_source_files=find_sdk_sources(repo_root),
    )

    # If the user asked for ONLY agentic checks, skip the mechanical pass.
    run_mechanical = not (args.agentic and not args.check)

    selected = args.check or (list(CHECKS.keys()) if run_mechanical else [])
    findings: list[Finding] = []
    for name in selected:
        mod = CHECKS[name]
        if name == "links":
            findings.extend(mod.run(ctx, check_external=args.check_external))
        else:
            findings.extend(mod.run(ctx))

    if args.agentic:
        findings.extend(_run_agentic(args, ctx))

    if args.format == "json":
        sys.stdout.write(format_findings_json(findings))
    else:
        sys.stdout.write(format_findings_text(findings, repo_root))

    has_error = any(f.severity == Severity.ERROR for f in findings)
    has_warning = any(f.severity == Severity.WARNING for f in findings)
    if has_error or (args.strict and has_warning):
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
