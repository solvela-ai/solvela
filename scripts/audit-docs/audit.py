#!/usr/bin/env python3
"""audit-docs — Solvela documentation drift detector.

Run from repo root:

    python scripts/audit-docs/audit.py            # all checks, pretty output
    python scripts/audit-docs/audit.py --format json
    python scripts/audit-docs/audit.py --check cli_examples
    python scripts/audit-docs/audit.py --check-external --strict

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

    selected = args.check or list(CHECKS.keys())
    findings: list[Finding] = []
    for name in selected:
        mod = CHECKS[name]
        if name == "links":
            findings.extend(mod.run(ctx, check_external=args.check_external))
        else:
            findings.extend(mod.run(ctx))

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
